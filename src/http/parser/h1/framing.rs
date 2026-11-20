use std::borrow::Cow;

use crate::http::parser::{Error, Header, HttpVersion, semantics};

const INLINE_CHUNK_SPANS: usize = 16;
const MAX_CHUNK_EXT_BYTES: usize = 1024;
const MAX_TRAILER_HEADERS: usize = 32;

/// Framing strategy derived from validated HTTP/1.x headers.
#[derive(Debug, Copy, Clone, PartialEq, Eq)]
pub(super) enum BodyFraming {
    ContentLength(usize),
    Chunked,
}

/// Framed message body paired with total wire bytes consumed.
pub(super) type FramedBody<'a> = (Cow<'a, [u8]>, usize);

/// Resumable state for incremental HTTP/1.x parsing across fragmented TCP reads.
///
/// Callers append incoming socket bytes to a single uncompacted buffer while parsing one
/// request, and call [`H1Cursor::reset`] once a complete frame is returned or before
/// reusing the cursor on a new buffer.
#[derive(Debug, Default, Clone)]
pub struct H1Cursor {
    pub(super) head_scan_pos: usize,
    pub(super) head_start: usize,
    pub(super) first_crlf: Option<usize>,
    pub(super) header_end: Option<usize>,
    pub(super) framing: Option<BodyFraming>,
    pub(super) expect_100_continue: bool,
    pub(super) chunk_pos: usize,
    pub(super) chunk_decoded_len: usize,
    pub(super) chunk_count: usize,
    pub(super) spans: Vec<(usize, usize)>,
}

impl H1Cursor {
    /// Creates a fresh incremental parser cursor.
    #[inline]
    pub const fn new() -> Self {
        Self {
            head_scan_pos: 0,
            head_start: 0,
            first_crlf: None,
            header_end: None,
            framing: None,
            expect_100_continue: false,
            chunk_pos: 0,
            chunk_decoded_len: 0,
            chunk_count: 0,
            spans: Vec::new(),
        }
    }

    /// Resets cursor state for the next request while retaining any allocated span capacity.
    #[inline]
    pub fn reset(&mut self) {
        self.head_scan_pos = 0;
        self.head_start = 0;
        self.first_crlf = None;
        self.header_end = None;
        self.framing = None;
        self.expect_100_continue = false;
        self.chunk_pos = 0;
        self.chunk_decoded_len = 0;
        self.chunk_count = 0;
        self.spans.clear();
    }

    /// Returns the total wire frame length in bytes (`head + Content-Length`) once headers are parsed.
    ///
    /// Returns `None` if headers are still incomplete or if the body uses chunked transfer framing.
    #[inline]
    pub fn expected_total_len(&self) -> Option<usize> {
        let header_end = self.header_end?;
        match self.framing? {
            BodyFraming::ContentLength(len) => header_end.checked_add(4)?.checked_add(len),
            BodyFraming::Chunked => None,
        }
    }

    /// Atomically returns and clears whether an interim `100 Continue` response should be emitted.
    #[inline]
    pub fn take_expect_100_continue(&mut self) -> bool {
        let pending = self.expect_100_continue;
        self.expect_100_continue = false;
        pending
    }
}

/// Single-pass header accumulator capturing `Content-Length` and `Transfer-Encoding` metadata.
#[derive(Default)]
pub(super) struct FramingMeta<'a> {
    content_length: Option<usize>,
    te_header: Option<Header<'a>>,
    te_chunked: bool,
}

