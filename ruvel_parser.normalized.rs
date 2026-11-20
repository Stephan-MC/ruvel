//! ruvel_http: std-only HTTP/1.x request framing and head parser.
//!
//! No crates. The only `unsafe` is the SSE2 load in `find_lf` (SSE2 is part of the
//! x86_64 baseline, so there is no runtime detection); other targets use a portable
//! SWAR (u64) fallback.
//!
//! Design in one paragraph
//! -----------------------
//! * One growable, always-initialised `Vec<u8>` per connection. Reads go straight into
//!   its spare room; there is no per-read copy and no zeroing on the hot path.
//! * The head is *located* first (SIMD scan for LF, every LF must follow a CR), then
//!   parsed once over a complete slice. The parser is not resumable and has no
//!   "partial" branches.
//! * The parser outputs `(u32, u32)` spans into the buffer instead of slices, so the
//!   buffer may be compacted or reallocated between reads without invalidating anything.
//! * `RV_REQUEST_BUFFER_SIZE` is only the *initial* size. What users can really limit
//!   is `RV_MAX_HEADER_SIZE` (-> 431) and `RV_MAX_BODY_SIZE` (-> 413).
//! * Content-Length bodies are sized from the header; chunked bodies are decoded *in
//!   place* (framing bytes are squeezed out), so a handler always sees one contiguous
//!   `&[u8]` body and nothing is allocated per request.
//! * Every state is resumable, so non-blocking sockets (`WouldBlock`) just re-call
//!   `read_request` later.

use std::io::{self, Read};

pub const MAX_HEADERS: usize = 64;

const EAGER_BODY: usize = 64 * 1024; // bodies up to this are allocated exactly, in one step
const TIER_1: usize = 16 * 1024; // head buffer tier between `initial` and `max_header`
const RETAIN_MAX: usize = 64 * 1024; // buffers bigger than this are shrunk when idle
const MAX_CHUNK_LINE: usize = 1024; // chunk-size line incl. extensions
const MAX_TRAILER: usize = 8 * 1024; // total trailer bytes (discarded)
const CHUNK_SLACK: usize = MAX_CHUNK_LINE + MAX_TRAILER + 64;

// ───────────────────────────── limits ─────────────────────────────

#[derive(Clone, Copy, Debug)]
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

    pub fn from_env() -> Limits {
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
}

// ───────────────────────────── errors ─────────────────────────────

#[derive(Debug)]
pub enum Error {
    Io(io::Error),
    /// Peer closed the socket in the middle of a request.
    Eof,
    /// 400
    Malformed,
    /// 431
    HeaderTooLarge,
    /// 413 (answer, then close: the body was not read)
    BodyTooLarge,
    /// 501 (Transfer-Encoding other than plain `chunked`)
    UnsupportedEncoding,
    /// 505
    VersionNotSupported,
}

impl Error {
    /// Status code to answer with, if a response makes sense at all.
    pub fn status(&self) -> Option<u16> {
        match self {
            Error::Io(_) | Error::Eof => None,
            Error::Malformed => Some(400),
            Error::BodyTooLarge => Some(413),
            Error::HeaderTooLarge => Some(431),
            Error::UnsupportedEncoding => Some(501),
            Error::VersionNotSupported => Some(505),
        }
    }
}

impl From<io::Error> for Error {
    fn from(e: io::Error) -> Self {
        Error::Io(e)
    }
}

// ───────────────────── byte classes (const tables) ─────────────────────

const fn is_tchar(c: u8) -> bool {
    matches!(c,
        b'!' | b'#' | b'$' | b'%' | b'&' | b'\'' | b'*' | b'+' | b'-' | b'.' | b'^' | b'_'
        | b'`' | b'|' | b'~' | b'0'..=b'9' | b'a'..=b'z' | b'A'..=b'Z')
}
const fn is_uri(c: u8) -> bool {
    matches!(c, 0x21..=0x7E)
}
const fn is_value(c: u8) -> bool {
    // HTAB, SP..~, and obs-text. Everything else (other CTLs, DEL, CR, LF) ends/invalidates a value.
    c == b'\t' || matches!(c, 0x20..=0x7E) || c >= 0x80
}
const fn build_token() -> [bool; 256] {
    let mut t = [false; 256];
    let mut i = 0;
    while i < 256 {
        t[i] = is_tchar(i as u8);
        i += 1;
    }
    t
}
const fn build_uri() -> [bool; 256] {
    let mut t = [false; 256];
    let mut i = 0;
    while i < 256 {
        t[i] = is_uri(i as u8);
        i += 1;
    }
    t
}
const fn build_value() -> [bool; 256] {
    let mut t = [false; 256];
    let mut i = 0;
    while i < 256 {
        t[i] = is_value(i as u8);
        i += 1;
    }
    t
}
static TOKEN: [bool; 256] = build_token();
static URI: [bool; 256] = build_uri();
static VALUE: [bool; 256] = build_value();

/// Out-of-range reads yield NUL, which no grammar rule accepts, so a truncated or
/// garbage head simply fails the next check instead of needing a bounds branch per byte.
#[inline(always)]
fn at(b: &[u8], i: usize) -> u8 {
    match b.get(i) {
        Some(&c) => c,
        None => 0,
    }
}

// ─────────────────────── SIMD / SWAR primitives ───────────────────────

const LO: u64 = 0x0101_0101_0101_0101;
const HI: u64 = 0x8080_8080_8080_8080;

#[inline(always)]
fn load_u64(c: &[u8]) -> u64 {
    // callers pass exactly 8 bytes
    u64::from_le_bytes([c[0], c[1], c[2], c[3], c[4], c[5], c[6], c[7]])
}

/// Index of the first `\n`.
#[cfg(target_arch = "x86_64")]
#[inline]
pub fn find_lf(h: &[u8]) -> Option<usize> {
    use std::arch::x86_64::{
        __m128i, _mm_cmpeq_epi8, _mm_loadu_si128, _mm_movemask_epi8, _mm_set1_epi8,
    };
    let mut i = 0;
    // SAFETY: SSE2 is baseline on x86_64; each load reads 16 bytes fully inside `h`.
    unsafe {
        let needle = _mm_set1_epi8(b'\n' as i8);
        while i + 16 <= h.len() {
            let v = _mm_loadu_si128(h.as_ptr().add(i) as *const __m128i);
            let m = _mm_movemask_epi8(_mm_cmpeq_epi8(v, needle)) as u32;
            if m != 0 {
                return Some(i + m.trailing_zeros() as usize);
            }
            i += 16;
        }
    }
    h[i..].iter().position(|&b| b == b'\n').map(|p| i + p)
}

