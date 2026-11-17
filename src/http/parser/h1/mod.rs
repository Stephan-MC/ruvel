use std::borrow::Cow;

use super::{Header, ParseError, Parser, Request, Status};
use crate::http::Method;

/// RFC 9110 §5.6.2 token character (`tchar`) lookup table.
///
/// Used to validate HTTP method names and header field names in O(1) per byte.
/// Because every `true` entry is strictly ASCII (`0x21..=0x7E`), any byte slice
/// whose bytes all satisfy `IS_TCHAR` is guaranteed to be valid UTF-8 without a
/// separate UTF-8 validation pass.
const IS_TCHAR: [bool; 256] = {
    let mut table = [false; 256];
    let mut b = 0usize;
    while b < 256 {
        table[b] = matches!(
            b as u8,
            b'!' | b'#'
                | b'$'
                | b'%'
                | b'&'
                | b'\''
                | b'*'
                | b'+'
                | b'-'
                | b'.'
                | b'0'..=b'9'
                | b'A'..=b'Z'
                | b'^'
                | b'_'
                | b'`'
                | b'a'..=b'z'
                | b'|'
                | b'~'
        );
        b += 1;
    }
    table
};

/// Tracks how the HTTP/1.x message body is framed on the wire (RFC 9112 §6).
enum BodyFraming {
    /// No `Transfer-Encoding: chunked`: body length is fixed by `Content-Length`
    /// (or `0` when neither `Content-Length` nor `Transfer-Encoding` is present).
    ContentLength(usize),
    /// `Transfer-Encoding: chunked`: body length is determined dynamically by
    /// walking `<hex-size>[;ext]\r\n<data>\r\n ... 0[;ext]\r\n[trailers]\r\n`.
    Chunked,
}

/// Zero-allocation streaming HTTP/1.x parser (`N` = max inline headers on stack).
pub struct H1<const N: usize>;

impl<const N: usize> Parser<N> for H1<N> {
    type Request<'a> = Request<'a, N>;