impl<'a> FramingMeta<'a> {
    /// Inspect one validated header during the main header-parsing pass.
    #[inline]
    pub(super) fn observe_header(
        &mut self,
        version: HttpVersion,
        header: Header<'a>,
    ) -> Result<(), Error<'a>> {
        match header.name.len() {
            14 if header.name.eq_ignore_ascii_case("content-length") => {
                let parsed = semantics::parse_content_length(header.value)
                    .ok_or(Error::InvalidHeader(header))?;
                if let Some(existing) = self.content_length {
                    if existing != parsed {
                        return Err(Error::InvalidHeader(header));
                    }
                } else {
                    self.content_length = Some(parsed);
                }
            }
            17 if header.name.eq_ignore_ascii_case("transfer-encoding") => {
                // RFC 9112 §6.1: Transfer-Encoding is undefined in HTTP/1.0 and must be rejected.
                if version == HttpVersion::Http1_0 || header.value.is_empty() {
                    return Err(Error::InvalidHeader(header));
                }

                for raw_part in header.value.split(',') {
                    let coding = raw_part.trim_matches([' ', '\t']);
                    if coding.is_empty() || !coding.bytes().all(semantics::is_tchar) {
                        return Err(Error::InvalidHeader(header));
                    }
                    // `chunked` must appear at most once and must be the final transfer coding.
                    if self.te_chunked {
                        return Err(Error::InvalidHeader(header));
                    }
                    if coding.eq_ignore_ascii_case("chunked") {
                        self.te_chunked = true;
                    } else if !coding.eq_ignore_ascii_case("identity") {
                        return Err(Error::InvalidHeader(header));
                    }
                }

                if self.te_header.is_none() {
                    self.te_header = Some(header);
                }
            }
            _ => {}
        }

        Ok(())
    }

    /// Resolve the final framing strategy after all headers and `Host` checks pass.
    #[inline]
    pub(super) fn finalize(self) -> Result<BodyFraming, Error<'a>> {
        if let Some(te_header) = self.te_header {
            // RFC 9112 §6.3: A message containing both Transfer-Encoding and Content-Length is invalid.
            if self.content_length.is_some() {
                return Err(Error::InvalidHeader(te_header));
            }
            return Ok(if self.te_chunked {
                BodyFraming::Chunked
            } else {
                BodyFraming::ContentLength(0)
            });
        }

        Ok(BodyFraming::ContentLength(self.content_length.unwrap_or(0)))
    }
}

/// Extract the framed body slice (or decoded chunked payload) and total wire bytes consumed.
///
/// Returns `Ok(None)` without heap allocations when the buffer does not yet hold the complete body.
pub(super) fn extract_body<'a>(
    buf: &'a [u8],
    body_start: usize,
    framing: BodyFraming,
) -> Result<Option<FramedBody<'a>>, Error<'a>> {
    match framing {
        BodyFraming::ContentLength(len) => extract_content_length(buf, body_start, len),
        BodyFraming::Chunked => {
            let Some(plan) = frame_chunked_body(&buf[body_start..])? else {
                return Ok(None);
            };
            let total_len = body_start
                .checked_add(plan.wire_consumed)
                .ok_or(Error::InvalidBody)?;
            let body = decode_chunked_body(&buf[body_start..total_len], &plan);
            Ok(Some((body, total_len)))
        }
    }
}

/// Streaming variant of [`extract_body`] that resumes chunked validation from `cursor.chunk_pos`.
pub(super) fn extract_body_with_cursor<'a>(
    buf: &'a [u8],
    body_start: usize,
    framing: BodyFraming,
    cursor: &mut H1Cursor,
) -> Result<Option<FramedBody<'a>>, Error<'a>> {
    match framing {
        BodyFraming::ContentLength(len) => extract_content_length(buf, body_start, len),
        BodyFraming::Chunked => {
            let src = &buf[body_start..];
            if cursor.chunk_pos > src.len() {
                return Err(Error::InvalidBody);
            }

            let Some(wire_consumed) = frame_chunked_with_cursor(src, cursor)? else {
                return Ok(None);
            };
            let total_len = body_start
                .checked_add(wire_consumed)
                .ok_or(Error::InvalidBody)?;
            let body = decode_chunked_from_spans(&buf[body_start..total_len], cursor);
            Ok(Some((body, total_len)))
        }
    }
}

#[inline]
fn extract_content_length<'a>(
    buf: &'a [u8],
    body_start: usize,
    len: usize,
) -> Result<Option<FramedBody<'a>>, Error<'a>> {
    let total_len = body_start.checked_add(len).ok_or(Error::InvalidBody)?;
    if buf.len() < total_len {
        return Ok(None);
    }

    let body = if len == 0 {
        Cow::Borrowed(&[][..])
    } else {
        Cow::Borrowed(&buf[body_start..total_len])
    };
    Ok(Some((body, total_len)))
}