#[cfg(not(target_arch = "x86_64"))]
#[inline]
pub fn find_lf(h: &[u8]) -> Option<usize> {
    find_lf_swar(h)
}

/// Portable 8-bytes-at-a-time version. For little-endian loads the *lowest* flagged
/// byte of the classic zero-byte trick is exact (false positives only appear above it).
#[allow(dead_code)]
pub fn find_lf_swar(h: &[u8]) -> Option<usize> {
    let mut i = 0;
    while let Some(c) = h.get(i..i + 8) {
        let x = load_u64(c) ^ (LO * 0x0A);
        let m = x.wrapping_sub(LO) & !x & HI;
        if m != 0 {
            return Some(i + (m.trailing_zeros() >> 3) as usize);
        }
        i += 8;
    }
    h[i..].iter().position(|&b| b == b'\n').map(|p| i + p)
}

/// Flags bytes < 0x20 or == 0x7F (lowest flagged byte is exact, see above).
#[inline(always)]
fn ctl_mask(w: u64) -> u64 {
    let lt_20 = w.wrapping_sub(LO * 0x20) & !w & HI;
    let x = w ^ (LO * 0x7F);
    let eq_7f = x.wrapping_sub(LO) & !x & HI;
    lt_20 | eq_7f
}

/// Returns the index of the first byte that cannot be part of a field value
/// (normally the CR that ends the line). HTAB is legal and is stepped over.
#[cfg(target_arch = "x86_64")]
#[inline]
pub fn skip_value(b: &[u8], mut i: usize) -> usize {
    use std::arch::x86_64::{
        __m128i, _mm_cmpeq_epi8, _mm_loadu_si128, _mm_min_epu8, _mm_movemask_epi8, _mm_or_si128,
        _mm_set1_epi8,
    };
    loop {
        if i + 16 <= b.len() {
            // SAFETY: SSE2 is baseline on x86_64; the 16-byte load is inside `b`.
            let m = unsafe {
                let v = _mm_loadu_si128(b.as_ptr().add(i) as *const __m128i);
                // unsigned v <= 0x1F  (min_epu8(v, 0x1F) == v), or v == 0x7F
                let ctl = _mm_cmpeq_epi8(_mm_min_epu8(v, _mm_set1_epi8(0x1F)), v);
                let del = _mm_cmpeq_epi8(v, _mm_set1_epi8(0x7F));
                _mm_movemask_epi8(_mm_or_si128(ctl, del)) as u32
            };
            if m == 0 {
                i += 16;
                continue;
            }
            i += m.trailing_zeros() as usize;
            if b[i] == b'\t' {
                i += 1;
                continue;
            }
            return i;
        }
        while i < b.len() && VALUE[b[i] as usize] {
            i += 1;
        }
        return i;
    }
}

#[cfg(not(target_arch = "x86_64"))]
#[inline]
pub fn skip_value(b: &[u8], i: usize) -> usize {
    skip_value_swar(b, i)
}

/// Portable 8-bytes-at-a-time version of `skip_value`.
#[allow(dead_code)]
#[inline]
pub fn skip_value_swar(b: &[u8], mut i: usize) -> usize {
    loop {
        if let Some(c) = b.get(i..i + 8) {
            let m = ctl_mask(load_u64(c));
            if m == 0 {
                i += 8;
                continue;
            }
            i += (m.trailing_zeros() >> 3) as usize;
            if b[i] == b'\t' {
                i += 1;
                continue;
            }
            return i;
        }
        while i < b.len() && VALUE[b[i] as usize] {
            i += 1;
        }
        return i;
    }
}

/// Index of the first byte at/after `i` that is not an RFC 9110 `tchar`.
/// SSE2 fast path covers `[A-Za-z0-9-]` 16 bytes at a time (the overwhelmingly common
/// header-name alphabet); rarer token characters (`_ . ! #` ...) are stepped over with
/// the table. Same result as the table loop, without one mispredicted exit per byte.
#[cfg(target_arch = "x86_64")]
#[inline]
pub fn scan_name(b: &[u8], mut i: usize) -> usize {
    use std::arch::x86_64::{
        __m128i, _mm_and_si128, _mm_cmpeq_epi8, _mm_cmpgt_epi8, _mm_cmplt_epi8, _mm_loadu_si128,
        _mm_movemask_epi8, _mm_or_si128, _mm_set1_epi8,
    };
    while i + 16 <= b.len() {
        // SAFETY: SSE2 is baseline on x86_64; the 16-byte load is inside `b`.
        let m = unsafe {
            let v = _mm_loadu_si128(b.as_ptr().add(i) as *const __m128i);
            let lo = _mm_or_si128(v, _mm_set1_epi8(0x20)); // fold case: A-Z -> a-z
            let alpha = _mm_and_si128(
                _mm_cmpgt_epi8(lo, _mm_set1_epi8(0x60)),
                _mm_cmplt_epi8(lo, _mm_set1_epi8(0x7B)),
            );
            let digit = _mm_and_si128(
                _mm_cmpgt_epi8(v, _mm_set1_epi8(0x2F)),
                _mm_cmplt_epi8(v, _mm_set1_epi8(0x3A)),
            );
            let dash = _mm_cmpeq_epi8(v, _mm_set1_epi8(0x2D));
            let ok = _mm_or_si128(_mm_or_si128(alpha, digit), dash);
            !(_mm_movemask_epi8(ok) as u32) & 0xFFFF // bits set = outside the fast class
        };
        if m == 0 {
            i += 16;
            continue;
        }
        i += m.trailing_zeros() as usize;
        if TOKEN[b[i] as usize] {
            i += 1; // legal but unusual token char
            continue;
        }
        return i;
    }
    while i < b.len() && TOKEN[b[i] as usize] {
        i += 1;
    }
    i
}

#[cfg(not(target_arch = "x86_64"))]
#[inline]
pub fn scan_name(b: &[u8], i: usize) -> usize {
    scan_name_scalar(b, i)
}

#[allow(dead_code)]
#[inline]
pub fn scan_name_scalar(b: &[u8], mut i: usize) -> usize {
    while TOKEN[at(b, i) as usize] {
        i += 1;
    }
    i
}

