//! Growable per-connection HTTP/1.x framing driver (`Conn`), runtime `Limits`,
//! and zero-copy borrowed `Request<'a>` view.

use std::io::{self, Read};
use std::sync::OnceLock;

use tokio::io::{AsyncRead, AsyncReadExt};

use crate::http::parser::Error;

use super::dechunk::{Dechunker, MAX_CHUNK_LINE, MAX_TRAILER};
use super::head::{Body, Head, Method};
use super::scan::find_head_end;

const EAGER_BODY: usize = 64 * 1024; // bodies up to this are allocated exactly, in one step
const TIER_1: usize = 16 * 1024; // head buffer tier between `initial` and `max_header`
const RETAIN_MAX: usize = 64 * 1024; // buffers bigger than this are shrunk when idle
const CHUNK_SLACK: usize = MAX_CHUNK_LINE + MAX_TRAILER + 64;

/// Per-connection memory and request framing limits.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Limits {
    /// Initial per-connection buffer (`RV_REQUEST_BUFFER_SIZE`). Most requests fit in it.
    pub initial_buf: usize,
    /// Hard cap on request line + headers (`RV_MAX_HEADER_SIZE`), exceeding -> 431.
    pub max_header: usize,
    /// Hard cap on the (decoded) body (`RV_MAX_BODY_SIZE`), exceeding -> 413.
    pub max_body: usize,
}

impl Limits {
    pub const DEFAULT: Limits = Limits {
        initial_buf: 4 * 1024,
        max_header: 16 * 1024,
        max_body: 2 * 1024 * 1024,
    };

    /// Resolve limits from runtime environment variables (`RV_REQUEST_BUFFER_SIZE`,
    /// `RV_MAX_HEADER_SIZE`, `RV_MAX_BODY_SIZE`), falling back to [`Limits::DEFAULT`].
    ///
    /// Reads process environment variables on each call; prefer calling once at startup
    /// or using [`Limits::cached`] on hot paths.
    pub fn new() -> Limits {
        fn var(key: &str, default: usize) -> usize {
            std::env::var(key)
                .ok()
                .and_then(|v| v.trim().parse::<usize>().ok())
                .filter(|&n| n > 0)
                .unwrap_or(default)
        }
        let d = Limits::DEFAULT;
        let max_header = var("RV_MAX_HEADER_SIZE", d.max_header).min(u32::MAX as usize);
        Limits {
            initial_buf: var("RV_REQUEST_BUFFER_SIZE", d.initial_buf)
                .clamp(256, max_header.max(256)),
            max_header,
            max_body: var("RV_MAX_BODY_SIZE", d.max_body),
        }
    }

    /// Return cached process-wide limits initialized once from [`Limits::new`].
    #[inline]
    pub fn cached() -> Limits {
        static CACHED: OnceLock<Limits> = OnceLock::new();
        *CACHED.get_or_init(Self::new)
    }
}

#[derive(Clone, Copy, Debug)]
enum Phase {
    Head,
    /// `need` = `head_len + body_len`, counted from `pos`.
    Body {
        head_len: usize,
        need: usize,
    },
    Chunked {
        head_len: usize,
        dec: Dechunker,
    },
}

/// One complete request in the connection buffer. Pass it to [`Conn::request`] / [`Conn::consume`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Frame {
    pub head_len: usize,
    pub body_len: usize,
}

/// Per-connection growable buffer and resumable HTTP/1.x request state machine.
pub struct Conn {
    buf: Vec<u8>,   // always fully initialized; reads fill buf[filled..]
    pos: usize,     // first unconsumed byte
    filled: usize,  // end of valid bytes
    scanned: usize, // bytes of buf[pos..] already searched for the head terminator
    lim: Limits,
    phase: Phase,
    pending_100_continue: bool,
    pub head: Head,
}

impl Conn {
    pub fn new(lim: Limits) -> Conn {
        Conn {
            buf: vec![0u8; lim.initial_buf.max(1)], // calloc: pages are not touched until used
            pos: 0,
            filled: 0,
            scanned: 0,
            lim,
            phase: Phase::Head,
            pending_100_continue: false,
            head: Head::new(),
        }
    }

