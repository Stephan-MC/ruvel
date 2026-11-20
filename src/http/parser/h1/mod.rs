mod framing;

pub use framing::H1Cursor;

use crate::http::{
    Method,
    parser::{
        Error, Header, Headers, HttpVersion, MAX_HEADERS_SIZE, ParsedRequest, Parser, Status,
        semantics,
    },
};
use framing::{BodyFraming, FramingMeta};

/// Optional bound on leading empty `CRLF` lines tolerated prior to the request line (RFC 9112 §2.2).
/// Keep `None` for strict rejection unless explicitly enabled (e.g., `Some(1)`).
const LEADING_CRLF_LIMIT: Option<usize> = None;

pub struct H1;

impl Parser for H1 {
    type Output<'a> = ParsedRequest<'a>;

    fn parse<'a>(buf: &'a [u8]) -> Result<Status<Self::Output<'a>>, Error<'a>> {
        let Some(bounds) = find_head_end(buf)? else {
            return Ok(Status::Incomplete);
        };

        let head = parse_head_only(buf, bounds)?;
        let body_start = bounds.header_end + 4;
        let Some((body, consumed)) = framing::extract_body(buf, body_start, head.framing)? else {
            return Ok(Status::Incomplete);
        };

        Ok(Status::Complete(
            ParsedRequest {
                method: head.method,
                query: head.query,
                path: head.path,
                version: head.version,
                headers: head.headers,
                body,
            },
            consumed,
        ))
    }
}

impl H1 {
    /// Incrementally parses an HTTP/1.x request across fragmented TCP reads using `cursor`.
    pub fn parse_incremental<'a>(
        buf: &'a [u8],
        cursor: &mut H1Cursor,
    ) -> Result<Status<ParsedRequest<'a>>, Error<'a>> {
        let (bounds, framing) =
            if let (Some(header_end), Some(framing)) = (cursor.header_end, cursor.framing) {
                if header_end + 4 > buf.len() {
                    return Err(Error::InvalidBody);
                }
                (
                    HeadBounds {
                        head_start: cursor.head_start,
                        first_crlf: cursor.first_crlf,
                        header_end,
                    },
                    framing,
                )
            } else {
                let Some(bounds) = find_head_end_with_cursor(buf, cursor)? else {
                    return Ok(Status::Incomplete);
                };
                let head = parse_head_only(buf, bounds)?;
                cursor.head_start = bounds.head_start;
                cursor.first_crlf = bounds.first_crlf;
                cursor.header_end = Some(bounds.header_end);
                cursor.framing = Some(head.framing);
                cursor.expect_100_continue =
                    head.expect_100_continue && bounds.header_end + 4 == buf.len();
                (bounds, head.framing)
            };

        let body_start = bounds.header_end + 4;
        let Some((body, consumed)) =
            framing::extract_body_with_cursor(buf, body_start, framing, cursor)?
        else {
            return Ok(Status::Incomplete);
        };

        let head = parse_head_only(buf, bounds)?;
        Ok(Status::Complete(
            ParsedRequest {
                method: head.method,
                query: head.query,
                path: head.path,
                version: head.version,
                headers: head.headers,
                body,
            },
            consumed,
        ))
    }
}

#[derive(Copy, Clone)]
struct HeadBounds {
    head_start: usize,
    first_crlf: Option<usize>,
    header_end: usize,
}

struct ParsedHead<'a> {
    method: Method,
    path: &'a str,
    query: &'a str,
    version: HttpVersion,
    headers: Headers<'a>,
    framing: BodyFraming,
    expect_100_continue: bool,
}

#[derive(Copy, Clone)]
enum HostState<'a> {
    Missing,
    Single(&'a str),
    Duplicate,
}