/// Index of the first byte at/after `i` outside `0x21..=0x7E` (request-target alphabet).
#[cfg(target_arch = "x86_64")]
#[inline]
pub fn scan_uri(b: &[u8], mut i: usize) -> usize {
    use std::arch::x86_64::{
        __m128i, _mm_and_si128, _mm_cmpgt_epi8, _mm_cmplt_epi8, _mm_loadu_si128,
        _mm_movemask_epi8, _mm_set1_epi8,
    };
    while i + 16 <= b.len() {
        // SAFETY: SSE2 is baseline on x86_64; the 16-byte load is inside `b`.
        let m = unsafe {
            let v = _mm_loadu_si128(b.as_ptr().add(i) as *const __m128i);
            // signed compare: bytes >= 0x80 are negative and fail `> 0x20`
            let ok = _mm_and_si128(
                _mm_cmpgt_epi8(v, _mm_set1_epi8(0x20)),
                _mm_cmplt_epi8(v, _mm_set1_epi8(0x7F)),
            );
            !(_mm_movemask_epi8(ok) as u32) & 0xFFFF
        };
        if m == 0 {
            i += 16;
            continue;
        }
        return i + m.trailing_zeros() as usize;
    }
    while i < b.len() && URI[b[i] as usize] {
        i += 1;
    }
    i
}

#[cfg(not(target_arch = "x86_64"))]
#[inline]
pub fn scan_uri(b: &[u8], i: usize) -> usize {
    scan_uri_scalar(b, i)
}

#[allow(dead_code)]
#[inline]
pub fn scan_uri_scalar(b: &[u8], mut i: usize) -> usize {
    while URI[at(b, i) as usize] {
        i += 1;
    }
    i
}

/// Locates the end of the head (`\r\n\r\n`) in `data`, resuming at `from` (everything
/// before it was already examined). Every LF must be preceded by a CR; a bare LF is
/// rejected right here, so the parser can assume strict CRLF line endings. LFs *after*
/// the terminator (body bytes, the next request) are never inspected.
/// Returns the head length including the terminator.
///
/// x86_64: 64 bytes per step. Bit `j` of an LF/CR mask is byte `i + j`, so
///   bare LF      = lf & !(cr << 1)
///   terminator   = lf & (lf << 2)     (CRs are implied by the bare-LF check)
/// and the bare-LF check is only applied up to the terminator bit.
#[cfg(target_arch = "x86_64")]
pub fn find_head_end(data: &[u8], from: usize) -> Result<Option<usize>, ()> {
    use std::arch::x86_64::{
        __m128i, _mm_cmpeq_epi8, _mm_loadu_si128, _mm_movemask_epi8, _mm_set1_epi8,
    };
    #[inline(always)]
    unsafe fn eq_mask(v: __m128i, n: __m128i) -> u64 {
        _mm_movemask_epi8(_mm_cmpeq_epi8(v, n)) as u32 as u64
    }
    let mut i = from;
    let p = data.as_ptr();
    // SAFETY: SSE2 is baseline on x86_64; every load stays inside `data`
    // (guarded by `i + 64 <= len` / `i + 16 <= len`).
    unsafe {
        let nl = _mm_set1_epi8(b'\n' as i8);
        let cr = _mm_set1_epi8(b'\r' as i8);
        while i + 64 <= data.len() {
            let v0 = _mm_loadu_si128(p.add(i) as *const __m128i);
            let v1 = _mm_loadu_si128(p.add(i + 16) as *const __m128i);
            let v2 = _mm_loadu_si128(p.add(i + 32) as *const __m128i);
            let v3 = _mm_loadu_si128(p.add(i + 48) as *const __m128i);
            let lf = eq_mask(v0, nl) | eq_mask(v1, nl) << 16 | eq_mask(v2, nl) << 32 | eq_mask(v3, nl) << 48;
            // No LF in the block (long header values: cookies, tokens): nothing to check,
            // and the CR mask is not even computed.
            if lf != 0 {
                let c = eq_mask(v0, cr) | eq_mask(v1, cr) << 16 | eq_mask(v2, cr) << 32 | eq_mask(v3, cr) << 48;
                match check_block(data, i, lf, c) {
                    Ok(None) => {}
                    other => return other,
                }
            }
            i += 64;
        }
        while i + 16 <= data.len() {
            let v = _mm_loadu_si128(p.add(i) as *const __m128i);
            let lf = eq_mask(v, nl);
            if lf != 0 {
                match check_block(data, i, lf, eq_mask(v, cr)) {
                    Ok(None) => {}
                    other => return other,
                }
            }
            i += 16;
        }
    }
    find_head_end_scalar(data, i)
}

#[cfg(not(target_arch = "x86_64"))]
pub fn find_head_end(data: &[u8], from: usize) -> Result<Option<usize>, ()> {
    find_head_end_scalar(data, from)
}

#[cfg(target_arch = "x86_64")]
#[inline(always)]
fn check_block(data: &[u8], i: usize, lf: u64, cr: u64) -> Result<Option<usize>, ()> {
    // carries from the bytes just before the block
    let cr_before = (i >= 1 && data[i - 1] == b'\r') as u64;
    let lf_1 = (i >= 1 && data[i - 1] == b'\n') as u64;
    let lf_2 = (i >= 2 && data[i - 2] == b'\n') as u64;
    let bad = lf & !((cr << 1) | cr_before);
    let term = lf & ((lf << 2) | lf_2 | (lf_1 << 1));
    if term != 0 {
        let low = term & term.wrapping_neg();
        if bad & (low << 1).wrapping_sub(1) != 0 {
            return Err(());
        }
        return Ok(Some(i + low.trailing_zeros() as usize + 1));
    }
    if bad != 0 {
        Err(())
    } else {
        Ok(None)
    }
}

/// Reference / portable version: one `find_lf` per line.
pub fn find_head_end_scalar(data: &[u8], from: usize) -> Result<Option<usize>, ()> {
    let mut i = from;
    while i < data.len() {
        let lf = match find_lf(&data[i..]) {
            Some(off) => i + off,
            None => return Ok(None),
        };
        if lf == 0 || data[lf - 1] != b'\r' {
            return Err(());
        }
        if lf >= 3 && data[lf - 3] == b'\r' && data[lf - 2] == b'\n' {
            return Ok(Some(lf + 1));
        }
        i = lf + 1;
    }
    Ok(None)
}

// ───────────────────────────── head ─────────────────────────────

/// Byte range inside the head slice.
#[derive(Clone, Copy, Default, Debug, PartialEq, Eq)]
pub struct Span {
    pub start: u32,
    pub end: u32,
}

impl Span {
    #[inline]
    pub fn of<'a>(self, raw: &'a [u8]) -> &'a [u8] {
        &raw[self.start as usize..self.end as usize]
    }
}

