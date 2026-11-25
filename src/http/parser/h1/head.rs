//! Span-based HTTP/1.x request head parser backed by runtime-dispatched
//! SIMD (`AVX-512BW+VL` / `AVX2` / `SSE2`) and scalar parsers writing 16-byte
//! `(Span, Span)` offsets directly into reusable [`Head`] storage.

use core::hint::assert_unchecked;
use core::mem::MaybeUninit;
#[cfg(target_arch = "x86_64")]
use core::sync::atomic::{AtomicU8, Ordering};

use crate::constants::MAX_REQUEST_HEADERS;
use crate::http::Status as HttpStatus;
use crate::http::parser::semantics;

pub use crate::http::Method;

#[cfg(target_arch = "x86_64")]
mod cpu_tier {
    use super::{AtomicU8, Ordering};

    pub(super) const SSE2: u8 = 1;
    pub(super) const AVX2: u8 = 2;
    pub(super) const AVX512: u8 = 3;

    static TIER: AtomicU8 = AtomicU8::new(0);

    #[inline(always)]
    pub(super) fn get() -> u8 {
        let tier = TIER.load(Ordering::Relaxed);
        if tier != 0 {
            return tier;
        }
        init()
    }

    #[cold]
    #[inline(never)]
    fn init() -> u8 {
        let tier = if std::is_x86_feature_detected!("avx512bw")
            && std::is_x86_feature_detected!("avx512vl")
        {
            AVX512
        } else if std::is_x86_feature_detected!("avx2") {
            AVX2
        } else {
            SSE2
        };
        TIER.store(tier, Ordering::Relaxed);
        tier
    }
}

/// Byte range `[start..end)` inside the raw request head slice.
#[derive(Clone, Copy, Default, Debug, PartialEq, Eq)]
pub struct Span {
    pub start: u32,
    pub end: u32,
}

impl Span {
    #[inline]
    pub fn of(self, raw: &[u8]) -> &[u8] {
        &raw[self.start as usize..self.end as usize]
    }

    /// Borrow `self` from `raw` without bounds checking.
    ///
    /// # Safety
    /// Callers must ensure `self.start <= self.end` and `(self.end as usize) <= raw.len()`.
    #[inline(always)]
    pub(crate) unsafe fn of_unchecked(self, raw: &[u8]) -> &[u8] {
        debug_assert!((self.end as usize) <= raw.len() && self.start <= self.end);
        unsafe { raw.get_unchecked(self.start as usize..self.end as usize) }
    }

    #[inline]
    pub const fn len(self) -> usize {
        (self.end - self.start) as usize
    }

    #[inline]
    pub const fn is_empty(self) -> bool {
        self.start == self.end
    }
}

#[inline(always)]
fn span(a: usize, b: usize) -> Span {
    Span {
        start: a as u32,
        end: b as u32,
    }
}

#[inline(always)]
unsafe fn span_ptrs(base: *const u8, start: *const u8, end: *const u8) -> Span {
    unsafe {
        assert_unchecked(start >= base && end >= start);
        Span {
            start: start.offset_from(base) as u32,
            end: end.offset_from(base) as u32,
        }
    }
}

/// Body framing determined from validated HTTP/1.x headers.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Body {
    None,
    Length(usize),
    Chunked,
}

/// Low-level parse completion/partial status returned by the syntactic head/trailer scanners.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ParseStatus<T> {
    Complete(T),
    Partial,
}

/// Monomorphized header sink writing 16-byte `(Span, Span)` offsets into caller storage.
pub(crate) trait HeaderSink<'buf> {
    fn capacity(&self) -> usize;

    unsafe fn write_header(
        &mut self,
        index: usize,
        base: *const u8,
        name_start: *const u8,
        name_end: *const u8,
        val_start: *const u8,
        val_end: *const u8,
    );
}

pub(crate) struct SpanHeaderSink {
    pub(crate) ptr: *mut MaybeUninit<(Span, Span)>,
    pub(crate) cap: usize,
}

impl<'buf> HeaderSink<'buf> for SpanHeaderSink {
    #[inline(always)]
    fn capacity(&self) -> usize {
        self.cap
    }

    #[inline(always)]
    unsafe fn write_header(
        &mut self,
        index: usize,
        base: *const u8,
        name_start: *const u8,
        name_end: *const u8,
        val_start: *const u8,
        val_end: *const u8,
    ) {
        unsafe {
            assert_unchecked(index < self.cap);
            self.ptr.add(index).write(MaybeUninit::new((
                span_ptrs(base, name_start, name_end),
                span_ptrs(base, val_start, val_end),
            )));
        }
    }
}

/// Raw offsets returned by the SIMD/scalar request-head scanner.
pub(crate) struct ParsedHead {
    pub method: Method,
    pub path_start: usize,
    pub path_end: usize,
    pub version: u8,
    pub headers_len: usize,
    pub consumed: usize,
}