fn parse_head_only<'a>(buf: &'a [u8], bounds: HeadBounds) -> Result<ParsedHead<'a>, Error<'a>> {
    let (request_line, header_bytes) = match bounds.first_crlf {
        Some(crlf) => (
            &buf[bounds.head_start..crlf],
            &buf[crlf + 2..bounds.header_end],
        ),
        None => (&buf[bounds.head_start..bounds.header_end], &[][..]),
    };

    let (method, path, query, version) = parse_request_line(request_line)?;

    let mut headers = Headers::new();
    let mut host_state = HostState::Missing;
    let mut framing_meta = FramingMeta::default();
    let mut expect_100_continue = false;

    if !header_bytes.is_empty() {
        let headers_str = std::str::from_utf8(header_bytes).map_err(|_| {
            Error::InvalidHeader(Header {
                name: "",
                value: "",
            })
        })?;

        let mut pos = 0usize;
        while pos < header_bytes.len() {
            let line_end = match semantics::find_byte(header_bytes, b'\n', pos) {
                Some(lf_pos) => lf_pos.saturating_sub(1),
                None => header_bytes.len(),
            };
            let line = &headers_str[pos..line_end];
            let header = parse_header_line(line)?;
            headers.push(header)?;

            if header.name.len() == 4 && header.name.eq_ignore_ascii_case("host") {
                host_state = match host_state {
                    HostState::Missing => HostState::Single(header.value),
                    HostState::Single(_) | HostState::Duplicate => HostState::Duplicate,
                };
            } else if header.name.len() == 6 && header.name.eq_ignore_ascii_case("expect") {
                if header.value.eq_ignore_ascii_case("100-continue") {
                    if version == HttpVersion::Http1_1 {
                        expect_100_continue = true;
                    }
                } else {
                    return Err(Error::ExpectationFailed);
                }
            } else {
                framing_meta.observe_header(version, header)?;
            }

            pos = line_end.saturating_add(2);
        }
    }

    validate_host_state(version, host_state)?;
    let framing = framing_meta.finalize()?;

    Ok(ParsedHead {
        method,
        path,
        query,
        version,
        headers,
        framing,
        expect_100_continue,
    })
}

#[inline]
fn validate_host_state<'a>(
    version: HttpVersion,
    host_state: HostState<'a>,
) -> Result<(), Error<'a>> {
    match host_state {
        HostState::Missing if version == HttpVersion::Http1_1 => Err(Error::MissingHost),
        HostState::Missing => Ok(()),
        HostState::Single(value) if semantics::is_valid_host_value(value) => Ok(()),
        HostState::Single(_) | HostState::Duplicate => Err(Error::InvalidHost),
    }
}

/// Locate the `\r\n\r\n` head terminator and first `\r\n` in a single SWAR pass.
#[inline]
fn find_head_end(buf: &[u8]) -> Result<Option<HeadBounds>, Error<'static>> {
    let mut scan_pos = 0usize;
    let mut head_start = 0usize;
    let mut first_crlf = None;
    scan_head_bounds(buf, &mut scan_pos, &mut head_start, &mut first_crlf)
}

#[inline]
fn find_head_end_with_cursor(
    buf: &[u8],
    cursor: &mut H1Cursor,
) -> Result<Option<HeadBounds>, Error<'static>> {
    scan_head_bounds(
        buf,
        &mut cursor.head_scan_pos,
        &mut cursor.head_start,
        &mut cursor.first_crlf,
    )
}

fn scan_head_bounds(
    buf: &[u8],
    scan_pos: &mut usize,
    head_start: &mut usize,
    first_crlf: &mut Option<usize>,
) -> Result<Option<HeadBounds>, Error<'static>> {
    if *scan_pos == 0
        && first_crlf.is_none()
        && let Some(limit) = LEADING_CRLF_LIMIT
    {
        let mut skipped = 0usize;
        while skipped < limit
            && *head_start + 1 < buf.len()
            && buf[*head_start] == b'\r'
            && buf[*head_start + 1] == b'\n'
        {
            *head_start += 2;
            skipped += 1;
        }
        *scan_pos = *head_start;
    }

    let max_end = (*head_start).saturating_add(MAX_HEADERS_SIZE);
    let scan_buf = &buf[..buf.len().min(max_end)];
    let mut i = (*scan_pos).min(scan_buf.len());

    while let Some(pos) = semantics::find_cr_or_lf(scan_buf, i) {
        let in_headers = first_crlf.is_some();
        if scan_buf[pos] == b'\n' {
            return Err(if in_headers {
                Error::InvalidHeader(Header {
                    name: "",
                    value: "",
                })
            } else {
                Error::InvalidRequestLine
            });
        }

        if pos + 1 >= scan_buf.len() {
            if buf.len() >= max_end {
                return Err(Error::HeadersTooLarge);
            }
            *scan_pos = pos;
            return Ok(None);
        }
        if scan_buf[pos + 1] != b'\n' {
            return Err(if in_headers {
                Error::InvalidHeader(Header {
                    name: "",
                    value: "",
                })
            } else {
                Error::InvalidRequestLine
            });
        }

        if pos + 3 < scan_buf.len() {
            if scan_buf[pos + 2] == b'\r' && scan_buf[pos + 3] == b'\n' {
                return Ok(Some(HeadBounds {
                    head_start: *head_start,
                    first_crlf: *first_crlf,
                    header_end: pos,
                }));
            }
        } else {
            if buf.len() >= max_end {
                return Err(Error::HeadersTooLarge);
            }
            // Fewer than 4 bytes remain from this `\r`; resume here on the next fragment.
            *scan_pos = pos;
            return Ok(None);
        }

        if first_crlf.is_none() {
            *first_crlf = Some(pos);
        }
        i = pos + 2;
    }

    if buf.len() >= max_end {
        return Err(Error::HeadersTooLarge);
    }
    *scan_pos = scan_buf.len();
    Ok(None)
}