    #[inline(always)]
    fn parse<'a>(buf: &'a [u8]) -> Result<Status<Self::Request<'a>>, ParseError> {
        // An empty read buffer cannot contain even a partial request line yet.
        if buf.is_empty() {
            return Ok(Status::Incomplete);
        }

        // 1. Locate the end of the Request Line (`<method> <target> <version>\r\n`).
        //    If the TCP packet split mid-line (or right between `\r` and `\n`),
        //    `find_crlf` returns `Ok(None)` so we bubble up `Status::Incomplete`.
        let Some((line_end, mut total_consumed)) =
            Self::find_crlf(buf, ParseError::InvalidRequestLine)?
        else {
            return Ok(Status::Incomplete);
        };
        let request_line_bytes = &buf[..line_end];

        // 2. Parse and validate the Request Line without heap allocation.
        //    Notice we keep `method_bytes` borrowed here instead of allocating
        //    `Method::Unknown(String)` early, so a fragmented read later in the
        //    headers or body never wastes a heap allocation.
        let (method_bytes, path, query, version) = Self::parse_request_line(request_line_bytes)?;

        // 3. Parse Headers into a stack-allocated fixed array (zero heap allocations).
        //    `header_count` tracks how many entries in `headers` are actually populated.
        let mut headers = [Header {
            name: "",
            value: "",
        }; N];
        let mut header_count = 0usize;

        // Track framing headers inline during the header scan so we enforce
        // RFC 9112 request-smuggling guards in a single pass.
        let mut content_length: Option<usize> = None;
        let mut has_transfer_encoding = false;
        let mut is_chunked = false;

        // Advance `cursor` to the first byte after the Request Line's `\r\n`.
        let mut cursor = &buf[total_consumed..];

        loop {
            // Locate the next `\r\n` line boundary. If the current TCP fragment ends
            // mid-header (or on a trailing `\r`), return `Status::Incomplete` with
            // zero heap allocations so the caller can append more bytes and retry.
            let Some((line_end, line_consumed)) =
                Self::find_crlf(cursor, ParseError::InvalidHeader)?
            else {
                return Ok(Status::Incomplete);
            };

            // An empty line (`\r\n` at offset 0) terminates the HTTP header block.
            if line_end == 0 {
                cursor = &cursor[line_consumed..];
                total_consumed += line_consumed;
                break;
            }

            let header_line = &cursor[..line_end];
            cursor = &cursor[line_consumed..];
            total_consumed += line_consumed;

            // Reject header lines before writing if the header table is already full.
            if header_count >= N {
                return Err(ParseError::TooManyHeaders);
            }

            // Parse and validate `<field-name>:<OWS><field-value><OWS>` in one pass.
            let (name, value) = Self::parse_header_line(header_line)?;

            // Enforce RFC 9112 §6.1 & §6.3 message-framing invariants immediately.
            if name.eq_ignore_ascii_case("content-length") {
                let parsed_cl = Self::parse_content_length(value.as_bytes())?;
                if let Some(existing) = content_length {
                    // Reject duplicate `Content-Length` headers whose values disagree
                    // (classic CL.CL request-smuggling vector).
                    if existing != parsed_cl {
                        return Err(ParseError::InvalidBody);
                    }
                } else {
                    content_length = Some(parsed_cl);
                }
            } else if name.eq_ignore_ascii_case("transfer-encoding") {
                // Reject multiple `Transfer-Encoding` headers to prevent proxy/origin desync.
                if has_transfer_encoding {
                    return Err(ParseError::InvalidBody);
                }
                has_transfer_encoding = true;

                if value.eq_ignore_ascii_case("chunked") {
                    is_chunked = true;
                } else if !value.eq_ignore_ascii_case("identity") {
                    // Any unsupported coding (e.g., `gzip`, stacked codings) is rejected.
                    return Err(ParseError::InvalidBody);
                }
            }

            headers[header_count] = Header { name, value };
            header_count += 1;
        }

        // 4. Validate combined body framing rules before reading body bytes.
        //    RFC 9112 §6.3: A message MUST NOT contain both `Transfer-Encoding: chunked`
        //    and `Content-Length` (CL.TE / TE.CL request-smuggling vector).
        let framing = if is_chunked {
            if content_length.is_some() {
                return Err(ParseError::InvalidBody);
            }
            BodyFraming::Chunked
        } else {
            BodyFraming::ContentLength(content_length.unwrap_or(0))
        };

        // 5. Frame (and wait if fragmented) the message body from `cursor` (`&buf[total_consumed..]`).
        let body = match framing {
            BodyFraming::ContentLength(0) => Cow::Borrowed(&[][..]),
            BodyFraming::ContentLength(cl) => {
                // If fewer than `cl` body bytes have arrived across TCP segments,
                // signal `Incomplete` with zero heap allocations so the caller keeps buffering.
                if cursor.len() < cl {
                    return Ok(Status::Incomplete);
                }
                total_consumed = total_consumed
                    .checked_add(cl)
                    .ok_or(ParseError::InvalidBody)?;
                // Zero-copy slice directly into the caller's read buffer.
                Cow::Borrowed(&cursor[..cl])
            }
            BodyFraming::Chunked => {
                // Pass 1 (zero allocation): verify every chunk, extension, data CRLF,
                // 0-terminator, and trailer has arrived cleanly.
                let Some((wire_len, decoded_len)) = Self::frame_chunked_body(cursor)? else {
                    return Ok(Status::Incomplete);
                };
                total_consumed = total_consumed
                    .checked_add(wire_len)
                    .ok_or(ParseError::InvalidBody)?;
                // Pass 2: assemble chunk payload slices into a contiguous `Vec<u8>`
                // pre-sized to the exact `decoded_len` computed in Pass 1.
                Cow::Owned(Self::extract_chunked_body(&cursor[..wire_len], decoded_len))
            }
        };

        // 6. Construct `Method` only after the entire frame is complete so
        //    `Method::Unknown(String)` never allocates on `Status::Incomplete`.
        let method = Self::match_method(method_bytes);

        Ok(Status::Complete(
            Request {
                method,
                query,
                path,
                version,
                headers,
                header_count,
                body,
            },
            total_consumed,
        ))
    }
}

impl<const N: usize> H1<N> {
    /// Locate the next `\r\n` line delimiter in a single pass over `buf`.
    ///
    /// - Returns `Ok(Some((line_end, consumed)))` when `\r\n` is found, where
    ///   `&buf[..line_end]` excludes `\r\n` and `consumed == line_end + 2`.
    /// - Returns `Ok(None)` when the current TCP fragment ends before `\r\n`
    ///   (including when `buf` ends on a lone `\r` whose `\n` is in the next packet).
    /// - Returns `Err(invalid_err)` on bare `\n` or `\r` followed by a non-`\n` byte,
    ///   preventing LF-based line-desynchronization / request smuggling (RFC 9112 §2.2).
    #[inline(always)]
    fn find_crlf(
        buf: &[u8],
        invalid_err: ParseError,
    ) -> Result<Option<(usize, usize)>, ParseError> {
        let mut i = 0usize;
        while i < buf.len() {
            match buf[i] {
                b'\r' => {
                    // TCP segment boundary fell right between `\r` and `\n`.
                    if i + 1 >= buf.len() {
                        return Ok(None);
                    }
                    if buf[i + 1] == b'\n' {
                        return Ok(Some((i, i + 2)));
                    }
                    // Bare `\r` inside a line is illegal in HTTP/1.x headers/request lines.
                    return Err(invalid_err);
                }
                // Bare `\n` without an immediately preceding `\r` is rejected immediately
                // so it can never be skipped over to match a later `\r\n` in pipelined data.
                b'\n' => return Err(invalid_err),
                _ => i += 1,
            }
        }
        Ok(None)
    }