// RFC 9110 tchar lookup table (1 = valid tchar, 0 = invalid)
pub(crate) const TOKEN_MAP: [u8; 256] = const {
    let mut map = [0u8; 256];
    let mut i = 0usize;
    while i < 256 {
        let b = i as u8;
        if matches!(
            b,
            b'!' | b'#' | b'$' | b'%' | b'&' | b'\'' | b'*' | b'+' | b'-' | b'.' | b'^' | b'_'
                | b'`' | b'|' | b'~' | b'0'..=b'9' | b'A'..=b'Z' | b'a'..=b'z'
        ) {
            map[i] = 1;
        }
        i += 1;
    }
    map
};

// Fast request-target byte table (`0x21..=0x7E`); semantic URI rules are enforced after scanning.
pub(crate) const URI_MAP: [u8; 256] = const {
    let mut map = [0u8; 256];
    let mut i = 0x21usize;
    while i <= 0x7E {
        map[i] = 1;
        i += 1;
    }
    map
};

// RFC 9110 field-content bytes: HTAB (0x09), SP (0x20), VCHAR (0x21..=0x7E), obs-text (0x80..=0xFF)
pub(crate) const HEADER_VALUE_MAP: [u8; 256] = const {
    let mut map = [0u8; 256];
    map[0x09] = 1;
    let mut i = 0x20usize;
    while i <= 0x7E {
        map[i] = 1;
        i += 1;
    }
    let mut j = 0x80usize;
    while j <= 0xFF {
        map[j] = 1;
        j += 1;
    }
    map
};

// Origin-form target character classifier:
//   0 = plain valid VCHAR, 1 = '?', 2 = '%', 3 = forbidden (# < > " { } | \ ^ ` or CTL/non-ASCII)
const ORIGIN_CLASS: [u8; 256] = const {
    let mut t = [0u8; 256];
    let mut i = 0usize;
    while i < 256 {
        let b = i as u8;
        if !semantics::is_valid_uri_byte(b) {
            t[i] = 3;
        } else if b == b'?' {
            t[i] = 1;
        } else if b == b'%' {
            t[i] = 2;
        }
        i += 1;
    }
    t
};

/// Fast-path HTTP method parser using little-endian unaligned word loads (with scalar fallback).
/// Returns `(Method, ptr_after_space)`.
#[inline(always)]
pub(crate) unsafe fn parse_method(
    mut ptr: *const u8,
    end: *const u8,
) -> Result<Option<(Method, *const u8)>, HttpStatus> {
    #[cfg(target_endian = "little")]
    {
        let rem = unsafe { end.offset_from(ptr) as usize };
        if rem >= 8 {
            let word = unsafe { (ptr as *const u64).read_unaligned() };
            let w4 = word as u32;
            if w4 == u32::from_le_bytes(*b"GET ") {
                return Ok(Some((Method::Get, unsafe { ptr.add(4) })));
            }
            if (word & 0x00FF_FFFF_FFFF) == (u64::from_le_bytes(*b"POST \0\0\0") & 0x00FF_FFFF_FFFF)
            {
                return Ok(Some((Method::Post, unsafe { ptr.add(5) })));
            }
            if w4 == u32::from_le_bytes(*b"PUT ") {
                return Ok(Some((Method::Put, unsafe { ptr.add(4) })));
            }
            if (word & 0x00FF_FFFF_FFFF) == (u64::from_le_bytes(*b"HEAD \0\0\0") & 0x00FF_FFFF_FFFF)
            {
                return Ok(Some((Method::Head, unsafe { ptr.add(5) })));
            }
            if (word & 0x00FF_FFFF_FFFF_FFFF)
                == (u64::from_le_bytes(*b"DELETE \0") & 0x00FF_FFFF_FFFF_FFFF)
            {
                return Ok(Some((Method::Delete, unsafe { ptr.add(7) })));
            }
            if word == u64::from_le_bytes(*b"OPTIONS ") {
                return Ok(Some((Method::Options, unsafe { ptr.add(8) })));
            }
            if (word & 0x0000_FFFF_FFFF_FFFF)
                == (u64::from_le_bytes(*b"PATCH \0\0") & 0x0000_FFFF_FFFF_FFFF)
            {
                return Ok(Some((Method::Patch, unsafe { ptr.add(6) })));
            }
            if word == u64::from_le_bytes(*b"CONNECT ") {
                return Ok(Some((Method::Connect, unsafe { ptr.add(8) })));
            }
            if (word & 0x0000_FFFF_FFFF_FFFF)
                == (u64::from_le_bytes(*b"TRACE \0\0") & 0x0000_FFFF_FFFF_FFFF)
            {
                return Ok(Some((Method::Trace, unsafe { ptr.add(6) })));
            }
        }
    }

    let start = ptr;
    while ptr < end {
        let b = unsafe { *ptr };
        if b == b' ' {
            if ptr == start {
                return Err(HttpStatus::BadRequest);
            }
            let slice =
                unsafe { core::slice::from_raw_parts(start, ptr.offset_from(start) as usize) };
            let method = Method::from_bytes(slice).ok_or(HttpStatus::NotImplemented)?;
            return Ok(Some((method, unsafe { ptr.add(1) })));
        }
        if TOKEN_MAP[b as usize] == 0 {
            return Err(HttpStatus::BadRequest);
        }
        ptr = unsafe { ptr.add(1) };
    }
    Ok(None)
}

