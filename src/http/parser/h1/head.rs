//! Span-based HTTP/1.x request head parser.

use std::mem::MaybeUninit;

use crate::http::parser::{Error, MAX_HEADERS, semantics};

use super::scan::{TOKEN, at, scan_name, scan_origin_target, scan_uri, skip_value};

/// Byte range inside the raw request head slice.
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
}

#[inline(always)]
fn span(a: usize, b: usize) -> Span {
    Span {
        start: a as u32,
        end: b as u32,
    }
}

/// Wire HTTP request method with zero-allocation matching for standard methods
/// and [`Method::Other`] spans for extension/custom tokens.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Method {
    Get,
    Head,
    Post,
    Put,
    Delete,
    Patch,
    Options,
    Trace,
    Connect,
    Other(Span),
}

/// Body framing determined from validated HTTP/1.x headers.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Body {
    None,
    Length(usize),
    Chunked,
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
    hdrs: [MaybeUninit<(Span, Span)>; MAX_HEADERS],
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
            hdrs: [const { MaybeUninit::uninit() }; MAX_HEADERS],
        }
    }

    #[inline]
    pub fn header_count(&self) -> usize {
        self.len
    }

    #[inline]
    pub(crate) fn header_spans(&self) -> &[(Span, Span)] {
        // SAFETY: `self.hdrs[..self.len]` are always initialized via `MaybeUninit::write`
        // before `self.len` is incremented in `Head::parse`, and `self.len <= MAX_HEADERS`.
        unsafe { std::slice::from_raw_parts(self.hdrs.as_ptr().cast(), self.len) }
    }

    /// Parse a complete HTTP/1.x request head.
    ///
    /// `b` must be exactly the head, including the final `\r\n\r\n`.
    #[inline]
    pub fn parse(&mut self, b: &[u8]) -> Result<(), Error> {
        self.parse_inner::<true>(b).map(|_| ())
    }

    /// Parse a complete HTTP/1.x request head from the leading prefix of `b`.
    ///
    /// Stops at the terminating `\r\n\r\n` and returns the head length in bytes,
    /// leaving any trailing body or pipelined bytes untouched.
    #[inline]
    pub fn parse_prefix(&mut self, b: &[u8]) -> Result<usize, Error> {
        self.parse_inner::<false>(b)
    }

    #[inline]
    fn parse_inner<const EXACT: bool>(&mut self, b: &[u8]) -> Result<usize, Error> {
        if b.len() > u32::MAX as usize {
            return Err(Error::HeaderTooLarge);
        }
        self.len = 0;
        self.host = None;
        self.expect_100_continue = false;

        // ── request line: METHOD SP target SP HTTP/1.x CRLF ──
        let (method, mut i) = parse_method(b)?;
        self.method = method;

        let t0 = i;
        if at(b, t0) == b'/' {
            let (t1, question_pos) = scan_origin_target(b, t0)?;
            let (path, query) = match question_pos {
                Some(q) => (span(t0, q), span(q + 1, t1)),
                None => (span(t0, t1), Span { start: 0, end: 0 }),
            };
            self.target = span(t0, t1);
            self.path = path;
            self.query = query;
            i = t1 + 1;
        } else {
            let (t1, path, query) = parse_non_origin_target(b, t0, method)?;
            self.target = span(t0, t1);
            self.path = path;
            self.query = query;
            i = t1 + 1;
        }

        match b.get(i..i + 10) {
            Some(b"HTTP/1.1\r\n") => self.minor = 1,
            Some(b"HTTP/1.0\r\n") => self.minor = 0,
            _ => return Err(reject_version(b, i)),
        }
        i += 10;

        // ── header fields ──
        let mut content_length: Option<usize> = None;
        let mut saw_te = false;
        let mut chunked = false;
        let (mut close, mut ka) = (false, false);

        loop {
            if at(b, i) == b'\r' {
                // Blank line: terminates the head.
                if at(b, i + 1) != b'\n' || (EXACT && i + 2 != b.len()) {
                    return Err(Error::Malformed);
                }
                i += 2;
                break;
            }

            // name: token ':' (a leading SP/HTAB = obs-fold, SP before ':' -> both fail here)
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

            // Only what framing and RFC 9112 semantics need is interpreted eagerly; switch on length first.
            let name = &b[n0..n1];
            let val = &b[v0..v1];
            match name.len() {
                4 if name.eq_ignore_ascii_case(b"host") => {
                    if self.host.is_some() || !semantics::is_valid_host_bytes(val) {
                        return Err(Error::Malformed);
                    }
                    self.host = Some(span(v0, v1));
                }
                6 if name.eq_ignore_ascii_case(b"expect") => {
                    parse_expect(self.minor, val, &mut self.expect_100_continue)?;
                }
                10 if name.eq_ignore_ascii_case(b"connection") => {
                    if val.eq_ignore_ascii_case(b"keep-alive") {
                        ka = true;
                    } else if val.eq_ignore_ascii_case(b"close") {
                        close = true;
                    } else {
                        parse_connection_slow(val, &mut close, &mut ka);
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
                    if self.minor == 1 && !saw_te && val.eq_ignore_ascii_case(b"chunked") {
                        chunked = true;
                        saw_te = true;
                    } else {
                        parse_transfer_encoding_slow(self.minor, val, &mut saw_te, &mut chunked)?;
                    }
                }
                _ => {}
            }

            if self.len == MAX_HEADERS {
                return Err(Error::HeaderTooLarge);
            }
            self.hdrs[self.len].write((span(n0, n1), span(v0, v1)));
            self.len += 1;
        }

        // ── request-smuggling guards and framing ──
        if saw_te && (content_length.is_some() || self.minor == 0) {
            return Err(Error::Malformed);
        }
        if self.minor == 1 && self.host.is_none() {
            return Err(Error::Malformed);
        }
        self.keep_alive = if self.minor == 1 {
            !close
        } else {
            ka && !close
        };
        self.body = if chunked {
            Body::Chunked
        } else if let Some(n) = content_length {
            Body::Length(n)
        } else {
            Body::None
        };
        Ok(i)
    }
}