    /// Parse `<method> <request-target> <HTTP-version>` from a single CRLF-trimmed line.
    ///
    /// Returns `(method_bytes, path, query, version)` with zero heap allocations.
    #[inline(always)]
    fn parse_request_line(
        line: &[u8],
    ) -> Result<(&[u8], &str, &str, &str), ParseError> {
        // 1. Extract and validate the HTTP method token (must be non-empty `tchar` bytes).
        let mut idx = 0usize;
        while idx < line.len() && line[idx] != b' ' {
            if !IS_TCHAR[line[idx] as usize] {
                return Err(ParseError::InvalidMethod);
            }
            idx += 1;
        }
        if idx == 0 || idx >= line.len() {
            // Empty method (`" / HTTP/1.1"`) or missing space after method.
            return Err(ParseError::InvalidRequestLine);
        }
        let method_bytes = &line[..idx];

        // Consume the single SP separator between method and request-target.
        idx += 1;
        let uri_start = idx;

        // 2. Scan the request-target up to the second SP, validating that every byte is a
        //    visible ASCII character (`0x21..=0x7E`) and recording the first `?` for query split.
        let mut query_split: Option<usize> = None;
        while idx < line.len() && line[idx] != b' ' {
            let b = line[idx];
            // Reject control bytes (including NUL, HTAB, CR) and non-ASCII bytes in request-target.
            if !(0x21..=0x7E).contains(&b) {
                return Err(ParseError::InvalidUri);
            }
            if b == b'?' && query_split.is_none() {
                query_split = Some(idx);
            }
            idx += 1;
        }
        if idx == uri_start || idx >= line.len() {
            // Empty URI (`"GET  HTTP/1.1"`) or missing space before HTTP-version.
            return Err(ParseError::InvalidRequestLine);
        }

        let (path_bytes, query_bytes) = match query_split {
            Some(q) => (&line[uri_start..q], &line[q + 1..idx]),
            None => (&line[uri_start..idx], &[][..]),
        };

        // SAFETY: Every byte in `path_bytes` and `query_bytes` was verified in `0x21..=0x7E`
        // (pure 7-bit ASCII), which is unconditionally valid UTF-8.
        let path = unsafe { std::str::from_utf8_unchecked(path_bytes) };
        let query = unsafe { std::str::from_utf8_unchecked(query_bytes) };

        // Consume the single SP separator between request-target and HTTP-version.
        let version_bytes = &line[idx + 1..];

        // 3. Strictly validate the HTTP version token (no trailing spaces or garbage).
        let version = match version_bytes {
            b"HTTP/1.1" => "HTTP/1.1",
            b"HTTP/1.0" => "HTTP/1.0",
            b"HTTP/2.0" | b"HTTP/3.0" => return Err(ParseError::UnsupportedProtocol),
            _ => return Err(ParseError::InvalidVersion),
        };

        Ok((method_bytes, path, query, version))
    }