struct ChunkedPlan {
    wire_consumed: usize,
    decoded_len: usize,
    chunk_count: usize,
    spans: [(usize, usize); INLINE_CHUNK_SPANS],
}

/// First zero-allocation pass validating chunk framing while caching up to 16 chunk data spans.
fn frame_chunked_body<'a>(src: &'a [u8]) -> Result<Option<ChunkedPlan>, Error<'a>> {
    let mut pos = 0usize;
    let mut decoded_len = 0usize;
    let mut chunk_count = 0usize;
    let mut spans = [(0usize, 0usize); INLINE_CHUNK_SPANS];

    loop {
        let Some((chunk_size, data_start)) = parse_chunk_size_line(src, pos)? else {
            return Ok(None);
        };

        if chunk_size == 0 {
            let Some(wire_end) = consume_chunked_trailers(src, data_start)? else {
                return Ok(None);
            };
            return Ok(Some(ChunkedPlan {
                wire_consumed: wire_end,
                decoded_len,
                chunk_count,
                spans,
            }));
        }

        let data_end = data_start
            .checked_add(chunk_size)
            .ok_or(Error::InvalidBody)?;
        let chunk_end = data_end.checked_add(2).ok_or(Error::InvalidBody)?;
        if src.len() < chunk_end {
            // Fail fast if a non-CR byte immediately follows the chunk data before EOF.
            if src.len() > data_end && src[data_end] != b'\r' {
                return Err(Error::InvalidBody);
            }
            return Ok(None);
        }
        if src[data_end] != b'\r' || src[data_end + 1] != b'\n' {
            return Err(Error::InvalidBody);
        }

        if chunk_count < INLINE_CHUNK_SPANS {
            spans[chunk_count] = (data_start, data_end);
        }
        chunk_count += 1;
        decoded_len = decoded_len
            .checked_add(chunk_size)
            .ok_or(Error::InvalidBody)?;
        pos = chunk_end;
    }
}

/// Resumable chunked framing pass that advances `cursor.chunk_pos` per completed chunk.
fn frame_chunked_with_cursor<'a>(
    src: &'a [u8],
    cursor: &mut H1Cursor,
) -> Result<Option<usize>, Error<'a>> {
    loop {
        let Some((chunk_size, data_start)) = parse_chunk_size_line(src, cursor.chunk_pos)? else {
            return Ok(None);
        };

        if chunk_size == 0 {
            let Some(wire_end) = consume_chunked_trailers(src, data_start)? else {
                return Ok(None);
            };
            return Ok(Some(wire_end));
        }

        let data_end = data_start
            .checked_add(chunk_size)
            .ok_or(Error::InvalidBody)?;
        let chunk_end = data_end.checked_add(2).ok_or(Error::InvalidBody)?;
        if src.len() < chunk_end {
            if src.len() > data_end && src[data_end] != b'\r' {
                return Err(Error::InvalidBody);
            }
            return Ok(None);
        }
        if src[data_end] != b'\r' || src[data_end + 1] != b'\n' {
            return Err(Error::InvalidBody);
        }

        cursor.spans.push((data_start, data_end));
        cursor.chunk_count += 1;
        cursor.chunk_decoded_len = cursor
            .chunk_decoded_len
            .checked_add(chunk_size)
            .ok_or(Error::InvalidBody)?;
        cursor.chunk_pos = chunk_end;
    }
}

/// Assemble decoded chunk payload without re-parsing cached chunk headers.
///
/// Zero-length and single-chunk payloads borrow directly from `src` without heap allocation.
fn decode_chunked_body<'a>(src: &'a [u8], plan: &ChunkedPlan) -> Cow<'a, [u8]> {
    if plan.decoded_len == 0 {
        return Cow::Borrowed(&[][..]);
    }
    if plan.chunk_count == 1 {
        let (start, end) = plan.spans[0];
        return Cow::Borrowed(&src[start..end]);
    }

    let mut out = Vec::with_capacity(plan.decoded_len);
    let cached = plan.chunk_count.min(INLINE_CHUNK_SPANS);
    for &(start, end) in &plan.spans[..cached] {
        out.extend_from_slice(&src[start..end]);
    }

    if plan.chunk_count > INLINE_CHUNK_SPANS {
        let mut pos = plan.spans[INLINE_CHUNK_SPANS - 1].1 + 2;
        while let Ok(Some((chunk_size, data_start))) = parse_chunk_size_line(src, pos) {
            if chunk_size == 0 {
                break;
            }
            let data_end = data_start + chunk_size;
            out.extend_from_slice(&src[data_start..data_end]);
            pos = data_end + 2;
        }
    }

    Cow::Owned(out)
}

