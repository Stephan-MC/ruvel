//! Growable per-connection HTTP/1.x framing driver (`Conn`)
//! and zero-copy borrowed `Request<'a>` view.

use std::io::{self, Read};

use tokio::io::{AsyncRead, AsyncReadExt};

use crate::constants::{
    MAX_REQUEST_BODY_SIZE, REQUEST_BODY_SIZE, REQUEST_BUFFER_SIZE, REQUEST_HEADER_SIZE,
};
use crate::http::Status as HttpStatus;
use crate::http::parser::Error;

use super::dechunk::{Dechunker, MAX_CHUNK_LINE, MAX_TRAILER};
use super::head::{Body, Head, Method, find_head_end};

const TIER_1: usize = 16 * 1024; // head buffer tier between `initial` and `REQUEST_HEADER_SIZE`
const RETAIN_MAX: usize = 64 * 1024; // buffers bigger than this are shrunk when idle
const CHUNK_SLACK: usize = MAX_CHUNK_LINE + MAX_TRAILER + 64;
const SHRINK_THRESHOLD: usize = if REQUEST_BUFFER_SIZE > RETAIN_MAX {
    REQUEST_BUFFER_SIZE
} else {
    RETAIN_MAX
};

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
    // always fully initialized; reads fill buf[filled..]
    buf: Vec<u8>,

    /// first unconsumed byte
    pos: usize,

    /// end of valid bytes
    filled: usize,

    // bytes of buf[pos..] already searched for the head terminator
    scanned: usize,

    phase: Phase,
    pending_100_continue: bool,
    pub head: Head,
}

impl Default for Conn {
    #[inline]
    fn default() -> Self {
        Self::new()
    }
}