#[inline(always)]
fn span(a: usize, b: usize) -> Span {
    Span { start: a as u32, end: b as u32 }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Method {
    Get,
    Head,
    Post,
    Put,
    Delete,
    Patch,
    Options,
    Other(Span),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Body {
    None,
    Length(usize),
    Chunked,
}

/// Parsed request head. Lives inside the connection and is reused for every request:
/// no per-request initialisation of the header table, no allocation.
pub struct Head {
    pub method: Method,
    /// HTTP/1.`minor`
    pub minor: u8,
    pub target: Span,
    pub keep_alive: bool,
    pub body: Body,
    pub host: Option<Span>,
    len: usize,
    hdrs: [(Span, Span); MAX_HEADERS],
}

impl Default for Head {
    fn default() -> Self {
        Head::new()
    }
}

impl Head {
    pub fn new() -> Head {
        Head {
            method: Method::Get,
            minor: 1,
            target: Span::default(),
            keep_alive: true,
            body: Body::None,
            host: None,
            len: 0,
            hdrs: [(Span { start: 0, end: 0 }, Span { start: 0, end: 0 }); MAX_HEADERS],
        }
    }

    pub fn header_count(&self) -> usize {
        self.len
    }

    /// `b` must be exactly the head, including the final CRLFCRLF.
    pub fn parse(&mut self, b: &[u8]) -> Result<(), Error> {
        if b.len() > u32::MAX as usize {
            return Err(Error::HeaderTooLarge);
        }
        self.len = 0;
        self.host = None;

        // ── request line: METHOD SP target SP HTTP/1.x CRLF ──
        let (method, mut i) = parse_method(b)?;
        self.method = method;

        let t0 = i;
        i = scan_uri(b, i);
        if i == t0 || at(b, i) != b' ' {
            return Err(Error::Malformed);
        }
        self.target = span(t0, i);
        i += 1;

        match b.get(i..i + 10) {
            Some(b"HTTP/1.1\r\n") => self.minor = 1,
            Some(b"HTTP/1.0\r\n") => self.minor = 0,
            Some(v) if v.starts_with(b"HTTP/") && v.ends_with(b"\r\n") => {
                return Err(Error::VersionNotSupported)
            }
            _ => return Err(Error::Malformed),
        }
        i += 10;

        // ── header fields ──
        let mut content_length: Option<usize> = None;
        let mut chunked = false;
        let (mut close, mut ka) = (false, false);

        loop {
            if at(b, i) == b'\r' {
                // blank line: must be the very end of the head
                if at(b, i + 1) != b'\n' || i + 2 != b.len() {
                    return Err(Error::Malformed);
                }
                break;
            }

            // name: token ':'  (a leading SP/HTAB = obs-fold, SP before ':' -> both fail here)
            let n0 = i;
            i = scan_name(b, i);
            if i == n0 || at(b, i) != b':' {
                return Err(Error::Malformed);
            }
            let n1 = i;
            i += 1;

            // OWS value OWS CRLF
            while matches!(at(b, i), b' ' | b'\t') {
                i += 1;
            }
            let v0 = i;
            i = skip_value(b, i);
            if at(b, i) != b'\r' || at(b, i + 1) != b'\n' {
                return Err(Error::Malformed);
            }
            let mut v1 = i;
            while v1 > v0 && matches!(b[v1 - 1], b' ' | b'\t') {
                v1 -= 1;
            }
            i += 2;

            // Only what framing needs is interpreted eagerly; switch on length first.
            let name = &b[n0..n1];
            let val = &b[v0..v1];
            match name.len() {
                4 if name.eq_ignore_ascii_case(b"host") => {
                    if self.host.is_some() {
                        return Err(Error::Malformed);
                    }
                    self.host = Some(span(v0, v1));
                }
                10 if name.eq_ignore_ascii_case(b"connection") => {
                    for tok in val.split(|&c| c == b',') {
                        let t = trim_ows(tok);
                        if t.eq_ignore_ascii_case(b"close") {
                            close = true;
                        } else if t.eq_ignore_ascii_case(b"keep-alive") {
                            ka = true;
                        }
                    }
                }
                14 if name.eq_ignore_ascii_case(b"content-length") => {
                    let n = parse_decimal(val).ok_or(Error::Malformed)?;
                    if content_length.is_some_and(|prev| prev != n) {
                        return Err(Error::Malformed);
                    }
                    content_length = Some(n);
                }
                17 if name.eq_ignore_ascii_case(b"transfer-encoding") => {
                    if chunked || !val.eq_ignore_ascii_case(b"chunked") {
                        return Err(Error::UnsupportedEncoding);
                    }
                    chunked = true;
                }
                _ => {}
            }

            if self.len == MAX_HEADERS {
                return Err(Error::HeaderTooLarge);
            }
            self.hdrs[self.len] = (span(n0, n1), span(v0, v1));
            self.len += 1;
        }

        // ── request-smuggling guards and framing ──
        if chunked && (content_length.is_some() || self.minor == 0) {
            return Err(Error::Malformed);
        }
        if self.minor == 1 && self.host.is_none() {
            return Err(Error::Malformed);
        }
        self.keep_alive = if self.minor == 1 { !close } else { ka && !close };
        self.body = if chunked {
            Body::Chunked
        } else if let Some(n) = content_length {
            Body::Length(n)
        } else {
            Body::None
        };
        Ok(())
    }
}

fn parse_method(b: &[u8]) -> Result<(Method, usize), Error> {
    Ok(match b {
        [b'G', b'E', b'T', b' ', ..] => (Method::Get, 4),
        [b'P', b'O', b'S', b'T', b' ', ..] => (Method::Post, 5),
        [b'P', b'U', b'T', b' ', ..] => (Method::Put, 4),
        [b'H', b'E', b'A', b'D', b' ', ..] => (Method::Head, 5),
        [b'D', b'E', b'L', b'E', b'T', b'E', b' ', ..] => (Method::Delete, 7),
        [b'P', b'A', b'T', b'C', b'H', b' ', ..] => (Method::Patch, 6),
        [b'O', b'P', b'T', b'I', b'O', b'N', b'S', b' ', ..] => (Method::Options, 8),
        _ => {
            let mut i = 0;
            while TOKEN[at(b, i) as usize] {
                i += 1;
            }
            if i == 0 || at(b, i) != b' ' {
                return Err(Error::Malformed);
            }
            (Method::Other(span(0, i)), i + 1)
        }
    })
}

fn parse_decimal(v: &[u8]) -> Option<usize> {
    if v.is_empty() {
        return None;
    }
    let mut n: usize = 0;
    for &c in v {
        let d = c.wrapping_sub(b'0');
        if d > 9 {
            return None; // also rejects '+', '-', ',', spaces
        }
        n = n.checked_mul(10)?.checked_add(d as usize)?;
    }
    Some(n)
}

fn trim_ows(mut s: &[u8]) -> &[u8] {
    while let [b' ' | b'\t', rest @ ..] = s {
        s = rest;
    }
    while let [rest @ .., b' ' | b'\t'] = s {
        s = rest;
    }
    s
}

// ─────────────────────── in-place chunked decoder ───────────────────────

#[derive(Clone, Copy, Debug)]
enum CState {
    Size,
    Data(usize),
    DataEnd,
    Trailer,
    Done,
}

#[inline]
fn hex(c: u8) -> u8 {
    match c {
        b'0'..=b'9' => c - b'0',
        b'a'..=b'f' => c - b'a' + 10,
        b'A'..=b'F' => c - b'A' + 10,
        _ => 0xFF,
    }
}

/// Decodes `Transfer-Encoding: chunked` inside the receive buffer. Decoded bytes are
/// written at `w`, raw bytes are read at `r >= w`; after each call the gap is squeezed
/// shut, so the body is contiguous and whatever follows it (a pipelined request)
/// directly follows the body.
#[derive(Clone, Copy, Debug)]
struct Dechunker {
    st: CState,
    w: usize,
    trailer: usize,
}

impl Dechunker {
    fn new() -> Self {
        Dechunker { st: CState::Size, w: 0, trailer: 0 }
    }

    /// `data` = raw bytes from the start of the body region. Returns (done, new data length).
    fn advance(&mut self, data: &mut [u8], max_body: usize) -> Result<(bool, usize), Error> {
        let mut w = self.w;
        let mut r = w;
        let done = loop {
            match self.st {
                CState::Size => {
                    let line = &data[r..];
                    let lf = match find_lf(line) {
                        Some(lf) => lf,
                        None if line.len() > MAX_CHUNK_LINE => return Err(Error::Malformed),
                        None => break false,
                    };
                    if lf == 0 || line[lf - 1] != b'\r' {
                        return Err(Error::Malformed);
                    }
                    let l = &line[..lf - 1];
                    let (mut n, mut k) = (0usize, 0usize);
                    while k < l.len() {
                        let d = hex(l[k]);
                        if d == 0xFF {
                            break;
                        }
                        if n > usize::MAX >> 4 {
                            return Err(Error::BodyTooLarge);
                        }
                        n = (n << 4) | d as usize;
                        k += 1;
                    }
                    // need >=1 hex digit; anything after them must be a `;ext` without CR
                    if k == 0 || (k < l.len() && (l[k] != b';' || l[k..].contains(&b'\r'))) {
                        return Err(Error::Malformed);
                    }
                    r += lf + 1;
                    if n == 0 {
                        self.st = CState::Trailer;
                    } else if n > max_body - w {
                        return Err(Error::BodyTooLarge);
                    } else {
                        self.st = CState::Data(n);
                    }
                }
                CState::Data(rem) => {
                    let avail = data.len() - r;
                    if avail == 0 {
                        break false;
                    }
                    let take = avail.min(rem);
                    data.copy_within(r..r + take, w);
                    r += take;
                    w += take;
                    if take == rem {
                        self.st = CState::DataEnd;
                    } else {
                        self.st = CState::Data(rem - take);
                        break false;
                    }
                }
                CState::DataEnd => match data.get(r..r + 2) {
                    None => break false,
                    Some(b"\r\n") => {
                        r += 2;
                        self.st = CState::Size;
                    }
                    Some(_) => return Err(Error::Malformed),
                },
                CState::Trailer => {
                    let line = &data[r..];
                    let lf = match find_lf(line) {
                        Some(lf) => lf,
                        None if line.len() > MAX_CHUNK_LINE => return Err(Error::HeaderTooLarge),
                        None => break false,
                    };
                    if lf == 0 || line[lf - 1] != b'\r' {
                        return Err(Error::Malformed);
                    }
                    self.trailer += lf + 1;
                    if self.trailer > MAX_TRAILER {
                        return Err(Error::HeaderTooLarge);
                    }
                    r += lf + 1;
                    if lf == 1 {
                        self.st = CState::Done;
                        break true;
                    }
                }
                CState::Done => break true,
            }
        };
        // squeeze out the framing bytes we consumed
        let total = data.len();
        data.copy_within(r..total, w);
        self.w = w;
        Ok((done, w + (total - r)))
    }
}

// ───────────────────────────── connection ─────────────────────────────

#[derive(Clone, Copy, Debug)]
enum Phase {
    Head,
    /// `need` = head_len + body length, counted from `pos`
    Body { head_len: usize, need: usize },
    Chunked { head_len: usize, dec: Dechunker },
}

/// One complete request in the buffer. Pass it to `Conn::request` / `Conn::consume`.
#[derive(Clone, Copy, Debug)]
pub struct Frame {
    pub head_len: usize,
    pub body_len: usize,
}

pub struct Conn {
    buf: Vec<u8>, // always fully initialised; reads fill buf[filled..]
    pos: usize,   // first unconsumed byte
    filled: usize, // end of valid bytes
    scanned: usize, // bytes of buf[pos..] already searched for the head terminator
    lim: Limits,
    phase: Phase,
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
            head: Head::new(),
        }
    }

    pub fn buffer_capacity(&self) -> usize {
        self.buf.len()
    }

    /// Reads until one complete request is buffered. `Ok(None)` = peer closed cleanly
    /// between requests. Safe to call again after `WouldBlock`: all progress is kept.
    /// The returned frame stays valid until `consume`.
    pub fn read_request<R: Read>(&mut self, r: &mut R) -> Result<Option<Frame>, Error> {
        loop {
            match self.phase {
                Phase::Head => {
                    if self.try_head()? {
                        continue;
                    }
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
                    if self.fill(r)? == 0 {
                        return if self.filled == self.pos { Ok(None) } else { Err(Error::Eof) };
                    }
                }
                Phase::Body { head_len, need } => {
                    let unread = self.filled - self.pos;
                    if unread >= need {
                        return Ok(Some(Frame { head_len, body_len: need - head_len }));
                    }
                    // Exact allocation for ordinary bodies. For big ones grow geometrically
                    // with the bytes that really arrived, so a lying Content-Length can't
                    // make us reserve megabytes up front.
                    let want = if need <= EAGER_BODY {
                        need
                    } else {
                        need.min(unread.saturating_mul(2)).max(EAGER_BODY)
                    };
                    self.reserve(want);
                    if self.fill(r)? == 0 {
                        return Err(Error::Eof);
                    }
                }
                Phase::Chunked { head_len, mut dec } => {
                    let b0 = self.pos + head_len;
                    let res = dec.advance(&mut self.buf[b0..self.filled], self.lim.max_body);
                    let (done, new_len) = res?;
                    self.filled = b0 + new_len;
                    self.phase = Phase::Chunked { head_len, dec };
                    if done {
                        return Ok(Some(Frame { head_len, body_len: dec.w }));
                    }
                    let unread = self.filled - self.pos;
                    let cap = head_len + self.lim.max_body.saturating_add(CHUNK_SLACK);
                    let want = unread.saturating_mul(2).max(EAGER_BODY).min(cap).max(unread + 1);
                    self.reserve(want);
                    if self.fill(r)? == 0 {
                        return Err(Error::Eof);
                    }
                }
            }
        }
    }

    /// Scans buffered bytes for the head terminator; on success parses the head and
    /// switches phase. Returns whether the phase changed.
    fn try_head(&mut self) -> Result<bool, Error> {
        let data = &self.buf[self.pos..self.filled];
        match find_head_end(data, self.scanned) {
            Err(()) => Err(Error::Malformed),
            Ok(None) => {
                self.scanned = data.len();
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
                self.phase = match self.head.body {
                    Body::None => Phase::Body { head_len, need: head_len },
                    Body::Length(n) => {
                        // Rejected from the header alone, before any body byte is buffered.
                        if n > self.lim.max_body {
                            return Err(Error::BodyTooLarge);
                        }
                        let need = head_len.checked_add(n).ok_or(Error::BodyTooLarge)?;
                        Phase::Body { head_len, need }
                    }
                    Body::Chunked => Phase::Chunked { head_len, dec: Dechunker::new() },
                };
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

    #[inline]
    fn fill<R: Read>(&mut self, r: &mut R) -> Result<usize, Error> {
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
    let t = if len < TIER_1 { TIER_1.min(max_header) } else { max_header };
    t.max(len + 1)
}

pub struct Request<'a> {
    head: &'a Head,
    raw: &'a [u8],
    body: &'a [u8],
}

impl<'a> Request<'a> {
    pub fn method(&self) -> Method {
        self.head.method
    }
    pub fn target(&self) -> &'a [u8] {
        self.head.target.of(self.raw)
    }
    pub fn body(&self) -> &'a [u8] {
        self.body
    }
    pub fn keep_alive(&self) -> bool {
        self.head.keep_alive
    }
    pub fn headers(&self) -> impl Iterator<Item = (&'a [u8], &'a [u8])> + 'a {
        let (head, raw) = (self.head, self.raw);
        head.hdrs[..head.len].iter().map(move |&(n, v)| (n.of(raw), v.of(raw)))
    }
    /// Case-insensitive lookup; linear over at most `MAX_HEADERS` short names.
    pub fn header(&self, name: &[u8]) -> Option<&'a [u8]> {
        self.headers().find(|(n, _)| n.eq_ignore_ascii_case(name)).map(|(_, v)| v)
    }
}