    #[inline]
    pub fn buffer_capacity(&self) -> usize {
        self.buf.len()
    }

    /// Returns `true` if there are unconsumed bytes buffered in this connection.
    #[inline]
    pub fn has_buffered_bytes(&self) -> bool {
        self.filled > self.pos
    }

    /// Atomically returns and clears whether an interim `100 Continue` response should be emitted
    /// before reading the request body.
    #[inline]
    pub fn take_expect_100_continue(&mut self) -> bool {
        let pending = self.pending_100_continue;
        self.pending_100_continue = false;
        pending
    }

    /// Attempts to advance and return one complete request strictly from already-buffered bytes
    /// without performing any socket I/O.
    ///
    /// Returns:
    /// - `Ok(Some(frame))` when a complete request is buffered and ready for [`Conn::request`].
    /// - `Ok(None)` when more bytes must be read from the peer via [`Conn::prepare_read`] and
    ///   [`Conn::fill`] / [`Conn::fill_async`].
    /// - `Err(err)` immediately on malformed syntax, unsupported protocol/encoding, or limit violation.
    pub fn try_next_request(&mut self) -> Result<Option<Frame>, Error> {
        loop {
            match self.phase {
                Phase::Head => {
                    if self.try_head()? {
                        continue;
                    }
                    return Ok(None);
                }
                Phase::Body { head_len, need } => {
                    let unread = self.filled - self.pos;
                    if unread >= need {
                        return Ok(Some(Frame {
                            head_len,
                            body_len: need - head_len,
                        }));
                    }
                    return Ok(None);
                }
                Phase::Chunked { head_len, mut dec } => {
                    let b0 = self.pos + head_len;
                    let (done, new_len) =
                        dec.advance(&mut self.buf[b0..self.filled], self.lim.max_body)?;
                    self.filled = b0 + new_len;
                    self.phase = Phase::Chunked { head_len, dec };
                    if done {
                        return Ok(Some(Frame {
                            head_len,
                            body_len: dec.w,
                        }));
                    }
                    return Ok(None);
                }
            }
        }
    }

    /// Ensures spare room exists in `self.buf[self.filled..]` appropriate for the current phase
    /// before calling [`Conn::fill`] or [`Conn::fill_async`].
    pub fn prepare_read(&mut self) {
        match self.phase {
            Phase::Head => {
                if self.filled == self.buf.len() {
                    // Out of room. If a consumed prefix exists, compacting is enough;
                    // otherwise climb a tier (initial -> 16 KiB -> max_header).
                    let want = if self.pos > 0 {
                        self.buf.len()
                    } else {
                        next_tier(self.buf.len(), self.lim.max_header)
                    };
                    self.reserve(want);
                }
            }
            Phase::Body { need, .. } => {
                let unread = self.filled - self.pos;
                // Exact allocation for ordinary bodies. For big ones grow geometrically
                // with the bytes that really arrived, so a lying Content-Length can't
                // make us reserve megabytes up front.
                let want = if need <= EAGER_BODY {
                    need
                } else {
                    need.min(unread.saturating_mul(2)).max(EAGER_BODY)
                };
                self.reserve(want);
            }
            Phase::Chunked { head_len, .. } => {
                let unread = self.filled - self.pos;
                let cap = head_len
                    .saturating_add(self.lim.max_body)
                    .saturating_add(CHUNK_SLACK);
                let want = unread
                    .saturating_mul(2)
                    .max(EAGER_BODY)
                    .min(cap)
                    .max(unread + 1);
                self.reserve(want);
            }
        }
    }