#[inline(always)]
fn parse_method(b: &[u8]) -> Result<(Method, usize), Error> {
    Ok(match b {
        [b'G', b'E', b'T', b' ', ..] => (Method::Get, 4),
        [b'P', b'O', b'S', b'T', b' ', ..] => (Method::Post, 5),
        [b'P', b'U', b'T', b' ', ..] => (Method::Put, 4),
        [b'H', b'E', b'A', b'D', b' ', ..] => (Method::Head, 5),
        [b'D', b'E', b'L', b'E', b'T', b'E', b' ', ..] => (Method::Delete, 7),
        [b'P', b'A', b'T', b'C', b'H', b' ', ..] => (Method::Patch, 6),
        [b'O', b'P', b'T', b'I', b'O', b'N', b'S', b' ', ..] => (Method::Options, 8),
        [b'T', b'R', b'A', b'C', b'E', b' ', ..] => (Method::Trace, 6),
        [b'C', b'O', b'N', b'N', b'E', b'C', b'T', b' ', ..] => (Method::Connect, 8),
        _ => return parse_method_other(b),
    })
}

#[cold]
#[inline(never)]
fn parse_method_other(b: &[u8]) -> Result<(Method, usize), Error> {
    let mut i = 0;
    while TOKEN[at(b, i) as usize] {
        i += 1;
    }
    if i == 0 || at(b, i) != b' ' {
        return Err(Error::Malformed);
    }
    Ok((Method::Other(span(0, i)), i + 1))
}

#[cold]
#[inline(never)]
fn reject_version(b: &[u8], i: usize) -> Error {
    if matches!(
        b.get(i..i + 10),
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
    ) || matches!(b.get(i..i + 8), Some(b"HTTP/2\r\n" | b"HTTP/3\r\n"))
    {
        Error::VersionNotSupported
    } else {
        Error::Malformed
    }
}

#[cold]
#[inline(never)]
fn parse_expect(minor: u8, val: &[u8], expect_100_continue: &mut bool) -> Result<(), Error> {
    if val.eq_ignore_ascii_case(b"100-continue") {
        if minor == 1 {
            *expect_100_continue = true;
        }
        Ok(())
    } else {
        Err(Error::ExpectationFailed)
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
) -> Result<(), Error> {
    // RFC 9112 §6.1: Transfer-Encoding is undefined in HTTP/1.0 and must be rejected.
    if minor == 0 || val.is_empty() {
        return Err(Error::Malformed);
    }
    for part in val.split(|&c| c == b',') {
        let coding = trim_ows(part);
        if coding.is_empty() || !coding.iter().all(|&c| TOKEN[c as usize]) {
            return Err(Error::Malformed);
        }
        // `chunked` must appear at most once and must be the final transfer coding.
        if *chunked {
            return Err(Error::Malformed);
        }
        if coding.eq_ignore_ascii_case(b"chunked") {
            *chunked = true;
        } else if !coding.eq_ignore_ascii_case(b"identity") {
            return Err(Error::UnsupportedEncoding);
        }
    }
    *saw_te = true;
    Ok(())
}

/// Validate non-origin RFC 9112 §3.2 `request-target` forms (`*`, `authority-form`, `absolute-form`),
/// returning `(target_end, path_span, query_span)`.
#[cold]
#[inline(never)]
fn parse_non_origin_target(
    b: &[u8],
    t0: usize,
    method: Method,
) -> Result<(usize, Span, Span), Error> {
    let t1 = scan_uri(b, t0);
    if t1 == t0 || at(b, t1) != b' ' {
        return Err(Error::Malformed);
    }
    let target = &b[t0..t1];

    if target == b"*" {
        return if method == Method::Options {
            Ok((t1, span(t0, t1), Span::default()))
        } else {
            Err(Error::Malformed)
        };
    }

    let mut question_pos: Option<usize> = None;
    let mut scheme_sep: Option<usize> = None;
    let mut i = 0usize;

    while i < target.len() {
        let c = target[i];
        if !semantics::is_valid_uri_byte(c) {
            return Err(Error::Malformed);
        }
        if c == b'%' {
            if i + 2 >= target.len()
                || !target[i + 1].is_ascii_hexdigit()
                || !target[i + 2].is_ascii_hexdigit()
            {
                return Err(Error::Malformed);
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
            Ok((t1, span(t0, t1), Span::default()))
        } else {
            Err(Error::Malformed)
        };
    }

    if let Some(colon) = scheme_sep {
        if !semantics::is_valid_scheme(&target[..colon]) {
            return Err(Error::Malformed);
        }

        let after_scheme_start = colon + 3;
        let query_end = question_pos.unwrap_or(target.len());
        if after_scheme_start >= query_end {
            return Err(Error::Malformed);
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
            return Err(Error::Malformed);
        }

        let query_span = match question_pos {
            Some(q) => span(t0 + q + 1, t1),
            None => Span::default(),
        };
        return Ok((t1, path_span, query_span));
    }

    Err(Error::Malformed)
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