// ───────────────────────────── tests ─────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    /// Hands out at most `step` bytes per read(): worst-case TCP fragmentation.
    struct Dribble<'a> {
        data: &'a [u8],
        pos: usize,
        step: usize,
    }
    impl Read for Dribble<'_> {
        fn read(&mut self, out: &mut [u8]) -> io::Result<usize> {
            let n = self.step.min(out.len()).min(self.data.len() - self.pos);
            out[..n].copy_from_slice(&self.data[self.pos..self.pos + n]);
            self.pos += n;
            Ok(n)
        }
    }

    #[derive(Debug, PartialEq)]
    struct Got {
        method: String,
        target: Vec<u8>,
        body: Vec<u8>,
        keep_alive: bool,
        host: Vec<u8>,
    }

    fn run(stream: &[u8], step: usize, lim: Limits) -> Result<Vec<Got>, Error> {
        let mut conn = Conn::new(lim);
        let mut src = Dribble { data: stream, pos: 0, step };
        let mut out = Vec::new();
        while let Some(f) = conn.read_request(&mut src)? {
            {
                let q = conn.request(f);
                out.push(Got {
                    method: format!("{:?}", q.method()),
                    target: q.target().to_vec(),
                    body: q.body().to_vec(),
                    keep_alive: q.keep_alive(),
                    host: q.header(b"HOST").unwrap_or(b"").to_vec(),
                });
            }
            conn.consume(f);
        }
        Ok(out)
    }

    fn lim() -> Limits {
        Limits { initial_buf: 256, max_header: 8 * 1024, max_body: 1 << 20 }
    }

    const GET: &[u8] = b"GET /a?b=1 HTTP/1.1\r\nHost: x.test\r\nUser-Agent: t\r\nAccept: */*\r\n\r\n";

    fn post(body: &[u8]) -> Vec<u8> {
        let mut v = format!(
            "POST /p HTTP/1.1\r\nHost: x.test\r\nContent-Length: {}\r\n\r\n",
            body.len()
        )
        .into_bytes();
        v.extend_from_slice(body);
        v
    }

    fn chunked(parts: &[&[u8]]) -> Vec<u8> {
        let mut v = b"POST /c HTTP/1.1\r\nHost: x.test\r\nTransfer-Encoding: chunked\r\n\r\n".to_vec();
        for p in parts {
            v.extend_from_slice(format!("{:x};ext=1\r\n", p.len()).as_bytes());
            v.extend_from_slice(p);
            v.extend_from_slice(b"\r\n");
        }
        v.extend_from_slice(b"0\r\nX-Trailer: 1\r\n\r\n");
        v
    }

    #[test]
    fn head_fields() {
        let mut h = Head::new();
        h.parse(b"PUT /x HTTP/1.1\r\nHost: a\r\nConnection: Keep-Alive, close\r\nContent-Length: 5\r\nX-A:  v \t\r\n\r\n").unwrap();
        assert_eq!(h.method, Method::Put);
        assert_eq!(h.body, Body::Length(5));
        assert!(!h.keep_alive);
        assert_eq!(h.header_count(), 4);
        let mut h = Head::new();
        h.parse(b"GET / HTTP/1.0\r\nConnection: keep-alive\r\n\r\n").unwrap();
        assert!(h.keep_alive && h.minor == 0);
    }

    #[test]
    fn every_fragmentation_gives_same_result() {
        let mut stream = Vec::new();
        stream.extend_from_slice(GET);
        stream.extend_from_slice(&post(b"hello world"));
        stream.extend_from_slice(&chunked(&[b"abc", b"defghij", b"k"]));
        stream.extend_from_slice(GET);
        stream.extend_from_slice(&post(&vec![b'z'; 3000]));
        stream.extend_from_slice(&chunked(&[&vec![b'q'; 5000], b"tail"]));
        stream.extend_from_slice(b"GET /last HTTP/1.1\r\nHost: x.test\r\nConnection: close\r\n\r\n");

        let whole = run(&stream, usize::MAX, lim()).unwrap();
        assert_eq!(whole.len(), 7);
        assert_eq!(whole[2].body, b"abcdefghijk");
        assert_eq!(whole[5].body.len(), 5004);
        assert!(!whole[6].keep_alive);

        let mut steps: Vec<usize> = (1..=64).collect();
        steps.extend([97, 255, 256, 257, 1000, 4096, 8191]);
        for step in steps {
            let got = run(&stream, step, lim()).unwrap();
            assert_eq!(got, whole, "step={step}");
        }
    }

    #[test]
    fn small_initial_buffer_grows_for_headers_and_bodies() {
        let mut req = b"GET /big HTTP/1.1\r\nHost: x.test\r\n".to_vec();
        req.extend_from_slice(b"X-Big: ");
        req.extend_from_slice(&vec![b'a'; 6000]);
        req.extend_from_slice(b"\r\n\r\n");
        let l = Limits { initial_buf: 256, max_header: 8192, max_body: 4 << 20 };
        assert_eq!(run(&req, 100, l).unwrap().len(), 1);

        let body = vec![b'x'; 1 << 20]; // lazy (geometric) growth path
        let got = run(&post(&body), 8192, l).unwrap();
        assert_eq!(got[0].body.len(), body.len());
    }

    #[test]
    fn buffer_shrinks_after_big_request() {
        let l = Limits { initial_buf: 1024, max_header: 8192, max_body: 4 << 20 };
        let mut stream = post(&vec![b'x'; 300_000]);
        stream.extend_from_slice(GET);
        let mut conn = Conn::new(l);
        let mut src = Dribble { data: &stream, pos: 0, step: 65536 };
        let f = conn.read_request(&mut src).unwrap().unwrap();
        assert!(conn.buffer_capacity() >= 300_000);
        conn.consume(f);
        assert_eq!(conn.buffer_capacity(), 1024);
        let f = conn.read_request(&mut src).unwrap().unwrap(); // pipelined leftover survives
        assert_eq!(conn.request(f).target(), b"/a?b=1");
    }

    fn status(stream: &[u8], l: Limits) -> Option<u16> {
        match run(stream, 7, l) {
            Err(e) => e.status(),
            Ok(_) => Some(200),
        }
    }

    #[test]
    fn rejections() {
        let l = lim();
        // limits
        let mut big = b"GET / HTTP/1.1\r\nHost: x\r\nX: ".to_vec();
        big.extend_from_slice(&vec![b'a'; 9000]);
        big.extend_from_slice(b"\r\n\r\n");
        assert_eq!(status(&big, l), Some(431));
        let cl = b"POST / HTTP/1.1\r\nHost: x\r\nContent-Length: 99999999\r\n\r\n";
        assert_eq!(status(cl, l), Some(413));
        let mut many = b"GET / HTTP/1.1\r\nHost: x\r\n".to_vec();
        for i in 0..MAX_HEADERS {
            many.extend_from_slice(format!("H{i}: v\r\n").as_bytes());
        }
        many.extend_from_slice(b"\r\n");
        assert_eq!(status(&many, l), Some(431));
        // chunked body over the limit
        let small = Limits { max_body: 10, ..l };
        assert_eq!(status(&chunked(&[b"0123456", b"789abc"]), small), Some(413));
        // smuggling / malformed
        let cases: &[&[u8]] = &[
            b"GET / HTTP/1.1\nHost: x\n\n",                                              // bare LF
            b"GET / HTTP/1.1\r\nHost: x\r\nFoo: a\nb\r\n\r\n",                           // bare LF in value
            b"GET / HTTP/1.1\r\nHost: x\r\nFoo: a\rb\r\n\r\n",                           // bare CR in value
            b"GET / HTTP/1.1\r\nHost : x\r\n\r\n",                                       // SP before colon
            b"GET / HTTP/1.1\r\nHost: x\r\n folded\r\n\r\n",                             // obs-fold
            b"GET / HTTP/1.1\r\n\r\n",                                                   // no Host
            b"GET / HTTP/1.1\r\nHost: a\r\nHost: b\r\n\r\n",                             // two Hosts
            b"POST / HTTP/1.1\r\nHost: x\r\nContent-Length: 1\r\nContent-Length: 2\r\n\r\nab",
            b"POST / HTTP/1.1\r\nHost: x\r\nContent-Length: 1, 1\r\n\r\na",
            b"POST / HTTP/1.1\r\nHost: x\r\nContent-Length: +1\r\n\r\na",
            b"POST / HTTP/1.1\r\nHost: x\r\nContent-Length: 1\r\nTransfer-Encoding: chunked\r\n\r\n0\r\n\r\n",
            b"G(T / HTTP/1.1\r\nHost: x\r\n\r\n",                                         // bad method
            b"GET  / HTTP/1.1\r\nHost: x\r\n\r\n",                                       // double SP
            b"GET /a b HTTP/1.1\r\nHost: x\r\n\r\n",                                     // SP in target
            b"GET / HTTP/1.1\r\nHost: x\r\nA\x01: v\r\n\r\n",                            // CTL in name
            b"GET / HTTP/1.1\r\nHost: x\r\nA: v\x00\r\n\r\n",                            // NUL in value
            b"GET / HTTP/1.1\r\nHost: x\r\nA: v\x7f\r\n\r\n",                            // DEL in value
            b"POST / HTTP/1.1\r\nHost: x\r\nTransfer-Encoding: chunked\r\n\r\nzz\r\n",   // bad chunk size
            b"POST / HTTP/1.1\r\nHost: x\r\nTransfer-Encoding: chunked\r\n\r\n1\r\nabX0\r\n\r\n", // missing CRLF after data
        ];
        for (i, c) in cases.iter().enumerate() {
            assert_eq!(status(c, l), Some(400), "case {i}: {}", String::from_utf8_lossy(c));
        }
        assert_eq!(status(b"GET / HTTP/2.0\r\nHost: x\r\n\r\n", l), Some(505));
        assert_eq!(
            status(b"POST / HTTP/1.1\r\nHost: x\r\nTransfer-Encoding: gzip, chunked\r\n\r\n", l),
            Some(501)
        );
    }

    #[test]
    fn eof_handling() {
        assert!(run(b"", 1, lim()).unwrap().is_empty()); // clean close
        assert!(matches!(run(&GET[..GET.len() - 1], 5, lim()), Err(Error::Eof)));
        let p = post(b"0123456789");
        assert!(matches!(run(&p[..p.len() - 1], 5, lim()), Err(Error::Eof)));
    }

    // ── tiny PRNG so the property tests need no crates ──
    struct Rng(u64);
    impl Rng {
        fn next(&mut self) -> u64 {
            self.0 ^= self.0 << 13;
            self.0 ^= self.0 >> 7;
            self.0 ^= self.0 << 17;
            self.0
        }
        fn below(&mut self, n: usize) -> usize {
            (self.next() % n as u64) as usize
        }
    }

    #[test]
    fn find_lf_matches_naive() {
        let mut rng = Rng(0x9E3779B97F4A7C15);
        for _ in 0..200_000 {
            let len = rng.below(80);
            let buf: Vec<u8> = (0..len)
                .map(|_| match rng.below(6) {
                    0 => b'\n',
                    1 => b'\r',
                    2 => 0x8A, // 0x0A with the high bit set: classic SWAR trap
                    3 => 0x0B,
                    4 => 0x09,
                    _ => rng.next() as u8,
                })
                .collect();
            let want = buf.iter().position(|&b| b == b'\n');
            assert_eq!(find_lf(&buf), want, "{buf:?}");
            assert_eq!(find_lf_swar(&buf), want, "{buf:?}");
        }
    }

    #[test]
    fn skip_value_matches_naive() {
        let mut rng = Rng(0xDEADBEEFCAFEF00D);
        for _ in 0..300_000 {
            let len = rng.below(70);
            let buf: Vec<u8> = (0..len)
                .map(|_| match rng.below(8) {
                    0 => b'\t',
                    1 => 0x7F,
                    2 => 0xFF,
                    3 => 0x80,
                    4 => b'\r',
                    5 => (rng.below(0x20)) as u8,
                    _ => 0x20 + rng.below(0x5F) as u8,
                })
                .collect();
            let start = rng.below(len + 1);
            let mut want = start;
            while want < buf.len() && VALUE[buf[want] as usize] {
                want += 1;
            }
            assert_eq!(skip_value(&buf, start), want, "{buf:?} from {start}");
        }
    }

    #[test]
    fn skip_value_swar_matches_naive() {
        let mut rng = Rng(0xA5A5_5A5A_1234_4321);
        for _ in 0..200_000 {
            let len = rng.below(70);
            let buf: Vec<u8> = (0..len)
                .map(|_| match rng.below(8) {
                    0 => b'\t',
                    1 => 0x7F,
                    2 => 0xFF,
                    3 => 0x80,
                    4 => b'\r',
                    5 => rng.below(0x20) as u8,
                    _ => 0x20 + rng.below(0x5F) as u8,
                })
                .collect();
            let start = rng.below(len + 1);
            assert_eq!(skip_value_swar(&buf, start), skip_value(&buf, start), "{buf:?} from {start}");
        }
    }

    #[test]
    fn find_head_end_block_matches_scalar() {
        let mut rng = Rng(0x0BAD_C0DE_F00D_1234);
        for round in 0..400_000 {
            let len = rng.below(300);
            // few distinct symbols so CRLFCRLF, bare LFs and near misses are all common
            let mut buf: Vec<u8> = (0..len)
                .map(|_| match rng.below(10) {
                    0..=2 => b'\r',
                    3..=5 => b'\n',
                    _ => b'a' + rng.below(3) as u8,
                })
                .collect();
            if round % 3 == 0 && len > 8 {
                // plant a clean CRLF-structured head so Ok(Some) is well exercised
                let mut j = 0;
                while j + 6 < len && rng.below(8) != 0 {
                    buf[j] = b'x';
                    buf[j + 1] = b'\r';
                    buf[j + 2] = b'\n';
                    j += 3;
                }
                if j + 4 <= len {
                    buf[j..j + 4].copy_from_slice(b"\r\n\r\n");
                }
            }
            let from = rng.below(len + 1);
            // `from` must be a position where everything before it was already validated,
            // exactly like the real caller; derive it the same way: scan prefix first.
            let prefix_ok = find_head_end_scalar(&buf[..from], 0).map(|r| r.is_none()).unwrap_or(false);
            if !prefix_ok {
                continue;
            }
            let a = find_head_end(&buf, from);
            let b = find_head_end_scalar(&buf, from);
            assert_eq!(a, b, "from={from} buf={:?}", String::from_utf8_lossy(&buf));
        }
    }

    #[test]
    fn scan_name_and_uri_match_table_loops() {
        let mut rng = Rng(0x5EED_5EED_1357_9BDF);
        for _ in 0..400_000 {
            let len = rng.below(60);
            let buf: Vec<u8> = (0..len)
                .map(|_| match rng.below(12) {
                    0 => b':',
                    1 => b' ',
                    2 => b'_',
                    3 => b'.',
                    4 => b'-',
                    5 => 0x80 + rng.below(0x80) as u8,
                    6 => rng.below(0x21) as u8,
                    7 => b"@[`{}/;<>?\\\"()=,"[rng.below(16)],
                    8 => b'0' + rng.below(10) as u8,
                    9 => b"!#$%&'*+^|~"[rng.below(11)],
                    _ => if rng.below(2) == 0 { b'a' + rng.below(26) as u8 } else { b'A' + rng.below(26) as u8 },
                })
                .collect();
            let start = rng.below(len + 1);
            assert_eq!(scan_name(&buf, start), scan_name_scalar(&buf, start), "name {buf:?} from {start}");
            assert_eq!(scan_uri(&buf, start), scan_uri_scalar(&buf, start), "uri {buf:?} from {start}");
        }
        // exhaustive over every single byte value in every lane of a 16-byte block
        for v in 0..=255u8 {
            for lane in 0..20 {
                let mut buf = vec![b'a'; 20];
                buf[lane] = v;
                assert_eq!(scan_name(&buf, 0), scan_name_scalar(&buf, 0), "byte {v:#x} lane {lane}");
                assert_eq!(scan_uri(&buf, 0), scan_uri_scalar(&buf, 0), "byte {v:#x} lane {lane}");
            }
        }
    }

    #[test]
    fn mutated_input_never_panics_and_fragmentation_is_consistent() {
        let mut seed = Vec::new();
        seed.extend_from_slice(GET);
        seed.extend_from_slice(&post(b"hello"));
        seed.extend_from_slice(&chunked(&[b"ab", b"cdef"]));
        let l = lim();
        let mut rng = Rng(0x1234_5678_9ABC_DEF1);
        for _ in 0..30_000 {
            let mut s = seed.clone();
            for _ in 0..1 + rng.below(4) {
                let i = rng.below(s.len());
                match rng.below(4) {
                    0 => s[i] = rng.next() as u8,
                    1 => s[i] = b"\r\n: ,;0aZ"[rng.below(9)],
                    2 => {
                        s.remove(i);
                    }
                    _ => s.insert(i, b"\r\n: ,;0aZ"[rng.below(9)]),
                }
            }
            let whole = run(&s, usize::MAX, l);
            let drib = run(&s, 1 + rng.below(9), l);
            match (whole, drib) {
                (Ok(a), Ok(b)) => assert_eq!(a, b),
                (Err(a), Err(b)) => {
                    assert_eq!(std::mem::discriminant(&a), std::mem::discriminant(&b), "{a:?} vs {b:?}")
                }
                (a, b) => panic!("fragmentation changed the outcome: {a:?} vs {b:?} for {s:?}"),
            }
        }
    }
}