impl Conn {
    pub fn new() -> Conn {
        Conn {
            buf: vec![0u8; REQUEST_BUFFER_SIZE], // calloc: pages are not touched until used
            pos: 0,
            filled: 0,
            scanned: 0,
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
    /// - `Err(status)` immediately on malformed syntax, unsupported protocol/encoding, or limit violation.
    pub fn try_next_request(&mut self) -> Result<Option<Frame>, HttpStatus> {
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
                    let (done, new_len) = dec.advance(&mut self.buf[b0..self.filled])?;
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
                    // otherwise climb a tier (initial -> 16 KiB -> REQUEST_HEADER_SIZE).
                    let want = if self.pos > 0 {
                        self.buf.len()
                    } else {
                        next_tier(self.buf.len())
                    };
                    self.reserve(want);
                }
            }
            Phase::Body { head_len, need } => {
                let unread = self.filled - self.pos;
                let eager_body = head_len.saturating_add(REQUEST_BODY_SIZE);
                // Exact allocation for ordinary bodies up to `REQUEST_BODY_SIZE`. For larger ones
                // grow geometrically with the bytes that really arrived, so a lying Content-Length
                // cannot force megabytes to be reserved up front.
                let want = if need <= eager_body {
                    need
                } else {
                    need.min(unread.saturating_mul(2).max(eager_body).max(unread + 1))
                };
                self.reserve(want);
            }
            Phase::Chunked { head_len, .. } => {
                let unread = self.filled - self.pos;
                let eager_body = head_len.saturating_add(REQUEST_BODY_SIZE);
                let cap = head_len
                    .saturating_add(MAX_REQUEST_BODY_SIZE)
                    .saturating_add(CHUNK_SLACK);
                let want = unread
                    .saturating_mul(2)
                    .max(eager_body)
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
    pub async fn fill_async<R: AsyncRead + Unpin>(&mut self, r: &mut R) -> io::Result<usize> {
        debug_assert!(self.filled < self.buf.len());
        loop {
            match r.read(&mut self.buf[self.filled..]).await {
                Ok(n) => {
                    self.filled += n;
                    return Ok(n);
                }
                Err(e) if e.kind() == io::ErrorKind::Interrupted => {}
                Err(e) => return Err(e),
            }
        }
    }

    /// Parses or scans buffered bytes for the head terminator; on success switches phase.
    /// Returns whether the phase changed.
    ///
    /// On the first read (`scanned == 0`), executes a single-pass SIMD/scalar parse directly
    /// via [`Head::parse_prefix_status`]. Only if the head spans multiple reads or errors
    /// does it use incremental [`find_head_end`] scanning / error classification.
    fn try_head(&mut self) -> Result<bool, HttpStatus> {
        let data = &self.buf[self.pos..self.filled];
        if data.is_empty() {
            return Ok(false);
        }
        let scan_limit = data.len().min(REQUEST_HEADER_SIZE);

        let head_len = if self.scanned == 0 {
            match self.head.parse_prefix_status(&data[..scan_limit]) {
                Ok(Some(len)) => len,
                Ok(None) => {
                    if data.len() >= REQUEST_HEADER_SIZE {
                        return Err(HttpStatus::RequestHeaderFieldsTooLarge);
                    }
                    self.scanned = scan_limit;
                    return Ok(false);
                }
                Err(status) => {
                    return match find_head_end(&data[..scan_limit], 0) {
                        Err(()) => Err(HttpStatus::BadRequest),
                        Ok(None) => {
                            if data.len() >= REQUEST_HEADER_SIZE {
                                Err(HttpStatus::RequestHeaderFieldsTooLarge)
                            } else {
                                self.scanned = scan_limit;
                                Ok(false)
                            }
                        }
                        Ok(Some(len)) => {
                            if len > REQUEST_HEADER_SIZE {
                                Err(HttpStatus::RequestHeaderFieldsTooLarge)
                            } else {
                                Err(status)
                            }
                        }
                    };
                }
            }
        } else {
            match find_head_end(&data[..scan_limit], self.scanned) {
                Err(()) => return Err(HttpStatus::BadRequest),
                Ok(None) => {
                    if data.len() >= REQUEST_HEADER_SIZE {
                        return Err(HttpStatus::RequestHeaderFieldsTooLarge);
                    }
                    self.scanned = scan_limit;
                    return Ok(false);
                }
                Ok(Some(len)) => {
                    if len > REQUEST_HEADER_SIZE {
                        return Err(HttpStatus::RequestHeaderFieldsTooLarge);
                    }
                    self.head.parse(&data[..len])?;
                    len
                }
            }
        };

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
                if n > MAX_REQUEST_BODY_SIZE {
                    return Err(HttpStatus::PayloadTooLarge);
                }
                let need = head_len.checked_add(n).ok_or(HttpStatus::PayloadTooLarge)?;
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
    pub fn fill<R: Read>(&mut self, r: &mut R) -> io::Result<usize> {
        debug_assert!(self.filled < self.buf.len());
        loop {
            match r.read(&mut self.buf[self.filled..]) {
                Ok(n) => {
                    self.filled += n;
                    return Ok(n);
                }
                Err(e) if e.kind() == io::ErrorKind::Interrupted => {}
                Err(e) => return Err(e),
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
        if self.buf.len() > SHRINK_THRESHOLD && unread <= REQUEST_BUFFER_SIZE {
            let mut nb = vec![0u8; REQUEST_BUFFER_SIZE];
            nb[..unread].copy_from_slice(&self.buf[self.pos..self.filled]);
            self.buf = nb;
            self.pos = 0;
            self.filled = unread;
        }
    }
}

fn next_tier(len: usize) -> usize {
    let t = if len < TIER_1 {
        TIER_1.min(REQUEST_HEADER_SIZE)
    } else {
        REQUEST_HEADER_SIZE
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

    /// Case-insensitive lookup; linear over at most `MAX_REQUEST_HEADERS` short names.
    #[inline]
    pub fn header(&self, name: &[u8]) -> Option<&'a [u8]> {
        self.headers()
            .find(|(n, _)| n.eq_ignore_ascii_case(name))
            .map(|(_, v)| v)
    }
}