    /// Parse and validate one header line (`<field-name>:<OWS><field-value><OWS>`) in one pass.
    #[inline(always)]
    fn parse_header_line(line: &[u8]) -> Result<(&str, &str), ParseError> {
        // Reject obsolete line folding (`obs-fold`, RFC 9112 §5.2): a header line
        // cannot begin with SP or HTAB.
        if matches!(line.first(), Some(b' ' | b'\t')) {
            return Err(ParseError::InvalidHeader);
        }

        // Scan forward for `:` while verifying every byte of the field-name is a valid
        // RFC 9110 `tchar`. This rejects empty names and whitespace before `:` (e.g.
        // `"Content-Length : 10"`) in the same pass as finding `:`.
        let mut colon_idx = 0usize;
        while colon_idx < line.len() && line[colon_idx] != b':' {
            if !IS_TCHAR[line[colon_idx] as usize] {
                return Err(ParseError::InvalidHeader);
            }
            colon_idx += 1;
        }
        if colon_idx == 0 || colon_idx >= line.len() {
            return Err(ParseError::InvalidHeader);
        }

        // SAFETY: Every byte in `&line[..colon_idx]` passed `IS_TCHAR`, which only admits
        // ASCII bytes (`0x21..=0x7E`), so the slice is guaranteed valid UTF-8.
        let name = unsafe { std::str::from_utf8_unchecked(&line[..colon_idx]) };

        // Slice the field-value after `:` and trim both leading and trailing OWS (`SP` / `HTAB`).
        let mut val_bytes = &line[colon_idx + 1..];
        while let Some((&b' ' | &b'\t', rest)) = val_bytes.split_first() {
            val_bytes = rest;
        }
        while let Some((&b' ' | &b'\t', rest)) = val_bytes.split_last() {
            val_bytes = rest;
        }

        // Validate field-value octets (RFC 9110 §5.5): reject control chars (`0x00..=0x08`,
        // `0x0A..=0x1F`, `0x7F` — including NUL and bare CR) while permitting `SP`, `HTAB`,
        // `VCHAR`, and valid UTF-8 `obs-text`.
        for &b in val_bytes {
            if b == b'\t' || (0x20..=0x7E).contains(&b) || b >= 0x80 {
                continue;
            }
            return Err(ParseError::InvalidHeader);
        }

        let value = std::str::from_utf8(val_bytes).map_err(|_| ParseError::InvalidHeader)?;
        Ok((name, value))
    }

    /// Parse a non-empty ASCII decimal `Content-Length` value with overflow checking.
    #[inline(always)]
    fn parse_content_length(bytes: &[u8]) -> Result<usize, ParseError> {
        if bytes.is_empty() {
            return Err(ParseError::InvalidBody);
        }
        let mut total = 0usize;
        for &b in bytes {
            if !b.is_ascii_digit() {
                return Err(ParseError::InvalidBody);
            }
            total = total
                .checked_mul(10)
                .and_then(|v| v.checked_add(usize::from(b - b'0')))
                .ok_or(ParseError::InvalidBody)?;
        }
        Ok(total)
    }

    /// First zero-allocation pass over a `Transfer-Encoding: chunked` body (RFC 9112 §7.1).
    ///
    /// Returns:
    /// - `Ok(Some((wire_len, decoded_len)))` when the terminating `0`-chunk and final empty
    ///   `\r\n` line have arrived intact.
    /// - `Ok(None)` when the body is still fragmented across TCP segments (0 heap allocations).
    /// - `Err(ParseError::InvalidBody)` on invalid hex sizes, overflow, bare LF, or bad trailers.
    fn frame_chunked_body(src: &[u8]) -> Result<Option<(usize, usize)>, ParseError> {
        let mut pos = 0usize;
        let mut decoded_len = 0usize;

        loop {
            let Some((chunk_size, data_start)) = Self::parse_chunk_size_line(src, pos)? else {
                return Ok(None);
            };

            // A `0`-byte chunk terminates the chunk stream; consume any optional trailers.
            if chunk_size == 0 {
                let Some(wire_end) = Self::consume_chunked_trailers(src, data_start)? else {
                    return Ok(None);
                };
                return Ok(Some((wire_end, decoded_len)));
            }

            // Compute exact wire span of `<data>\r\n` using overflow-safe arithmetic.
            let data_end = data_start
                .checked_add(chunk_size)
                .ok_or(ParseError::InvalidBody)?;
            let chunk_end = data_end.checked_add(2).ok_or(ParseError::InvalidBody)?;

            if src.len() < chunk_end {
                // Fail fast if a non-CR byte already arrived right after chunk data.
                if src.len() > data_end && src[data_end] != b'\r' {
                    return Err(ParseError::InvalidBody);
                }
                return Ok(None);
            }
            if src[data_end] != b'\r' || src[data_end + 1] != b'\n' {
                return Err(ParseError::InvalidBody);
            }

            decoded_len = decoded_len
                .checked_add(chunk_size)
                .ok_or(ParseError::InvalidBody)?;
            pos = chunk_end;
        }
    }