    /// Reads until one complete request is buffered from a synchronous [`Read`] stream.
    ///
    /// `Ok(None)` = peer closed cleanly between requests. Safe to call again after `WouldBlock`:
    /// all progress is preserved. The returned [`Frame`] stays valid until [`Conn::consume`].
    pub fn read_request<R: Read>(&mut self, r: &mut R) -> Result<Option<Frame>, Error> {
        loop {
            if let Some(frame) = self.try_next_request()? {
                return Ok(Some(frame));
            }
            self.prepare_read();
            if self.fill(r)? == 0 {
                return if matches!(self.phase, Phase::Head) && self.filled == self.pos {
                    Ok(None)
                } else {
                    Err(Error::Eof)
                };
            }
        }
    }

    /// Reads until one complete request is buffered from an asynchronous [`AsyncRead`] stream.
    ///
    /// `Ok(None)` = peer closed cleanly between requests.
    pub async fn read_request_async<R: AsyncRead + Unpin>(
        &mut self,
        r: &mut R,
    ) -> Result<Option<Frame>, Error> {
        loop {
            if let Some(frame) = self.try_next_request()? {
                return Ok(Some(frame));
            }
            self.prepare_read();
            if self.fill_async(r).await? == 0 {
                return if matches!(self.phase, Phase::Head) && self.filled == self.pos {
                    Ok(None)
                } else {
                    Err(Error::Eof)
                };
            }
        }
    }

    /// Reads bytes asynchronously into the connection's spare buffer space.
    #[inline]
    pub async fn fill_async<R: AsyncRead + Unpin>(&mut self, r: &mut R) -> Result<usize, Error> {
        debug_assert!(self.filled < self.buf.len());
        loop {
            match r.read(&mut self.buf[self.filled..]).await {
                Ok(n) => {
                    self.filled += n;
                    return Ok(n);
                }
                Err(e) if e.kind() == io::ErrorKind::Interrupted => {}
                Err(e) => return Err(Error::Io(e)),
            }
        }
    }

    /// Scans buffered bytes for the head terminator; on success parses the head and
    /// switches phase. Returns whether the phase changed.
    fn try_head(&mut self) -> Result<bool, Error> {
        let data = &self.buf[self.pos..self.filled];
        let scan_limit = data.len().min(self.lim.max_header);
        match find_head_end(&data[..scan_limit], self.scanned) {
            Err(()) => Err(Error::Malformed),
            Ok(None) => {
                self.scanned = scan_limit;
                if data.len() >= self.lim.max_header {
                    return Err(Error::HeaderTooLarge);
                }
                Ok(false)
            }
            Ok(Some(head_len)) => {
                if head_len > self.lim.max_header {
                    return Err(Error::HeaderTooLarge);
                }
                self.head.parse(&data[..head_len])?;
                let no_body_buffered_yet = head_len == data.len();
                let (phase, expects_body) = match self.head.body {
                    Body::None => (
                        Phase::Body {
                            head_len,
                            need: head_len,
                        },
                        false,
                    ),
                    Body::Length(n) => {
                        // Rejected from the header alone, before any body byte is buffered.
                        if n > self.lim.max_body {
                            return Err(Error::BodyTooLarge);
                        }
                        let need = head_len.checked_add(n).ok_or(Error::BodyTooLarge)?;
                        (Phase::Body { head_len, need }, n > 0)
                    }
                    Body::Chunked => (
                        Phase::Chunked {
                            head_len,
                            dec: Dechunker::new(),
                        },
                        true,
                    ),
                };
                self.pending_100_continue =
                    self.head.expect_100_continue && no_body_buffered_yet && expects_body;
                self.phase = phase;
                Ok(true)
            }
        }
    }

    /// Guarantees `buf.len() - pos >= want`, compacting and/or growing in one copy.
    fn reserve(&mut self, want: usize) {
        if self.buf.len() - self.pos >= want {
            return;
        }
        let unread = self.filled - self.pos;
        if want <= self.buf.len() {
            self.buf.copy_within(self.pos..self.filled, 0);
        } else {
            let mut nb = vec![0u8; want];
            nb[..unread].copy_from_slice(&self.buf[self.pos..self.filled]);
            self.buf = nb;
        }
        self.pos = 0;
        self.filled = unread;
    }

