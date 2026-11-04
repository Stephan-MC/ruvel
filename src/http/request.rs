use super::headers::HeaderMap;
use std::borrow::Cow;

const MAX_HEADERS: usize = 64;
const MAX_INLINE_PARAMS: usize = 8;

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

/// Fixed-capacity, inline route parameter storage to avoid heap HashMap allocations per match.
#[derive(Debug, Clone, Default)]
pub struct RouteParams {
    len: usize,
    entries: [(Cow<'static, str>, Cow<'static, str>); MAX_INLINE_PARAMS],
}

impl RouteParams {
    pub const fn new() -> Self {
        const EMPTY: (Cow<'static, str>, Cow<'static, str>) = (Cow::Borrowed(""), Cow::Borrowed(""));
        Self {
            len: 0,
            entries: [EMPTY; MAX_INLINE_PARAMS],
        }
    }

    pub fn insert(&mut self, key: impl Into<Cow<'static, str>>, value: impl Into<Cow<'static, str>>) {
        let key_cow = key.into();
        let val_cow = value.into();

        for i in 0..self.len {
            if self.entries[i].0 == key_cow {
                self.entries[i].1 = val_cow;
                return;
            }
        }

        if self.len < MAX_INLINE_PARAMS {
            self.entries[self.len] = (key_cow, val_cow);
            self.len += 1;
        }
    }

    pub fn get(&self, key: &str) -> Option<&str> {
        for i in 0..self.len {
            if self.entries[i].0 == key {
                return Some(self.entries[i].1.as_ref());
            }
        }
        None
    }

    pub fn len(&self) -> usize {
        self.len
    }

    pub fn is_empty(&self) -> bool {
        self.len == 0
    }
}

#[derive(Debug)]
pub struct Request {
    pub method: Method,
    /// Origin path. The query string is stored separately so routing can match this field.
    pub path: Cow<'static, str>,
    /// Raw query string without the leading `?`. Empty when the target has no query.
    pub query: Cow<'static, str>,
    pub headers: HeaderMap,
    pub body: Vec<u8>,
    pub params: RouteParams,
}

impl Request {
    pub fn new(method: Method, path: impl Into<Cow<'static, str>>) -> Self {
        Self {
            method,
            path: path.into(),
            query: Cow::Borrowed(""),
            headers: HeaderMap::new(),
            body: Vec::new(),
            params: RouteParams::new(),
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
        let (path_str, query_str) = split_target(target);

        let mut header_map = HeaderMap::with_capacity(parser.headers.len());
        for header in parser.headers.iter() {
            let val = match std::str::from_utf8(header.value) {
                Ok(v) => Cow::Owned(v.to_string()),
                Err(_) => Cow::Owned(String::from_utf8_lossy(header.value).into_owned()),
            };
            header_map.insert(Cow::Owned(header.name.to_string()), val);
        }

        let body = if total == header_len {
            Vec::new()
        } else {
            buf[header_len..total].to_vec()
        };

        let method = match parser.method.ok_or(())? {
            "GET" => Method::GET,
            "POST" => Method::POST,
            "PUT" => Method::PUT,
            "DELETE" => Method::DELETE,
            "PATCH" => Method::PATCH,
            other => Method::UNKNOWN(other.to_string()),
        };

        Ok(Some((
            Self {
                method,
                path: Cow::Owned(path_str.to_string()),
                query: Cow::Owned(query_str.to_string()),
                headers: header_map,
                body,
                params: RouteParams::new(),
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
        self.params.get(key)
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
