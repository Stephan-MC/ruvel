use super::headers::HeaderMap;
use std::collections::HashMap;

const MAX_HEADERS: usize = 64;

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum Method {
    GET,
    POST,
    PUT,
    DELETE,
    PATCH,
    UNKNOWN(String),
}

impl From<&str> for Method {
    fn from(s: &str) -> Self {
        match s {
            "GET" => Method::GET,
            "POST" => Method::POST,
            "PUT" => Method::PUT,
            "DELETE" => Method::DELETE,
            "PATCH" => Method::PATCH,
            other => Method::UNKNOWN(other.to_string()),
        }
    }
}

#[derive(Debug)]
pub struct Request {
    pub method: Method,
    /// Origin path. The query string is stored separately so routing can match this field.
    pub path: String,
    /// Raw query string without the leading `?`. Empty when the target has no query.
    pub query: String,
    pub headers: HeaderMap,
    pub body: Vec<u8>,
    pub params: HashMap<String, String>,
}

impl Request {
    pub fn new(method: Method, path: impl Into<String>) -> Self {
        Self {
            method,
            path: path.into(),
            query: String::new(),
            headers: HeaderMap::new(),
            body: Vec::new(),
            params: HashMap::new(),
        }
    }

    /// Parse one HTTP/1.x request from `buf` with `httparse`.
    ///
    /// `Ok(None)` means the headers or `Content-Length` body are not in `buf` yet.
    /// Nothing is allocated in that case. `Ok(Some((request, consumed)))` is one
    /// full request and how many bytes of `buf` it used. `Err(())` means these
    /// bytes cannot be a request this server accepts.
    pub fn parse(buf: &[u8]) -> Result<Option<(Self, usize)>, ()> {
        let mut headers = [httparse::EMPTY_HEADER; MAX_HEADERS];
        let mut parser = httparse::Request::new(&mut headers);

        let header_len = match parser.parse(buf) {
            Ok(httparse::Status::Complete(len)) => len,
            Ok(httparse::Status::Partial) => return Ok(None),
            Err(_) => return Err(()),
        };

        // Frame the body before allocating. A short read must be able to retry
        // this parse without having built a Request.
        let total = framed_len(header_len, parser.headers)?;
        if buf.len() < total {
            return Ok(None);
        }

        let target = parser.path.ok_or(())?;
        let (path, query) = split_target(target);

        let mut header_map = HeaderMap::new();
        for header in parser.headers.iter() {
            header_map.insert(
                header.name,
                String::from_utf8_lossy(header.value).into_owned(),
            );
        }

        let body = if total == header_len {
            Vec::new()
        } else {
            buf[header_len..total].to_vec()
        };

        Ok(Some((
            Self {
                method: Method::from(parser.method.ok_or(())?),
                path: path.to_string(),
                query: query.to_string(),
                headers: header_map,
                body,
                params: HashMap::new(),
            },
            total,
        )))
    }

    /// Retrieve a header value by key (case-insensitive).
    pub fn header(&self, key: &str) -> Option<&str> {
        self.headers.get(key)
    }

    /// Retrieve a URL parameter by key (e.g., req.param("id")).
    pub fn param(&self, key: &str) -> Option<&str> {
        self.params.get(key).map(|s| s.as_str())
    }
}

/// Byte length of one request: header block plus `Content-Length` body.
///
/// `httparse` does not decode bodies. A non-identity `Transfer-Encoding` is rejected
/// so a chunked body is not mistaken for the next request.
fn framed_len(header_len: usize, headers: &[httparse::Header<'_>]) -> Result<usize, ()> {
    let mut content_length: Option<usize> = None;

    for header in headers {
        if header.name.eq_ignore_ascii_case("content-length") {
            let parsed = parse_content_length(header.value)?;
            if let Some(existing) = content_length {
                if existing != parsed {
                    return Err(());
                }
            } else {
                content_length = Some(parsed);
            }
        } else if header.name.eq_ignore_ascii_case("transfer-encoding")
            && !header.value.trim_ascii().eq_ignore_ascii_case(b"identity")
        {
            return Err(());
        }
    }

    header_len
        .checked_add(content_length.unwrap_or(0))
        .ok_or(())
}

fn parse_content_length(value: &[u8]) -> Result<usize, ()> {
    let value = value.trim_ascii();
    if value.is_empty() {
        return Err(());
    }

    let mut length: usize = 0;
    for &byte in value {
        if !byte.is_ascii_digit() {
            return Err(());
        }
        length = length
            .checked_mul(10)
            .ok_or(())?
            .checked_add(usize::from(byte - b'0'))
            .ok_or(())?;
    }
    Ok(length)
}

fn split_target(target: &str) -> (&str, &str) {
    let (path, query) = target.split_once('?').unwrap_or((target, ""));
    if path.is_empty() {
        ("/", query)
    } else {
        (path, query)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse_ok(raw: &[u8]) -> (Request, usize) {
        Request::parse(raw).unwrap().expect("complete request")
    }

    #[test]
    fn parses_headers_without_allocating_a_body() {
        let raw = b"GET /users?active=1 HTTP/1.1\r\nHost: localhost\r\nX-Request-Id: abc\r\n\r\n";
        let (req, consumed) = parse_ok(raw);

        assert_eq!(consumed, raw.len());
        assert_eq!(req.method, Method::GET);
        assert_eq!(req.path, "/users");
        assert_eq!(req.query, "active=1");
        assert!(req.body.is_empty());
        assert_eq!(req.header("host"), Some("localhost"));
        assert_eq!(req.header("x-request-id"), Some("abc"));
    }

    #[test]
    fn waits_for_the_content_length_body_then_parses_once() {
        let headers = b"POST /items HTTP/1.1\r\nContent-Length: 5\r\n\r\n";
        assert!(Request::parse(headers).unwrap().is_none());

        let mut raw = headers.to_vec();
        raw.extend_from_slice(b"helloEXTRA");
        let (req, consumed) = parse_ok(&raw);

        assert_eq!(consumed, headers.len() + 5);
        assert_eq!(req.method, Method::POST);
        assert_eq!(req.path, "/items");
        assert_eq!(req.body, b"hello");
        assert_eq!(&raw[consumed..], b"EXTRA");
    }

    #[test]
    fn parses_one_request_out_of_a_pipeline() {
        let raw = b"GET /a HTTP/1.1\r\n\r\nGET /b?x=1 HTTP/1.1\r\n\r\n";
        let (first, consumed) = parse_ok(raw);
        let (second, _) = parse_ok(&raw[consumed..]);

        assert_eq!(first.path, "/a");
        assert_eq!(second.path, "/b");
        assert_eq!(second.query, "x=1");
    }

    #[test]
    fn rejects_conflicting_lengths_and_chunked_bodies() {
        let conflict = b"POST / HTTP/1.1\r\nContent-Length: 1\r\nContent-Length: 2\r\n\r\n";
        assert!(Request::parse(conflict).is_err());

        let chunked = b"POST / HTTP/1.1\r\nTransfer-Encoding: chunked\r\n\r\n";
        assert!(Request::parse(chunked).is_err());
    }

    #[test]
    fn partial_headers_allocate_nothing_and_return_none() {
        assert!(
            Request::parse(b"GET /users HTTP/1.1\r\nHost:")
                .unwrap()
                .is_none()
        );
    }
}