fn decode_chunked_from_spans<'a>(src: &'a [u8], cursor: &H1Cursor) -> Cow<'a, [u8]> {
    if cursor.chunk_decoded_len == 0 {
        return Cow::Borrowed(&[][..]);
    }
    if cursor.chunk_count == 1 {
        let (start, end) = cursor.spans[0];
        return Cow::Borrowed(&src[start..end]);
    }

    let mut out = Vec::with_capacity(cursor.chunk_decoded_len);
    for &(start, end) in &cursor.spans {
        out.extend_from_slice(&src[start..end]);
    }
    Cow::Owned(out)
}

/// Parse one `<hex-size>[chunk-ext]\r\n` line starting at `pos` per RFC 9112 §7.1.1.
fn parse_chunk_size_line<'a>(
    src: &'a [u8],
    mut pos: usize,
) -> Result<Option<(usize, usize)>, Error<'a>> {
    let mut chunk_size = 0usize;
    let mut has_digits = false;

    while pos < src.len() {
        let Some(nibble) = hex_nibble(src[pos]) else {
            break;
        };
        has_digits = true;
        chunk_size = chunk_size
            .checked_mul(16)
            .ok_or(Error::InvalidBody)?
            .checked_add(usize::from(nibble))
            .ok_or(Error::InvalidBody)?;
        pos += 1;
    }

    if !has_digits {
        return if pos == src.len() {
            Ok(None)
        } else {
            Err(Error::InvalidBody)
        };
    }
    if pos == src.len() {
        return Ok(None);
    }

    if matches!(src[pos], b';' | b' ' | b'\t') {
        let Some(after_ext) = parse_chunk_extensions(src, pos)? else {
            return Ok(None);
        };
        pos = after_ext;
    }

    if src[pos] != b'\r' {
        return Err(Error::InvalidBody);
    }
    pos += 1;
    if pos == src.len() {
        return Ok(None);
    }
    if src[pos] != b'\n' {
        return Err(Error::InvalidBody);
    }

    Ok(Some((chunk_size, pos + 1)))
}

#[inline]
fn check_ext_cap<'a>(pos: usize, ext_start: usize) -> Result<(), Error<'a>> {
    if pos - ext_start > MAX_CHUNK_EXT_BYTES {
        Err(Error::InvalidBody)
    } else {
        Ok(())
    }
}

#[inline]
fn skip_bws<'a>(
    src: &'a [u8],
    mut pos: usize,
    ext_start: usize,
) -> Result<(usize, bool), Error<'a>> {
    let mut saw = false;
    while pos < src.len() && matches!(src[pos], b' ' | b'\t') {
        saw = true;
        pos += 1;
        check_ext_cap(pos, ext_start)?;
    }
    Ok((pos, saw))
}

#[inline]
fn scan_tchars<'a>(src: &'a [u8], mut pos: usize, ext_start: usize) -> Result<usize, Error<'a>> {
    while pos < src.len() && semantics::is_tchar(src[pos]) {
        pos += 1;
        check_ext_cap(pos, ext_start)?;
    }
    Ok(pos)
}

fn parse_quoted_ext_val<'a>(
    src: &'a [u8],
    mut pos: usize,
    ext_start: usize,
) -> Result<Option<usize>, Error<'a>> {
    pos += 1; // skip opening `"`
    check_ext_cap(pos, ext_start)?;

    let mut escaped = false;
    loop {
        if pos == src.len() {
            return Ok(None);
        }
        let b = src[pos];
        pos += 1;
        check_ext_cap(pos, ext_start)?;

        if escaped {
            if !semantics::is_valid_header_value_byte(b) {
                return Err(Error::InvalidBody);
            }
            escaped = false;
        } else if b == b'\\' {
            escaped = true;
        } else if b == b'"' {
            return Ok(Some(pos));
        } else if !semantics::is_valid_header_value_byte(b) {
            return Err(Error::InvalidBody);
        }
    }
}