#[inline]
fn is_valid_version_prefix(s: &[u8]) -> bool {
    const PREFIX: &[u8] = b"HTTP/";
    let n = s.len();
    for i in 0..n.min(5) {
        if s[i] != PREFIX[i] {
            return false;
        }
    }
    if n <= 5 {
        return true;
    }
    if !s[5].is_ascii_digit() {
        return false;
    }
    if n == 6 {
        return true;
    }
    if (s[5] == b'2' || s[5] == b'3') && s[6] == b'\r' {
        return n == 7 || (n == 8 && s[7] == b'\n');
    }
    if s[6] != b'.' {
        return false;
    }
    if n == 7 {
        return true;
    }
    if !s[7].is_ascii_digit() {
        return false;
    }
    if n == 8 {
        return true;
    }
    s[8] == b'\r'
}

/// Strict HTTP version parser (`"HTTP/1.1\r\n"` or `"HTTP/1.0\r\n"`).
#[inline(always)]
pub(crate) unsafe fn parse_version(
    ptr: *const u8,
    end: *const u8,
) -> Result<Option<(u8, *const u8)>, HttpStatus> {
    const CRLF: u16 = u16::from_ne_bytes(*b"\r\n");
    let rem = unsafe { end.offset_from(ptr) as usize };
    if rem >= 10 {
        let v8 = unsafe { (ptr as *const u64).read_unaligned() };
        let crlf = unsafe { (ptr.add(8) as *const u16).read_unaligned() };
        if crlf == CRLF {
            if v8 == u64::from_ne_bytes(*b"HTTP/1.1") {
                return Ok(Some((1, unsafe { ptr.add(10) })));
            }
            if v8 == u64::from_ne_bytes(*b"HTTP/1.0") {
                return Ok(Some((0, unsafe { ptr.add(10) })));
            }
        }
    }
    unsafe { parse_version_cold(ptr, rem) }
}

#[cold]
#[inline(never)]
unsafe fn parse_version_cold(
    ptr: *const u8,
    rem: usize,
) -> Result<Option<(u8, *const u8)>, HttpStatus> {
    let s = unsafe { core::slice::from_raw_parts(ptr, rem) };

    if matches!(
        s.get(..10),
        Some([
            b'H',
            b'T',
            b'T',
            b'P',
            b'/',
            b'0'..=b'9',
            b'.',
            b'0'..=b'9',
            b'\r',
            b'\n'
        ])
    ) || matches!(s.get(..8), Some(b"HTTP/2\r\n" | b"HTTP/3\r\n"))
    {
        return Err(HttpStatus::HttpVersionNotSupported);
    }

    if matches!(s.get(..8), Some(b"HTTP/1.1" | b"HTTP/1.0")) && rem >= 9 && s[8] != b'\r' {
        return Err(HttpStatus::BadRequest);
    }
    if matches!(s.get(..9), Some(b"HTTP/1.1\r" | b"HTTP/1.0\r")) && rem >= 10 && s[9] != b'\n' {
        return Err(HttpStatus::BadRequest);
    }

    if rem < 10 && is_valid_version_prefix(s) {
        return Ok(None);
    }

    Err(HttpStatus::BadRequest)
}

/// Parses standalone HTTP headers (chunked trailers) into 16-byte `(Span, Span)` offsets
/// with strict `\r\n` enforcement.
#[inline]
pub(crate) unsafe fn parse_headers_only(
    buf: &[u8],
    headers: &mut [MaybeUninit<(Span, Span)>],
) -> Result<ParseStatus<(usize, usize)>, HttpStatus> {
    let mut sink = SpanHeaderSink {
        ptr: headers.as_mut_ptr(),
        cap: headers.len(),
    };
    #[cfg(target_arch = "x86_64")]
    if buf.len() >= 32 && cpu_tier::get() == cpu_tier::AVX512 {
        return unsafe { super::simd::parse_headers_avx512(buf, &mut sink) };
    }
    let base = buf.as_ptr();
    let end = unsafe { base.add(buf.len()) };
    match unsafe { parse_headers_scalar(base, base, end, &mut sink)? } {
        ParseStatus::Complete((count, next_ptr)) => {
            let consumed = unsafe { next_ptr.offset_from(base) as usize };
            Ok(ParseStatus::Complete((count, consumed)))
        }
        ParseStatus::Partial => Ok(ParseStatus::Partial),
    }
}

