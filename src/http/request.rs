use super::headers::HeaderMap;
use std::borrow::Cow;
use std::mem::MaybeUninit;

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

impl std::fmt::Display for Method {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Method::GET => f.write_str("GET"),
            Method::POST => f.write_str("POST"),
            Method::PUT => f.write_str("PUT"),
            Method::DELETE => f.write_str("DELETE"),
            Method::PATCH => f.write_str("PATCH"),
            Method::UNKNOWN(other) => f.write_str(other),
        }
    }
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
pub struct RouteParams {
    len: usize,
    entries: [MaybeUninit<(Cow<'static, str>, Cow<'static, str>)>; MAX_INLINE_PARAMS],
}

impl Default for RouteParams {
    fn default() -> Self {
        Self::new()
    }
}

impl Drop for RouteParams {
    fn drop(&mut self) {
        for i in 0..self.len {
            // SAFETY: `entries[..self.len]` are always initialized before `len` is incremented.
            unsafe { self.entries[i].assume_init_drop() };
        }
    }
}

impl Clone for RouteParams {
    fn clone(&self) -> Self {
        let mut out = Self::new();
        for i in 0..self.len {
            // SAFETY: `entries[i]` is initialized for all `i < self.len`.
            let cloned = unsafe { self.entries[i].assume_init_ref() }.clone();
            out.entries[i].write(cloned);
            out.len += 1;
        }
        out
    }
}

impl std::fmt::Debug for RouteParams {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RouteParams")
            .field("len", &self.len)
            .field("entries", &self.as_slice())
            .finish()
    }
}

impl RouteParams {
    pub const fn new() -> Self {
        Self {
            len: 0,
            entries: [const { MaybeUninit::uninit() }; MAX_INLINE_PARAMS],
        }
    }

    #[inline]
    fn as_slice(&self) -> &[(Cow<'static, str>, Cow<'static, str>)] {
        // SAFETY: `entries[..self.len]` are always initialized via `insert` or `clone`.
        unsafe { std::slice::from_raw_parts(self.entries.as_ptr().cast(), self.len) }
    }

    pub fn insert(
        &mut self,
        key: impl Into<Cow<'static, str>>,
        value: impl Into<Cow<'static, str>>,
    ) {
        let key_cow = key.into();
        let val_cow = value.into();

        for i in 0..self.len {
            // SAFETY: `entries[i]` is initialized for all `i < self.len`.
            let entry = unsafe { self.entries[i].assume_init_mut() };
            if entry.0 == key_cow {
                entry.1 = val_cow;
                return;
            }
        }

        if self.len < MAX_INLINE_PARAMS {
            self.entries[self.len].write((key_cow, val_cow));
            self.len += 1;
        }
    }