    /// Reads bytes synchronously into the connection's spare buffer space.
    #[inline]
    pub fn fill<R: Read>(&mut self, r: &mut R) -> Result<usize, Error> {
        debug_assert!(self.filled < self.buf.len());
        loop {
            match r.read(&mut self.buf[self.filled..]) {
                Ok(n) => {
                    self.filled += n;
                    return Ok(n);
                }
                Err(e) if e.kind() == io::ErrorKind::Interrupted => {}
                Err(e) => return Err(Error::Io(e)),
            }
        }
    }

    /// Borrowed view of the request described by `f`.
    #[inline]
    pub fn request(&self, f: Frame) -> Request<'_> {
        let h = self.pos + f.head_len;
        Request {
            head: &self.head,
            raw: &self.buf[self.pos..h],
            body: &self.buf[h..h + f.body_len],
        }
    }

    /// Drops the request. Pipelined bytes already read stay for the next call.
    pub fn consume(&mut self, f: Frame) {
        self.pos += f.head_len + f.body_len;
        self.scanned = 0;
        self.pending_100_continue = false;
        self.phase = Phase::Head;
        let unread = self.filled - self.pos;
        if unread == 0 {
            self.pos = 0;
            self.filled = 0;
        }
        // A one-off big upload must not pin its buffer for the life of the connection.
        if self.buf.len() > self.lim.initial_buf.max(RETAIN_MAX) && unread <= self.lim.initial_buf {
            let mut nb = vec![0u8; self.lim.initial_buf.max(1)];
            nb[..unread].copy_from_slice(&self.buf[self.pos..self.filled]);
            self.buf = nb;
            self.pos = 0;
            self.filled = unread;
        }
    }
}

fn next_tier(len: usize, max_header: usize) -> usize {
    let t = if len < TIER_1 {
        TIER_1.min(max_header)
    } else {
        max_header
    };
    t.max(len + 1)
}

/// Zero-copy borrowed HTTP/1.x request view backed by a [`Conn`] or raw slice buffer.
#[derive(Clone, Copy)]
pub struct Request<'a> {
    head: &'a Head,
    raw: &'a [u8],
    body: &'a [u8],
}

impl<'a> Request<'a> {
    #[inline]
    pub(crate) fn from_parts(head: &'a Head, raw: &'a [u8], body: &'a [u8]) -> Self {
        Self { head, raw, body }
    }

    #[inline]
    pub fn method(&self) -> Method {
        self.head.method
    }

    #[inline]
    pub fn raw_head(&self) -> &'a [u8] {
        self.raw
    }

    #[inline]
    pub fn minor(&self) -> u8 {
        self.head.minor
    }

    #[inline]
    pub fn target(&self) -> &'a [u8] {
        self.head.target.of(self.raw)
    }

    #[inline]
    pub fn path(&self) -> &'a [u8] {
        self.head.path.of(self.raw)
    }

    #[inline]
    pub fn query(&self) -> &'a [u8] {
        self.head.query.of(self.raw)
    }

    #[inline]
    pub fn body(&self) -> &'a [u8] {
        self.body
    }

    #[inline]
    pub fn keep_alive(&self) -> bool {
        self.head.keep_alive
    }

    #[inline]
    pub fn header_count(&self) -> usize {
        self.head.header_count()
    }

    #[inline]
    pub fn headers(&self) -> impl Iterator<Item = (&'a [u8], &'a [u8])> + 'a {
        let (head, raw) = (self.head, self.raw);
        head.header_spans()
            .iter()
            .map(move |&(n, v)| (n.of(raw), v.of(raw)))
    }

    /// Case-insensitive lookup; linear over at most `MAX_HEADERS` short names.
    #[inline]
    pub fn header(&self, name: &[u8]) -> Option<&'a [u8]> {
        self.headers()
            .find(|(n, _)| n.eq_ignore_ascii_case(name))
            .map(|(_, v)| v)
    }
}

impl Default for Limits {
    fn default() -> Self {
        Self::DEFAULT
    }
}