/// Enforce `chunk-ext = *( BWS ";" BWS chunk-ext-name [ BWS "=" BWS chunk-ext-val ] )`
/// and cap extension bytes at [`MAX_CHUNK_EXT_BYTES`].
fn parse_chunk_extensions<'a>(src: &'a [u8], mut pos: usize) -> Result<Option<usize>, Error<'a>> {
    let ext_start = pos;

    loop {
        let (after_bws, saw_bws) = skip_bws(src, pos, ext_start)?;
        pos = after_bws;
        if pos == src.len() {
            return Ok(None);
        }

        if src[pos] != b';' {
            return if saw_bws {
                Err(Error::InvalidBody)
            } else {
                Ok(Some(pos))
            };
        }

        pos += 1;
        check_ext_cap(pos, ext_start)?;

        (pos, _) = skip_bws(src, pos, ext_start)?;
        if pos == src.len() {
            return Ok(None);
        }

        let name_start = pos;
        pos = scan_tchars(src, pos, ext_start)?;
        if pos == name_start {
            return if pos == src.len() {
                Ok(None)
            } else {
                Err(Error::InvalidBody)
            };
        }
        if pos == src.len() {
            return Ok(None);
        }

        let (after_name_bws, saw_val_bws) = skip_bws(src, pos, ext_start)?;
        pos = after_name_bws;
        if pos == src.len() {
            return Ok(None);
        }

        if src[pos] != b'=' {
            if saw_val_bws && src[pos] != b';' {
                return Err(Error::InvalidBody);
            }
            continue;
        }

        pos += 1;
        check_ext_cap(pos, ext_start)?;

        (pos, _) = skip_bws(src, pos, ext_start)?;
        if pos == src.len() {
            return Ok(None);
        }

        if src[pos] == b'"' {
            let Some(after_quote) = parse_quoted_ext_val(src, pos, ext_start)? else {
                return Ok(None);
            };
            pos = after_quote;
        } else {
            let val_start = pos;
            pos = scan_tchars(src, pos, ext_start)?;
            if pos == val_start {
                return if pos == src.len() {
                    Ok(None)
                } else {
                    Err(Error::InvalidBody)
                };
            }
        }

        if pos == src.len() {
            return Ok(None);
        }
    }
}

/// Validate optional trailer field lines following a `0`-size chunk and consume the terminating `\r\n`.
fn consume_chunked_trailers<'a>(src: &'a [u8], mut pos: usize) -> Result<Option<usize>, Error<'a>> {
    let mut trailer_count = 0usize;

    loop {
        if pos == src.len() {
            return Ok(None);
        }
        if src[pos] == b'\r' {
            if pos + 1 == src.len() {
                return Ok(None);
            }
            if src[pos + 1] != b'\n' {
                return Err(Error::InvalidBody);
            }
            return Ok(Some(pos + 2));
        }

        if trailer_count >= MAX_TRAILER_HEADERS {
            return Err(Error::TooManyHeaders);
        }

        let line_start = pos;
        let Some(term_pos) = semantics::find_cr_or_lf(src, pos) else {
            return Ok(None);
        };
        if src[term_pos] == b'\n' {
            return Err(Error::InvalidBody);
        }
        if term_pos + 1 == src.len() {
            return Ok(None);
        }
        if src[term_pos + 1] != b'\n' {
            return Err(Error::InvalidBody);
        }

        let line = std::str::from_utf8(&src[line_start..term_pos]).map_err(|_| {
            Error::InvalidHeader(Header {
                name: "",
                value: "",
            })
        })?;
        let header = super::parse_header_line(line)?;
        if semantics::is_forbidden_trailer(header.name) {
            return Err(Error::InvalidHeader(header));
        }

        trailer_count += 1;
        pos = term_pos + 2;
    }
}

#[inline]
const fn hex_nibble(byte: u8) -> Option<u8> {
    match byte {
        b'0'..=b'9' => Some(byte - b'0'),
        b'a'..=b'f' => Some(byte - b'a' + 10),
        b'A'..=b'F' => Some(byte - b'A' + 10),
        _ => None,
    }
}