/// Strict `\r\n` scalar header loop generic over [`HeaderSink`].
#[inline]
unsafe fn parse_headers_scalar<'buf, S: HeaderSink<'buf>>(
    base: *const u8,
    mut ptr: *const u8,
    end: *const u8,
    sink: &mut S,
) -> Result<ParseStatus<(usize, *const u8)>, HttpStatus> {
    const CRLF: u16 = u16::from_ne_bytes(*b"\r\n");
    let cap = sink.capacity();
    let mut count = 0usize;

    loop {
        if ptr >= end {
            return Ok(ParseStatus::Partial);
        }
        let b0 = unsafe { *ptr };
        if b0 == b'\r' {
            if unsafe { ptr.add(1) } >= end {
                return Ok(ParseStatus::Partial);
            }
            if unsafe { *ptr.add(1) } == b'\n' {
                ptr = unsafe { ptr.add(2) };
                break;
            }
            return Err(HttpStatus::BadRequest);
        }

        let name_start = ptr;
        while ptr < end {
            let b = unsafe { *ptr };
            if TOKEN_MAP[b as usize] == 0 {
                if b == b':' {
                    break;
                }
                return Err(HttpStatus::BadRequest);
            }
            ptr = unsafe { ptr.add(1) };
        }
        if ptr == end {
            return Ok(ParseStatus::Partial);
        }
        if ptr == name_start {
            return Err(HttpStatus::BadRequest);
        }
        let name_end = ptr;
        ptr = unsafe { ptr.add(1) }; // skip ':'

        // Skip leading OWS
        while ptr < end {
            let b = unsafe { *ptr };
            if b != b' ' && b != b'\t' {
                break;
            }
            ptr = unsafe { ptr.add(1) };
        }

        let val_start = ptr;
        while ptr < end {
            let b = unsafe { *ptr };
            if HEADER_VALUE_MAP[b as usize] == 0 {
                if b == b'\r' {
                    break;
                }
                return Err(HttpStatus::BadRequest);
            }
            ptr = unsafe { ptr.add(1) };
        }
        if ptr == end {
            return Ok(ParseStatus::Partial);
        }

        let mut val_end = ptr;
        while val_end > val_start {
            let prev = unsafe { *val_end.sub(1) };
            if prev != b' ' && prev != b'\t' {
                break;
            }
            val_end = unsafe { val_end.sub(1) };
        }

        // Strictly require CRLF
        if unsafe { end.offset_from(ptr) } >= 2
            && unsafe { (ptr as *const u16).read_unaligned() } == CRLF
        {
            ptr = unsafe { ptr.add(2) };
        } else if unsafe { *ptr } == b'\r' {
            if unsafe { ptr.add(1) } >= end {
                return Ok(ParseStatus::Partial);
            }
            return Err(HttpStatus::BadRequest);
        } else {
            return Err(HttpStatus::BadRequest);
        }

        if count >= cap {
            return Err(HttpStatus::RequestHeaderFieldsTooLarge);
        }
        unsafe { sink.write_header(count, base, name_start, name_end, val_start, val_end) };
        count += 1;
    }

    Ok(ParseStatus::Complete((count, ptr)))
}

/// Core scalar request parser (fallback on non-`x86_64` and `< 16`-byte inputs).
#[inline]
unsafe fn parse_request_scalar<'buf, S: HeaderSink<'buf>>(
    buf: &'buf [u8],
    sink: &mut S,
) -> Result<ParseStatus<ParsedHead>, HttpStatus> {
    const SLASH_SPACE: u16 = u16::from_ne_bytes(*b"/ ");
    let base = buf.as_ptr();
    let end = unsafe { base.add(buf.len()) };

    let (method, mut ptr) = match unsafe { parse_method(base, end)? } {
        Some(v) => v,
        None => return Ok(ParseStatus::Partial),
    };

    let path_start_off: usize;
    let path_end_off: usize;
    if unsafe { end.offset_from(ptr) } >= 2
        && unsafe { (ptr as *const u16).read_unaligned() } == SLASH_SPACE
    {
        path_start_off = unsafe { ptr.offset_from(base) as usize };
        path_end_off = path_start_off + 1;
        ptr = unsafe { ptr.add(2) };
    } else {
        let path_start = ptr;
        while ptr < end {
            let b = unsafe { *ptr };
            if URI_MAP[b as usize] == 0 {
                if b == b' ' {
                    break;
                }
                return Err(HttpStatus::BadRequest);
            }
            ptr = unsafe { ptr.add(1) };
        }
        if ptr == end {
            return Ok(ParseStatus::Partial);
        }
        if ptr == path_start {
            return Err(HttpStatus::BadRequest);
        }
        path_start_off = unsafe { path_start.offset_from(base) as usize };
        path_end_off = unsafe { ptr.offset_from(base) as usize };
        ptr = unsafe { ptr.add(1) }; // skip ' '
    }

    let (version, next_ptr) = match unsafe { parse_version(ptr, end)? } {
        Some(v) => v,
        None => return Ok(ParseStatus::Partial),
    };

    match unsafe { parse_headers_scalar(base, next_ptr, end, sink)? } {
        ParseStatus::Complete((headers_len, final_ptr)) => {
            let consumed = unsafe { final_ptr.offset_from(base) as usize };
            Ok(ParseStatus::Complete(ParsedHead {
                method,
                path_start: path_start_off,
                path_end: path_end_off,
                version,
                headers_len,
                consumed,
            }))
        }
        ParseStatus::Partial => Ok(ParseStatus::Partial),
    }
}