    /// Parse `<hex-size>[;chunk-ext]\r\n` starting at `pos`.
    #[inline]
    fn parse_chunk_size_line(
        src: &[u8],
        mut pos: usize,
    ) -> Result<Option<(usize, usize)>, ParseError> {
        let mut chunk_size = 0usize;
        let mut has_digits = false;

        while pos < src.len() {
            let Some(nibble) = Self::hex_nibble(src[pos]) else {
                break;
            };
            has_digits = true;
            chunk_size = chunk_size
                .checked_mul(16)
                .and_then(|v| v.checked_add(usize::from(nibble)))
                .ok_or(ParseError::InvalidBody)?;
            pos += 1;
        }

        if !has_digits {
            return if pos == src.len() {
                Ok(None)
            } else {
                Err(ParseError::InvalidBody)
            };
        }
        if pos == src.len() {
            return Ok(None);
        }

        // Skip optional chunk extensions (`;ext=val`) or BWS up to `\r\n`.
        if matches!(src[pos], b';' | b' ' | b'\t') {
            while pos < src.len() && src[pos] != b'\r' {
                if src[pos] == b'\n' {
                    return Err(ParseError::InvalidBody);
                }
                pos += 1;
            }
            if pos == src.len() {
                return Ok(None);
            }
        }

        if src[pos] != b'\r' {
            return Err(ParseError::InvalidBody);
        }
        pos += 1;
        if pos == src.len() {
            return Ok(None);
        }
        if src[pos] != b'\n' {
            return Err(ParseError::InvalidBody);
        }

        Ok(Some((chunk_size, pos + 1)))
    }

    /// Consume zero or more optional HTTP/1.1 trailer lines up to the final empty `\r\n`.
    #[inline]
    fn consume_chunked_trailers(src: &[u8], mut pos: usize) -> Result<Option<usize>, ParseError> {
        loop {
            if pos == src.len() {
                return Ok(None);
            }
            if src[pos] == b'\r' {
                if pos + 1 == src.len() {
                    return Ok(None);
                }
                if src[pos + 1] != b'\n' {
                    return Err(ParseError::InvalidBody);
                }
                return Ok(Some(pos + 2));
            }

            // Each trailer line must contain `:` and end with `\r\n` (no bare `\n`).
            let mut has_colon = false;
            while pos < src.len() && src[pos] != b'\r' {
                if src[pos] == b'\n' {
                    return Err(ParseError::InvalidBody);
                }
                if src[pos] == b':' {
                    has_colon = true;
                }
                pos += 1;
            }
            if pos == src.len() {
                return Ok(None);
            }
            if !has_colon {
                return Err(ParseError::InvalidBody);
            }
            if pos + 1 == src.len() {
                return Ok(None);
            }
            if src[pos + 1] != b'\n' {
                return Err(ParseError::InvalidBody);
            }
            pos += 2;
        }
    }

    /// Second pass over an already-validated chunked wire slice, assembling chunk payloads
    /// into a single `Vec<u8>` pre-sized to `decoded_len`.
    fn extract_chunked_body(src: &[u8], decoded_len: usize) -> Vec<u8> {
        if decoded_len == 0 {
            return Vec::new();
        }
        let mut out = Vec::with_capacity(decoded_len);
        let mut pos = 0usize;

        while let Ok(Some((chunk_size, data_start))) = Self::parse_chunk_size_line(src, pos) {
            if chunk_size == 0 {
                break;
            }
            let data_end = data_start + chunk_size;
            out.extend_from_slice(&src[data_start..data_end]);
            pos = data_end + 2;
        }

        out
    }

    /// Convert one ASCII hex digit byte into its `0..=15` nibble value.
    #[inline(always)]
    fn hex_nibble(b: u8) -> Option<u8> {
        match b {
            b'0'..=b'9' => Some(b - b'0'),
            b'a'..=b'f' => Some(b - b'a' + 10),
            b'A'..=b'F' => Some(b - b'A' + 10),
            _ => None,
        }
    }

    /// Map validated ASCII `tchar` method bytes to [`Method`].
    #[inline(always)]
    fn match_method(bytes: &[u8]) -> Method {
        match bytes {
            b"GET" => Method::Get,
            b"POST" => Method::Post,
            b"PUT" => Method::Put,
            b"DELETE" => Method::Delete,
            b"PATCH" => Method::Patch,
            b"HEAD" => Method::Head,
            b"OPTIONS" => Method::Options,
            b"TRACE" => Method::Trace,
            b"CONNECT" => Method::Connect,
            other => {
                // SAFETY: `parse_request_line` verified all bytes satisfy `IS_TCHAR` (ASCII).
                let s = unsafe { std::str::from_utf8_unchecked(other) };
                Method::Unknown(s.to_owned())
            }
        }
    }
}