/// Parse and validate `method SP request-target SP HTTP-version` (RFC 9112 §3) without `Split`.
fn parse_request_line(line: &[u8]) -> Result<(Method, &str, &str, HttpVersion), Error<'static>> {
    if line.is_empty() {
        return Err(Error::InvalidRequestLine);
    }

    let Some(sp1) = semantics::find_byte(line, b' ', 0) else {
        return Err(Error::InvalidRequestLine);
    };
    let method_bytes = &line[..sp1];

    let rest_start = sp1 + 1;
    let Some(sp2) = semantics::find_byte(line, b' ', rest_start) else {
        return Err(Error::InvalidRequestLine);
    };
    let target_bytes = &line[rest_start..sp2];
    let version_bytes = &line[sp2 + 1..];

    if method_bytes.is_empty()
        || target_bytes.is_empty()
        || version_bytes.is_empty()
        || semantics::find_byte(version_bytes, b' ', 0).is_some()
    {
        return Err(Error::InvalidRequestLine);
    }

    let method = parse_method(method_bytes)?;
    let (path, query) = parse_target(target_bytes, &method)?;
    let version = parse_version(version_bytes)?;

    Ok((method, path, query, version))
}

/// Validate that the method is a non-empty HTTP token (`1*tchar`) before converting.
fn parse_method(method_bytes: &[u8]) -> Result<Method, Error<'static>> {
    if method_bytes.is_empty() || !method_bytes.iter().copied().all(semantics::is_tchar) {
        return Err(Error::InvalidMethod);
    }

    let method_str = std::str::from_utf8(method_bytes).map_err(|_| Error::InvalidMethod)?;
    Ok(Method::from(method_str))
}

/// Validate `request-target` bytes and form in a single scan, returning `(path, query)`.
fn parse_target<'a>(
    target_bytes: &'a [u8],
    method: &Method,
) -> Result<(&'a str, &'a str), Error<'static>> {
    if target_bytes.is_empty() {
        return Err(Error::InvalidUri);
    }

    if target_bytes == b"*" {
        return if *method == Method::Options {
            Ok(("*", ""))
        } else {
            Err(Error::InvalidUri)
        };
    }

    let mut question_pos: Option<usize> = None;
    let mut scheme_sep: Option<usize> = None;
    let mut i = 0usize;

    while i < target_bytes.len() {
        let b = target_bytes[i];
        if !semantics::is_valid_uri_byte(b) {
            return Err(Error::InvalidUri);
        }
        if b == b'%' {
            if i + 2 >= target_bytes.len()
                || !target_bytes[i + 1].is_ascii_hexdigit()
                || !target_bytes[i + 2].is_ascii_hexdigit()
            {
                return Err(Error::InvalidUri);
            }
            i += 3;
            continue;
        }
        if b == b'?' && question_pos.is_none() {
            question_pos = Some(i);
        } else if b == b':'
            && scheme_sep.is_none()
            && question_pos.is_none()
            && i + 2 < target_bytes.len()
            && target_bytes[i + 1] == b'/'
            && target_bytes[i + 2] == b'/'
        {
            scheme_sep = Some(i);
        }
        i += 1;
    }

    let target = std::str::from_utf8(target_bytes).map_err(|_| Error::InvalidUri)?;

    if target_bytes[0] == b'/' {
        return Ok(split_at_question(target, question_pos));
    }

    if *method == Method::Connect {
        return if scheme_sep.is_none()
            && question_pos.is_none()
            && semantics::is_valid_authority_form(target)
        {
            Ok((target, ""))
        } else {
            Err(Error::InvalidUri)
        };
    }

    if let Some(colon) = scheme_sep {
        if !semantics::is_valid_scheme(&target_bytes[..colon]) {
            return Err(Error::InvalidUri);
        }

        let after_scheme_start = colon + 3;
        let query_end = question_pos.unwrap_or(target.len());
        if after_scheme_start >= query_end {
            return Err(Error::InvalidUri);
        }

        let after_scheme = &target[after_scheme_start..query_end];
        let (authority, path) = match semantics::find_byte(after_scheme.as_bytes(), b'/', 0) {
            Some(slash) => (&after_scheme[..slash], &after_scheme[slash..]),
            None => (after_scheme, "/"),
        };

        if !semantics::is_valid_host_value(authority) {
            return Err(Error::InvalidUri);
        }

        let query = match question_pos {
            Some(q) => &target[q + 1..],
            None => "",
        };
        return Ok((path, query));
    }

    Err(Error::InvalidUri)
}