    pub fn get(&self, key: &str) -> Option<&str> {
        for (k, v) in self.as_slice() {
            if k == key {
                return Some(v.as_ref());
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
        let path = if path_str == "/" {
            Cow::Borrowed("/")
        } else {
            Cow::Owned(path_str.to_owned())
        };
        let query = if query_str.is_empty() {
            Cow::Borrowed("")
        } else {
            Cow::Owned(query_str.to_owned())
        };

        let mut header_map = HeaderMap::with_capacity(parser.headers.len());
        for header in parser.headers.iter() {
            let name = intern_header_name(header.name);
            let val = match std::str::from_utf8(header.value) {
                Ok(v) => intern_common_header_value(v),
                Err(_) => Cow::Owned(String::from_utf8_lossy(header.value).into_owned()),
            };
            header_map.insert(name, val);
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
            other => Method::UNKNOWN(other.to_owned()),
        };

        Ok(Some((
            Self {
                method,
                path,
                query,
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

/// Map common HTTP request header names to `'static` slices (case-insensitive) to avoid
/// per-header heap allocations while preserving RFC 9110 case-insensitive lookup.
#[inline]
fn intern_header_name(name: &str) -> Cow<'static, str> {
    let static_name: Option<&'static str> = match name.len() {
        2 if name.eq_ignore_ascii_case("te") => Some("TE"),
        4 => {
            if name.eq_ignore_ascii_case("host") {
                Some("Host")
            } else if name.eq_ignore_ascii_case("date") {
                Some("Date")
            } else if name.eq_ignore_ascii_case("from") {
                Some("From")
            } else {
                None
            }
        }
        5 if name.eq_ignore_ascii_case("range") => Some("Range"),
        6 => {
            if name.eq_ignore_ascii_case("accept") {
                Some("Accept")
            } else if name.eq_ignore_ascii_case("cookie") {
                Some("Cookie")
            } else if name.eq_ignore_ascii_case("origin") {
                Some("Origin")
            } else if name.eq_ignore_ascii_case("expect") {
                Some("Expect")
            } else if name.eq_ignore_ascii_case("pragma") {
                Some("Pragma")
            } else {
                None
            }
        }
        7 => {
            if name.eq_ignore_ascii_case("referer") {
                Some("Referer")
            } else if name.eq_ignore_ascii_case("upgrade") {
                Some("Upgrade")
            } else {
                None
            }
        }
        8 => {
            if name.eq_ignore_ascii_case("if-match") {
                Some("If-Match")
            } else if name.eq_ignore_ascii_case("if-range") {
                Some("If-Range")
            } else {
                None
            }
        }
        9 => {
            if name.eq_ignore_ascii_case("x-real-ip") {
                Some("X-Real-IP")
            } else if name.eq_ignore_ascii_case("forwarded") {
                Some("Forwarded")
            } else {
                None
            }
        }
        10 => {
            if name.eq_ignore_ascii_case("user-agent") {
                Some("User-Agent")
            } else if name.eq_ignore_ascii_case("connection") {
                Some("Connection")
            } else if name.eq_ignore_ascii_case("keep-alive") {
                Some("Keep-Alive")
            } else {
                None
            }
        }
        12 => {
            if name.eq_ignore_ascii_case("content-type") {
                Some("Content-Type")
            } else if name.eq_ignore_ascii_case("x-request-id") {
                Some("X-Request-Id")
            } else if name.eq_ignore_ascii_case("max-forwards") {
                Some("Max-Forwards")
            } else {
                None
            }
        }
        13 => {
            if name.eq_ignore_ascii_case("authorization") {
                Some("Authorization")
            } else if name.eq_ignore_ascii_case("cache-control") {
                Some("Cache-Control")
            } else if name.eq_ignore_ascii_case("if-none-match") {
                Some("If-None-Match")
            } else {
                None
            }
        }
        14 => {
            if name.eq_ignore_ascii_case("content-length") {
                Some("Content-Length")
            } else if name.eq_ignore_ascii_case("sec-fetch-mode") {
                Some("Sec-Fetch-Mode")
            } else if name.eq_ignore_ascii_case("sec-fetch-site") {
                Some("Sec-Fetch-Site")
            } else if name.eq_ignore_ascii_case("sec-fetch-dest") {
                Some("Sec-Fetch-Dest")
            } else if name.eq_ignore_ascii_case("sec-fetch-user") {
                Some("Sec-Fetch-User")
            } else {
                None
            }
        }
        15 => {
            if name.eq_ignore_ascii_case("accept-encoding") {
                Some("Accept-Encoding")
            } else if name.eq_ignore_ascii_case("accept-language") {
                Some("Accept-Language")
            } else if name.eq_ignore_ascii_case("x-forwarded-for") {
                Some("X-Forwarded-For")
            } else {
                None
            }
        }
        16 => {
            if name.eq_ignore_ascii_case("x-requested-with") {
                Some("X-Requested-With")
            } else if name.eq_ignore_ascii_case("x-forwarded-host") {
                Some("X-Forwarded-Host")
            } else if name.eq_ignore_ascii_case("content-encoding") {
                Some("Content-Encoding")
            } else if name.eq_ignore_ascii_case("proxy-connection") {
                Some("Proxy-Connection")
            } else {
                None
            }
        }
        17 => {
            if name.eq_ignore_ascii_case("if-modified-since") {
                Some("If-Modified-Since")
            } else if name.eq_ignore_ascii_case("transfer-encoding") {
                Some("Transfer-Encoding")
            } else if name.eq_ignore_ascii_case("x-forwarded-proto") {
                Some("X-Forwarded-Proto")
            } else {
                None
            }
        }
        19 if name.eq_ignore_ascii_case("if-unmodified-since") => Some("If-Unmodified-Since"),
        _ => None,
    };

    match static_name {
        Some(s) => Cow::Borrowed(s),
        None => Cow::Owned(name.to_owned()),
    }
}

/// Intern a small set of exact, high-frequency immutable header values to avoid allocation.
#[inline]
fn intern_common_header_value(value: &str) -> Cow<'static, str> {
    match value {
        "keep-alive" => Cow::Borrowed("keep-alive"),
        "close" => Cow::Borrowed("close"),
        "*/*" => Cow::Borrowed("*/*"),
        "application/json" => Cow::Borrowed("application/json"),
        "text/plain" => Cow::Borrowed("text/plain"),
        "no-cache" => Cow::Borrowed("no-cache"),
        "gzip, deflate, br" => Cow::Borrowed("gzip, deflate, br"),
        "gzip, deflate" => Cow::Borrowed("gzip, deflate"),
        "0" => Cow::Borrowed("0"),
        _ => Cow::Owned(value.to_owned()),
    }
}