#[inline(always)]
unsafe fn dispatch_parse<'buf, S: HeaderSink<'buf>>(
    buf: &'buf [u8],
    sink: &mut S,
) -> Result<ParseStatus<ParsedHead>, HttpStatus> {
    #[cfg(target_arch = "x86_64")]
    {
        if buf.len() >= 32 {
            return match cpu_tier::get() {
                cpu_tier::AVX512 => unsafe { super::simd::parse_request_avx512(buf, sink) },
                cpu_tier::AVX2 => unsafe { super::simd::parse_request_avx2(buf, sink) },
                _ => unsafe { super::simd::parse_request_sse2(buf, sink) },
            };
        }
        if buf.len() >= 16 {
            return unsafe { super::simd::parse_request_sse2(buf, sink) };
        }
    }
    unsafe { parse_request_scalar(buf, sink) }
}

/// Find the end of the HTTP head (`\r\n\r\n`) starting from byte `from`, rejecting bare `\n`.
///
/// Used only on fragmented heads (`scanned > 0`) and on the cold error classification path.
#[allow(clippy::result_unit_err)]
pub(crate) fn find_head_end(data: &[u8], from: usize) -> Result<Option<usize>, ()> {
    const LO: u64 = 0x0101_0101_0101_0101;
    const HI: u64 = 0x8080_8080_8080_8080;

    let mut i = from;
    while i < data.len() {
        let mut lf = None;
        let mut j = i;
        let (chunks, rem) = data[i..].as_chunks::<8>();
        for chunk in chunks {
            let v = u64::from_le_bytes(*chunk);
            let x = v ^ 0x0A0A_0A0A_0A0A_0A0A;
            let m = x.wrapping_sub(LO) & !x & HI;
            if m != 0 {
                lf = Some(j + (m.trailing_zeros() >> 3) as usize);
                break;
            }
            j += 8;
        }
        let lf = match lf {
            Some(pos) => pos,
            None => match rem.iter().position(|&b| b == b'\n') {
                Some(p) => j + p,
                None => return Ok(None),
            },
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

/// Parsed request head. Lives inside [`super::Conn`] and is reused for every request:
/// no per-request initialization of the header table, no heap allocation.
pub struct Head {
    pub method: Method,
    /// HTTP/1.`minor` (`0` or `1`)
    pub minor: u8,
    pub target: Span,
    pub path: Span,
    pub query: Span,
    pub keep_alive: bool,
    pub expect_100_continue: bool,
    pub body: Body,
    pub host: Option<Span>,
    len: usize,
    hdrs: [MaybeUninit<(Span, Span)>; MAX_REQUEST_HEADERS],
}

impl Default for Head {
    #[inline]
    fn default() -> Self {
        Head::new()
    }
}

impl Head {
    /// Creates an empty [`Head`] without zero-initializing the backing header span table.
    #[inline]
    pub const fn new() -> Head {
        Head {
            method: Method::Get,
            minor: 1,
            target: Span { start: 0, end: 0 },
            path: Span { start: 0, end: 0 },
            query: Span { start: 0, end: 0 },
            keep_alive: true,
            expect_100_continue: false,
            body: Body::None,
            host: None,
            len: 0,
            hdrs: [const { MaybeUninit::uninit() }; MAX_REQUEST_HEADERS],
        }
    }

    #[inline]
    pub fn header_count(&self) -> usize {
        self.len
    }

    #[inline]
    pub(crate) fn header_spans(&self) -> &[(Span, Span)] {
        // SAFETY: `self.hdrs[..self.len]` are initialized by `SpanHeaderSink::write_header`
        // and `self.len <= MAX_REQUEST_HEADERS`.
        unsafe { std::slice::from_raw_parts(self.hdrs.as_ptr().cast(), self.len) }
    }

    /// Parse a complete HTTP/1.x request head.
    ///
    /// `b` must be exactly the head, including the final `\r\n\r\n`.
    #[inline]
    pub fn parse(&mut self, b: &[u8]) -> Result<(), HttpStatus> {
        match self.parse_inner::<true>(b)? {
            Some(_) => Ok(()),
            None => Err(HttpStatus::BadRequest),
        }
    }

    /// Parse a complete HTTP/1.x request head from the leading prefix of `b`.
    ///
    /// Stops at the terminating `\r\n\r\n` and returns the head length in bytes,
    /// leaving any trailing body or pipelined bytes untouched.
    #[inline]
    pub fn parse_prefix(&mut self, b: &[u8]) -> Result<usize, HttpStatus> {
        match self.parse_inner::<false>(b)? {
            Some(len) => Ok(len),
            None => Err(HttpStatus::BadRequest),
        }
    }

    /// Parse a request head from the leading prefix of `b`, returning `Ok(None)` when
    /// `b` is a valid incomplete prefix, `Ok(Some(consumed))` on completion, or `Err(HttpStatus)`
    /// on syntax/limit violations.
    #[inline]
    pub(crate) fn parse_prefix_status(&mut self, b: &[u8]) -> Result<Option<usize>, HttpStatus> {
        self.parse_inner::<false>(b)
    }

    #[inline]
    fn parse_inner<const EXACT: bool>(&mut self, b: &[u8]) -> Result<Option<usize>, HttpStatus> {
        if b.len() > u32::MAX as usize {
            return Err(HttpStatus::RequestHeaderFieldsTooLarge);
        }
        let mut sink = SpanHeaderSink {
            ptr: self.hdrs.as_mut_ptr(),
            cap: MAX_REQUEST_HEADERS,
        };
        let parsed = match unsafe { dispatch_parse(b, &mut sink)? } {
            ParseStatus::Complete(p) => p,
            ParseStatus::Partial => return Ok(None),
        };
        self.finalize_parsed::<EXACT>(b, parsed).map(Some)
    }

    #[inline]
    fn finalize_parsed<const EXACT: bool>(
        &mut self,
        b: &[u8],
        parsed: ParsedHead,
    ) -> Result<usize, HttpStatus> {
        if EXACT && parsed.consumed != b.len() {
            return Err(HttpStatus::BadRequest);
        }

        // SAFETY: `dispatch_parse` guarantees `parsed.headers_len <= MAX_REQUEST_HEADERS` and
        // all `self.hdrs[..parsed.headers_len]` slots were initialized.
        self.len = parsed.headers_len;
        let minor = parsed.version;
        self.minor = minor;
        self.host = None;
        self.expect_100_continue = false;

        self.method = parsed.method;

        let (t0, t1) = (parsed.path_start, parsed.path_end);
        let (path, query) = if b[t0] == b'/' {
            validate_origin_target(b, t0, t1)?
        } else {
            validate_non_origin_target(b, t0, t1, self.method)?
        };
        self.target = span(t0, t1);
        self.path = path;
        self.query = query;

        // Only what framing and RFC 9112 semantics need is interpreted eagerly; switch on length first.
        let mut host: Option<Span> = None;
        let mut expect_100_continue = false;
        let mut content_length: Option<usize> = None;
        let mut saw_te = false;
        let mut chunked = false;
        let (mut close, mut ka) = (false, false);

        // SAFETY: `dispatch_parse` guarantees every initialized slot in `self.header_spans()` holds
        // in-bounds `Span`s into `b`.
        for &(n_span, v_span) in self.header_spans() {
            match n_span.len() {
                4 if unsafe { n_span.of_unchecked(b) }.eq_ignore_ascii_case(b"host") => {
                    let val = unsafe { v_span.of_unchecked(b) };
                    if host.is_some() || !semantics::is_valid_host_bytes(val) {
                        return Err(HttpStatus::BadRequest);
                    }
                    host = Some(v_span);
                }
                6 if unsafe { n_span.of_unchecked(b) }.eq_ignore_ascii_case(b"expect") => {
                    let val = unsafe { v_span.of_unchecked(b) };
                    parse_expect(minor, val, &mut expect_100_continue)?;
                }
                10 if unsafe { n_span.of_unchecked(b) }.eq_ignore_ascii_case(b"connection") => {
                    let val = unsafe { v_span.of_unchecked(b) };
                    if val.eq_ignore_ascii_case(b"keep-alive") {
                        ka = true;
                    } else if val.eq_ignore_ascii_case(b"close") {
                        close = true;
                    } else {
                        parse_connection_slow(val, &mut close, &mut ka);
                    }
                }
                14 if unsafe { n_span.of_unchecked(b) }.eq_ignore_ascii_case(b"content-length") => {
                    let val = unsafe { v_span.of_unchecked(b) };
                    let n = parse_decimal(val).ok_or(HttpStatus::BadRequest)?;
                    if content_length.is_some_and(|prev| prev != n) {
                        return Err(HttpStatus::BadRequest);
                    }
                    content_length = Some(n);
                }
                17 if unsafe { n_span.of_unchecked(b) }
                    .eq_ignore_ascii_case(b"transfer-encoding") =>
                {
                    let val = unsafe { v_span.of_unchecked(b) };
                    if minor == 1 && !saw_te && val.eq_ignore_ascii_case(b"chunked") {
                        chunked = true;
                        saw_te = true;
                    } else {
                        parse_transfer_encoding_slow(minor, val, &mut saw_te, &mut chunked)?;
                    }
                }
                _ => {}
            }
        }

        // ── request-smuggling guards and framing ──
        if saw_te && (content_length.is_some() || minor == 0) {
            return Err(HttpStatus::BadRequest);
        }
        if minor == 1 && host.is_none() {
            return Err(HttpStatus::BadRequest);
        }
        self.host = host;
        self.expect_100_continue = expect_100_continue;
        self.keep_alive = if minor == 1 { !close } else { ka && !close };
        self.body = if chunked {
            Body::Chunked
        } else if let Some(n) = content_length {
            Body::Length(n)
        } else {
            Body::None
        };

        Ok(parsed.consumed)
    }

    #[cfg(test)]
    pub(crate) fn parse_with_backend_for_test(
        &mut self,
        b: &[u8],
        backend: TestBackend,
    ) -> Result<Option<usize>, HttpStatus> {
        if b.len() > u32::MAX as usize {
            return Err(HttpStatus::RequestHeaderFieldsTooLarge);
        }
        let mut sink = SpanHeaderSink {
            ptr: self.hdrs.as_mut_ptr(),
            cap: MAX_REQUEST_HEADERS,
        };
        let res = match backend {
            TestBackend::Scalar => unsafe { parse_request_scalar(b, &mut sink) },
            #[cfg(target_arch = "x86_64")]
            TestBackend::Sse2 => unsafe { super::simd::parse_request_sse2(b, &mut sink) },
            #[cfg(target_arch = "x86_64")]
            TestBackend::Avx2 => unsafe { super::simd::parse_request_avx2(b, &mut sink) },
            #[cfg(target_arch = "x86_64")]
            TestBackend::Avx512 => unsafe { super::simd::parse_request_avx512(b, &mut sink) },
        };
        let parsed = match res? {
            ParseStatus::Complete(p) => p,
            ParseStatus::Partial => return Ok(None),
        };
        self.finalize_parsed::<false>(b, parsed).map(Some)
    }
}

#[cfg(test)]
#[derive(Clone, Copy, Debug)]
pub(crate) enum TestBackend {
    Scalar,
    #[cfg(target_arch = "x86_64")]
    Sse2,
    #[cfg(target_arch = "x86_64")]
    Avx2,
    #[cfg(target_arch = "x86_64")]
    Avx512,
}

#[inline]
fn validate_origin_target(b: &[u8], t0: usize, t1: usize) -> Result<(Span, Span), HttpStatus> {
    debug_assert!(t0 < t1 && t1 <= b.len() && b[t0] == b'/');
    if t1 - t0 == 1 {
        return Ok((span(t0, t1), Span::default()));
    }
    // SAFETY: Caller verified `b[t0] == b'/'` and `t0 < t1 <= b.len()`.
    let target = unsafe { b.get_unchecked(t0..t1) };
    let mut i = 1usize;
    let mut question_pos: Option<usize> = None;
    while i < target.len() {
        match ORIGIN_CLASS[target[i] as usize] {
            0 => i += 1,
            1 => {
                if question_pos.is_none() {
                    question_pos = Some(i);
                }
                i += 1;
            }
            2 => {
                if i + 2 >= target.len()
                    || !target[i + 1].is_ascii_hexdigit()
                    || !target[i + 2].is_ascii_hexdigit()
                {
                    return Err(HttpStatus::BadRequest);
                }
                i += 3;
            }
            _ => return Err(HttpStatus::BadRequest),
        }
    }
    Ok(match question_pos {
        Some(q) => (span(t0, t0 + q), span(t0 + q + 1, t1)),
        None => (span(t0, t1), Span::default()),
    })
}

/// Validate non-origin RFC 9112 §3.2 `request-target` forms (`*`, `authority-form`, `absolute-form`),
/// returning `(path_span, query_span)`.
#[cold]
#[inline(never)]
fn validate_non_origin_target(
    b: &[u8],
    t0: usize,
    t1: usize,
    method: Method,
) -> Result<(Span, Span), HttpStatus> {
    let target = &b[t0..t1];

    if target == b"*" {
        return if method == Method::Options {
            Ok((span(t0, t1), Span::default()))
        } else {
            Err(HttpStatus::BadRequest)
        };
    }

    let mut question_pos: Option<usize> = None;
    let mut scheme_sep: Option<usize> = None;
    let mut i = 0usize;

    while i < target.len() {
        let c = target[i];
        if !semantics::is_valid_uri_byte(c) {
            return Err(HttpStatus::BadRequest);
        }
        if c == b'%' {
            if i + 2 >= target.len()
                || !target[i + 1].is_ascii_hexdigit()
                || !target[i + 2].is_ascii_hexdigit()
            {
                return Err(HttpStatus::BadRequest);
            }
            i += 3;
            continue;
        }
        if c == b'?' && question_pos.is_none() {
            question_pos = Some(i);
        } else if c == b':'
            && scheme_sep.is_none()
            && question_pos.is_none()
            && i + 2 < target.len()
            && target[i + 1] == b'/'
            && target[i + 2] == b'/'
        {
            scheme_sep = Some(i);
        }
        i += 1;
    }

    if method == Method::Connect {
        return if scheme_sep.is_none()
            && question_pos.is_none()
            && semantics::is_valid_authority_bytes(target)
        {
            Ok((span(t0, t1), Span::default()))
        } else {
            Err(HttpStatus::BadRequest)
        };
    }

    if let Some(colon) = scheme_sep {
        if !semantics::is_valid_scheme(&target[..colon]) {
            return Err(HttpStatus::BadRequest);
        }

        let after_scheme_start = colon + 3;
        let query_end = question_pos.unwrap_or(target.len());
        if after_scheme_start >= query_end {
            return Err(HttpStatus::BadRequest);
        }

        let after_scheme = &target[after_scheme_start..query_end];
        let (authority, path_span) = match after_scheme.iter().position(|&c| c == b'/') {
            Some(slash) => (
                &after_scheme[..slash],
                span(t0 + after_scheme_start + slash, t0 + query_end),
            ),
            None => (
                after_scheme,
                // Points to the first `/` in `://`, yielding `b"/"` without allocation.
                span(t0 + colon + 1, t0 + colon + 2),
            ),
        };

        if !semantics::is_valid_host_bytes(authority) {
            return Err(HttpStatus::BadRequest);
        }

        let query_span = match question_pos {
            Some(q) => span(t0 + q + 1, t1),
            None => Span::default(),
        };
        return Ok((path_span, query_span));
    }

    Err(HttpStatus::BadRequest)
}

#[cold]
#[inline(never)]
fn parse_expect(minor: u8, val: &[u8], expect_100_continue: &mut bool) -> Result<(), HttpStatus> {
    if val.eq_ignore_ascii_case(b"100-continue") {
        if minor == 1 {
            *expect_100_continue = true;
        }
        Ok(())
    } else {
        Err(HttpStatus::ExpectationFailed)
    }
}

#[cold]
#[inline(never)]
fn parse_connection_slow(val: &[u8], close: &mut bool, ka: &mut bool) {
    for tok in val.split(|&c| c == b',') {
        let t = trim_ows(tok);
        if t.eq_ignore_ascii_case(b"close") {
            *close = true;
        } else if t.eq_ignore_ascii_case(b"keep-alive") {
            *ka = true;
        }
    }
}

#[cold]
#[inline(never)]
fn parse_transfer_encoding_slow(
    minor: u8,
    val: &[u8],
    saw_te: &mut bool,
    chunked: &mut bool,
) -> Result<(), HttpStatus> {
    if minor == 0 || val.is_empty() {
        return Err(HttpStatus::BadRequest);
    }
    for part in val.split(|&c| c == b',') {
        let coding = trim_ows(part);
        if coding.is_empty() || !coding.iter().all(|&c| TOKEN_MAP[c as usize] != 0) {
            return Err(HttpStatus::BadRequest);
        }
        if *chunked {
            return Err(HttpStatus::BadRequest);
        }
        if coding.eq_ignore_ascii_case(b"chunked") {
            *chunked = true;
        } else if !coding.eq_ignore_ascii_case(b"identity") {
            return Err(HttpStatus::NotImplemented);
        }
    }
    *saw_te = true;
    Ok(())
}

#[inline]
fn parse_decimal(v: &[u8]) -> Option<usize> {
    if v.is_empty() {
        return None;
    }
    let mut n: usize = 0;
    for &c in v {
        let d = c.wrapping_sub(b'0');
        if d > 9 {
            return None;
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