#[inline]
fn split_at_question(target: &str, question_pos: Option<usize>) -> (&str, &str) {
    match question_pos {
        Some(q) => (&target[..q], &target[q + 1..]),
        None => (target, ""),
    }
}

/// Accept only `HTTP/1.0` and `HTTP/1.1` in the HTTP/1.x parser.
fn parse_version(version_bytes: &[u8]) -> Result<HttpVersion, Error<'static>> {
    let version_str = std::str::from_utf8(version_bytes).map_err(|_| Error::InvalidVersion)?;
    match HttpVersion::from(version_str) {
        HttpVersion::Http1_0 => Ok(HttpVersion::Http1_0),
        HttpVersion::Http1_1 => Ok(HttpVersion::Http1_1),
        HttpVersion::Http2 | HttpVersion::Http3 => Err(Error::UnsupportedProtocol),
        HttpVersion::Unknown => {
            // Syntactically valid `HTTP/x.y` versions outside 1.0/1.1 are unsupported protocols.
            if matches!(
                version_bytes,
                [b'H', b'T', b'T', b'P', b'/', b'0'..=b'9', b'.', b'0'..=b'9']
            ) {
                Err(Error::UnsupportedProtocol)
            } else {
                Err(Error::InvalidVersion)
            }
        }
    }
}

/// Parse and validate one header field line (`field-name ":" OWS field-value OWS`) per RFC 9112 §5.
pub(super) fn parse_header_line(line: &str) -> Result<Header<'_>, Error<'_>> {
    let Some(colon) = semantics::find_byte(line.as_bytes(), b':', 0) else {
        return Err(Error::InvalidHeader(Header {
            name: line,
            value: "",
        }));
    };

    let name = &line[..colon];
    let value = line[colon + 1..].trim_matches([' ', '\t']);

    if name.is_empty() || !name.bytes().all(semantics::is_tchar) {
        return Err(Error::InvalidHeader(Header { name, value }));
    }

    if !value.bytes().all(semantics::is_valid_header_value_byte) {
        return Err(Error::InvalidHeader(Header { name, value }));
    }

    Ok(Header { name, value })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::http::parser::MAX_HEADERS;

    fn parse_h1(buf: &[u8]) -> Result<Status<ParsedRequest<'_>>, Error<'_>> {
        H1::parse(buf)
    }

    #[test]
    fn parses_bodyless_request_without_headers_and_returns_exact_consumed() {
        let raw = b"GET / HTTP/1.0\r\n\r\n";
        let res = parse_h1(raw).expect("valid request");
        let Status::Complete(req, consumed) = res else {
            panic!("expected complete frame");
        };

        assert_eq!(req.method, Method::Get);
        assert_eq!(req.path, "/");
        assert_eq!(req.query, "");
        assert_eq!(req.version, HttpVersion::Http1_0);
        assert!(req.headers.is_empty());
        assert!(req.body.is_empty());
        assert_eq!(consumed, raw.len());
    }

    #[test]
    fn splits_query_populates_headers_and_preserves_pipelined_tail() {
        let head =
            b"GET /users?page=2&sort=asc HTTP/1.1\r\nHost: example.com\r\nAccept: */*\r\n\r\n";
        let mut raw = head.to_vec();
        raw.extend_from_slice(b"GET /next HTTP/1.1\r\n\r\n");

        let res = parse_h1(&raw).expect("valid request");
        let Status::Complete(req, consumed) = res else {
            panic!("expected complete frame");
        };

        assert_eq!(req.method, Method::Get);
        assert_eq!(req.path, "/users");
        assert_eq!(req.query, "page=2&sort=asc");
        assert_eq!(req.version, HttpVersion::Http1_1);
        assert_eq!(req.headers.len(), 2);
        assert_eq!(
            req.headers.as_slice(),
            &[
                Header {
                    name: "Host",
                    value: "example.com"
                },
                Header {
                    name: "Accept",
                    value: "*/*"
                },
            ]
        );
        assert_eq!(consumed, head.len());
    }

    #[test]
    fn returns_incomplete_on_fragmented_head() {
        assert_eq!(
            parse_h1(b"GET / HTTP/1.1\r\nHost: a\r\n\r"),
            Ok(Status::Incomplete)
        );
        assert_eq!(parse_h1(b"GET / HTTP/1.1\r"), Ok(Status::Incomplete));
        assert_eq!(parse_h1(b""), Ok(Status::Incomplete));
    }

    #[test]
    fn rejects_too_many_headers_instead_of_panicking() {
        let mut raw = b"GET / HTTP/1.1\r\nHost: a\r\n".to_vec();
        for i in 0..MAX_HEADERS {
            raw.extend_from_slice(format!("X-H{i}: 1\r\n").as_bytes());
        }
        raw.extend_from_slice(b"\r\n");
        assert_eq!(parse_h1(&raw), Err(Error::TooManyHeaders));
    }

    #[test]
    fn accepts_valid_target_forms_and_extension_methods() {
        let options = parse_h1(b"OPTIONS * HTTP/1.1\r\nHost: localhost\r\n\r\n").unwrap();
        let Status::Complete(req, _) = options else {
            panic!("expected complete");
        };
        assert_eq!(req.method, Method::Options);
        assert_eq!(req.path, "*");
        assert_eq!(req.query, "");

        let connect =
            parse_h1(b"CONNECT example.com:443 HTTP/1.1\r\nHost: example.com:443\r\n\r\n").unwrap();
        let Status::Complete(req, _) = connect else {
            panic!("expected complete");
        };
        assert_eq!(req.method, Method::Connect);
        assert_eq!(req.path, "example.com:443");

        let abs = parse_h1(b"GET http://example.com/v1?x=1%202 HTTP/1.0\r\n\r\n").unwrap();
        let Status::Complete(req, _) = abs else {
            panic!("expected complete");
        };
        assert_eq!(req.path, "/v1");
        assert_eq!(req.query, "x=1%202");
    }

    #[test]
    fn rejects_bare_cr_or_lf_in_head() {
        assert_eq!(
            parse_h1(b"GET / HTTP/1.1\n\r\n"),
            Err(Error::InvalidRequestLine)
        );
        assert_eq!(
            parse_h1(b"GET / HTTP/1.1\rX\r\n\r\n"),
            Err(Error::InvalidRequestLine)
        );
        assert!(matches!(
            parse_h1(b"GET / HTTP/1.1\r\nHost: a\n\r\n"),
            Err(Error::InvalidHeader(_))
        ));
    }

    #[test]
    fn rejects_invalid_request_lines_methods_uris_and_versions() {
        assert_eq!(parse_h1(b"\r\n\r\n"), Err(Error::InvalidRequestLine));
        assert_eq!(
            parse_h1(b"GET  / HTTP/1.1\r\n\r\n"),
            Err(Error::InvalidRequestLine)
        );
        assert_eq!(parse_h1(b"GET /\r\n\r\n"), Err(Error::InvalidRequestLine));
        assert_eq!(
            parse_h1(b"BAD\tMETHOD / HTTP/1.1\r\n\r\n"),
            Err(Error::InvalidMethod)
        );
        assert_eq!(
            parse_h1(b"GET /bad#frag HTTP/1.1\r\n\r\n"),
            Err(Error::InvalidUri)
        );
        assert_eq!(
            parse_h1(b"GET /bad%ZZ HTTP/1.1\r\n\r\n"),
            Err(Error::InvalidUri)
        );
        assert_eq!(parse_h1(b"GET * HTTP/1.1\r\n\r\n"), Err(Error::InvalidUri));
        assert_eq!(
            parse_h1(b"GET foo HTTP/1.1\r\n\r\n"),
            Err(Error::InvalidUri)
        );
        assert_eq!(
            parse_h1(b"GET / HTTP/2\r\n\r\n"),
            Err(Error::UnsupportedProtocol)
        );
        assert_eq!(
            parse_h1(b"GET / HTTP/2.0\r\n\r\n"),
            Err(Error::UnsupportedProtocol)
        );
        assert_eq!(
            parse_h1(b"GET / GARBAGE\r\n\r\n"),
            Err(Error::InvalidVersion)
        );
        assert_eq!(
            parse_h1(b"GET / http/1.1\r\n\r\n"),
            Err(Error::InvalidVersion)
        );
    }

    #[test]
    fn validates_absolute_form_authority_connect_ports_and_host_headers() {
        let Status::Complete(root_abs, _) =
            parse_h1(b"GET http://example.com?a=1 HTTP/1.1\r\nHost: example.com\r\n\r\n").unwrap()
        else {
            panic!("expected complete");
        };
        assert_eq!(root_abs.path, "/");
        assert_eq!(root_abs.query, "a=1");

        assert_eq!(
            parse_h1(b"GET http://user@example.com/p HTTP/1.1\r\nHost: example.com\r\n\r\n"),
            Err(Error::InvalidUri)
        );
        assert_eq!(
            parse_h1(b"GET http://example.com:0/p HTTP/1.1\r\nHost: example.com\r\n\r\n"),
            Err(Error::InvalidUri)
        );
        assert_eq!(
            parse_h1(b"CONNECT example.com:0 HTTP/1.1\r\nHost: example.com\r\n\r\n"),
            Err(Error::InvalidUri)
        );
        assert_eq!(
            parse_h1(b"CONNECT example.com:080 HTTP/1.1\r\nHost: example.com\r\n\r\n"),
            Err(Error::InvalidUri)
        );
        assert_eq!(
            parse_h1(b"CONNECT example.com:65536 HTTP/1.1\r\nHost: example.com\r\n\r\n"),
            Err(Error::InvalidUri)
        );
        assert_eq!(parse_h1(b"GET / HTTP/1.1\r\n\r\n"), Err(Error::MissingHost));
        assert_eq!(
            parse_h1(b"GET / HTTP/1.1\r\nHost: a\r\nHost: b\r\n\r\n"),
            Err(Error::InvalidHost)
        );
        assert_eq!(
            parse_h1(b"GET / HTTP/1.1\r\nHost: example.com:0\r\n\r\n"),
            Err(Error::InvalidHost)
        );
    }

    #[test]
    fn validates_ordered_transfer_encoding_and_rejects_smuggling() {
        let ok_ordered = b"POST / HTTP/1.1\r\nHost: localhost\r\nTransfer-Encoding: identity, chunked\r\n\r\n5\r\nhello\r\n0\r\n\r\n";
        let Status::Complete(req, consumed) = parse_h1(ok_ordered).unwrap() else {
            panic!("expected complete");
        };
        assert_eq!(req.body.as_ref(), b"hello");
        assert!(matches!(req.body, std::borrow::Cow::Borrowed(_)));
        assert_eq!(consumed, ok_ordered.len());

        for bad in [
            &b"POST / HTTP/1.1\r\nHost: a\r\nTransfer-Encoding: chunked, identity\r\n\r\n0\r\n\r\n"[..],
            &b"POST / HTTP/1.1\r\nHost: a\r\nTransfer-Encoding: chunked, chunked\r\n\r\n0\r\n\r\n"[..],
            &b"POST / HTTP/1.1\r\nHost: a\r\nTransfer-Encoding: xchunked\r\n\r\n0\r\n\r\n"[..],
            &b"POST / HTTP/1.1\r\nHost: a\r\nTransfer-Encoding: chunked\r\nContent-Length: 0\r\n\r\n0\r\n\r\n"[..],
            &b"POST / HTTP/1.0\r\nTransfer-Encoding: chunked\r\n\r\n0\r\n\r\n"[..],
        ] {
            assert!(matches!(parse_h1(bad), Err(Error::InvalidHeader(_))));
        }
    }

    #[test]
    fn validates_chunk_extensions_trailers_and_multi_chunk_decode() {
        let quoted_ext = b"POST / HTTP/1.1\r\nHost: a\r\nTransfer-Encoding: chunked\r\n\r\n5;ext=\"a\\\"b\";flag\r\nhello\r\n0\r\nX-Sum: 1\r\n\r\n";
        let Status::Complete(req, consumed) = parse_h1(quoted_ext).unwrap() else {
            panic!("expected complete");
        };
        assert_eq!(req.body.as_ref(), b"hello");
        assert_eq!(consumed, quoted_ext.len());

        // > 16 chunks exercises span cache spill path
        let mut many = b"POST / HTTP/1.1\r\nHost: a\r\nTransfer-Encoding: chunked\r\n\r\n".to_vec();
        for _ in 0..20 {
            many.extend_from_slice(b"1\r\nx\r\n");
        }
        many.extend_from_slice(b"0\r\n\r\n");
        let Status::Complete(many_req, many_consumed) = parse_h1(&many).unwrap() else {
            panic!("expected complete");
        };
        assert_eq!(many_req.body.as_ref(), &[b'x'; 20]);
        assert_eq!(many_consumed, many.len());

        for bad_ext in [
            &b"POST / HTTP/1.1\r\nHost: a\r\nTransfer-Encoding: chunked\r\n\r\n5;\r\nhello\r\n0\r\n\r\n"[..],
            &b"POST / HTTP/1.1\r\nHost: a\r\nTransfer-Encoding: chunked\r\n\r\n5;a=\r\nhello\r\n0\r\n\r\n"[..],
            &b"POST / HTTP/1.1\r\nHost: a\r\nTransfer-Encoding: chunked\r\n\r\n5;bad name=1\r\nhello\r\n0\r\n\r\n"[..],
        ] {
            assert_eq!(parse_h1(bad_ext), Err(Error::InvalidBody));
        }

        let mut oversized_ext =
            b"POST / HTTP/1.1\r\nHost: a\r\nTransfer-Encoding: chunked\r\n\r\n5;".to_vec();
        oversized_ext.extend_from_slice(&[b'a'; 1025]);
        oversized_ext.extend_from_slice(b"\r\nhello\r\n0\r\n\r\n");
        assert_eq!(parse_h1(&oversized_ext), Err(Error::InvalidBody));

        for forbidden_trailer in [
            &b"POST / HTTP/1.1\r\nHost: a\r\nTransfer-Encoding: chunked\r\n\r\n0\r\nAuthorization: Bearer x\r\n\r\n"[..],
            &b"POST / HTTP/1.1\r\nHost: a\r\nTransfer-Encoding: chunked\r\n\r\n0\r\nCookie: a=1\r\n\r\n"[..],
            &b"POST / HTTP/1.1\r\nHost: a\r\nTransfer-Encoding: chunked\r\n\r\n0\r\nContent-Type: text/plain\r\n\r\n"[..],
        ] {
            assert!(matches!(
                parse_h1(forbidden_trailer),
                Err(Error::InvalidHeader(_))
            ));
        }
    }

    #[test]
    fn incremental_cursor_parses_chunked_byte_by_byte() {
        let raw = b"POST /up?v=1 HTTP/1.1\r\nHost: localhost\r\nTransfer-Encoding: chunked\r\n\r\n5\r\nhello\r\n6\r\n world\r\n0\r\nX-Check: ok\r\n\r\n";
        let mut cursor = H1Cursor::new();

        for split in 0..raw.len() {
            assert_eq!(
                H1::parse_incremental(&raw[..split], &mut cursor),
                Ok(Status::Incomplete),
                "split at byte {split} must be incomplete"
            );
        }

        let Status::Complete(req, consumed) = H1::parse_incremental(raw, &mut cursor).unwrap()
        else {
            panic!("expected complete frame");
        };
        assert_eq!(req.path, "/up");
        assert_eq!(req.query, "v=1");
        assert_eq!(req.body.as_ref(), b"hello world");
        assert_eq!(consumed, raw.len());
    }

    #[test]
    fn enforces_max_headers_size_exact_boundary() {
        let prefix = b"GET / HTTP/1.1\r\nHost: localhost\r\nX-Pad: ";
        let suffix = b"\r\n\r\n";

        let exact_pad_len = MAX_HEADERS_SIZE - prefix.len() - suffix.len();
        let mut exact = Vec::with_capacity(MAX_HEADERS_SIZE + 5);
        exact.extend_from_slice(prefix);
        exact.extend(std::iter::repeat_n(b'a', exact_pad_len));
        exact.extend_from_slice(suffix);
        assert_eq!(exact.len(), MAX_HEADERS_SIZE);

        let Status::Complete(req, consumed) = H1::parse(&exact).expect("exact limit must pass")
        else {
            panic!("expected complete frame");
        };
        assert_eq!(consumed, MAX_HEADERS_SIZE);
        assert_eq!(req.headers.get("Host"), Some("localhost"));

        let mut cursor = H1Cursor::new();
        assert_eq!(
            H1::parse_incremental(&exact[..MAX_HEADERS_SIZE - 1], &mut cursor),
            Ok(Status::Incomplete)
        );
        assert!(matches!(
            H1::parse_incremental(&exact, &mut cursor),
            Ok(Status::Complete(_, c)) if c == MAX_HEADERS_SIZE
        ));

        let mut too_large = Vec::with_capacity(MAX_HEADERS_SIZE + 1);
        too_large.extend_from_slice(prefix);
        too_large.extend(std::iter::repeat_n(b'a', exact_pad_len + 1));
        too_large.extend_from_slice(suffix);
        assert_eq!(too_large.len(), MAX_HEADERS_SIZE + 1);
        assert_eq!(H1::parse(&too_large), Err(Error::HeadersTooLarge));

        let mut cursor_over = H1Cursor::new();
        assert_eq!(
            H1::parse_incremental(&too_large[..MAX_HEADERS_SIZE - 1], &mut cursor_over),
            Ok(Status::Incomplete)
        );
        assert_eq!(
            H1::parse_incremental(&too_large[..MAX_HEADERS_SIZE], &mut cursor_over),
            Err(Error::HeadersTooLarge)
        );
    }

    #[test]
    fn validates_expect_header_and_100_continue_cursor_state() {
        let head_only = b"POST /up HTTP/1.1\r\nHost: localhost\r\nExpect: 100-continue\r\nContent-Length: 5\r\n\r\n";
        let mut cursor = H1Cursor::new();
        assert_eq!(
            H1::parse_incremental(head_only, &mut cursor),
            Ok(Status::Incomplete)
        );
        assert!(cursor.take_expect_100_continue());
        assert!(!cursor.take_expect_100_continue());
        assert_eq!(cursor.expected_total_len(), Some(head_only.len() + 5));

        let mut full = head_only.to_vec();
        full.extend_from_slice(b"hello");
        let Status::Complete(req, consumed) = H1::parse_incremental(&full, &mut cursor).unwrap()
        else {
            panic!("expected complete frame");
        };
        assert_eq!(req.body.as_ref(), b"hello");
        assert_eq!(consumed, full.len());

        // If body arrived in the same buffer as headers, skip unsolicited 100 Continue.
        let partial_body = b"POST /up HTTP/1.1\r\nHost: localhost\r\nExpect: 100-continue\r\nContent-Length: 5\r\n\r\nhe";
        let mut cursor_same_buf = H1Cursor::new();
        assert_eq!(
            H1::parse_incremental(partial_body, &mut cursor_same_buf),
            Ok(Status::Incomplete)
        );
        assert!(!cursor_same_buf.take_expect_100_continue());

        // HTTP/1.0 accepts Expect: 100-continue without requesting 100 Continue.
        let http10 = b"POST /up HTTP/1.0\r\nExpect: 100-continue\r\nContent-Length: 5\r\n\r\n";
        let mut cursor_10 = H1Cursor::new();
        assert_eq!(
            H1::parse_incremental(http10, &mut cursor_10),
            Ok(Status::Incomplete)
        );
        assert!(!cursor_10.take_expect_100_continue());

        for bad_expect in [
            &b"POST /up HTTP/1.1\r\nHost: localhost\r\nExpect: bogus\r\nContent-Length: 5\r\n\r\n"
                [..],
            &b"POST /up HTTP/1.1\r\nHost: localhost\r\nExpect:\r\nContent-Length: 5\r\n\r\n"[..],
        ] {
            assert_eq!(H1::parse(bad_expect), Err(Error::ExpectationFailed));
        }
    }
}
