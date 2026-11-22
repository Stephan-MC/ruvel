use crate::http::Method;
use crate::http::parser::{self, HttpVersion, Status as ParseStatus, h1};
use crate::routing::route::Params;

use super::headers::Headers;
use std::borrow::Cow;

#[derive(Debug)]
pub struct Request {
    pub method: Method,
    /// Origin path. The query string is stored separately so routing can match this field.
    pub path: Cow<'static, str>,
    /// Raw query string without the leading `?`. Empty when the target has no query.
    pub query: Cow<'static, str>,
    /// Wire HTTP protocol version negotiated/parsed for this request.
    pub version: HttpVersion,
    pub headers: Headers,
    pub body: Vec<u8>,
    pub params: Params,
}

impl Request {
    pub fn new(method: Method, path: impl Into<Cow<'static, str>>) -> Self {
        Self {
            method,
            path: path.into(),
            query: Cow::Borrowed(""),
            version: HttpVersion::Http1_1,
            headers: Headers::new(),
            body: Vec::new(),
            params: Params::new(),
        }
    }

    /// Capture one complete HTTP/1.x request from the leading bytes of `buf`
    /// enforcing [`crate::constants::REQUEST_HEADER_SIZE`] and
    /// [`crate::constants::MAX_REQUEST_BODY_SIZE`].
    pub fn capture(buf: &[u8]) -> Result<ParseStatus<Self>, parser::Error> {
        let mut head = h1::Head::new();
        match h1::parse_slice_into(buf, &mut head)? {
            ParseStatus::Complete((head_len, body), consumed) => {
                let raw = h1::Request::from_parts(&head, &buf[..head_len], &body);
                let mut req = Self::from_h1_head_only(&raw)?;
                req.body = match body {
                    Cow::Borrowed([]) => Vec::new(),
                    Cow::Borrowed(b) => b.to_vec(),
                    Cow::Owned(v) => v,
                };
                Ok(ParseStatus::Complete(req, consumed))
            }
            ParseStatus::Incomplete => Ok(ParseStatus::Incomplete),
        }
    }

    /// Materialize an owned [`Request`] from a zero-copy borrowed [`h1::Request`] view.
    pub fn from_h1(raw: h1::Request<'_>) -> Result<Self, parser::Error> {
        let mut req = Self::from_h1_head_only(&raw)?;
        let body = raw.body();
        if !body.is_empty() {
            req.body = body.to_vec();
        }
        Ok(req)
    }

    fn from_h1_head_only(raw: &h1::Request<'_>) -> Result<Self, parser::Error> {
        let path_bytes = raw.path();
        let path = if path_bytes == b"/" {
            Cow::Borrowed("/")
        } else {
            let s = std::str::from_utf8(path_bytes).map_err(|_| parser::Error::Malformed)?;
            Cow::Owned(s.to_owned())
        };

        let query_bytes = raw.query();
        let query = if query_bytes.is_empty() {
            Cow::Borrowed("")
        } else {
            let s = std::str::from_utf8(query_bytes).map_err(|_| parser::Error::Malformed)?;
            Cow::Owned(s.to_owned())
        };

        let mut headers = Headers::with_capacity(raw.header_count());
        for (name_bytes, value_bytes) in raw.headers() {
            let name_str = std::str::from_utf8(name_bytes).map_err(|_| parser::Error::Malformed)?;
            let value_str =
                std::str::from_utf8(value_bytes).map_err(|_| parser::Error::Malformed)?;
            let name = intern_header_name(name_str);
            let value = intern_common_header_value(value_str);
            headers.insert(name, value);
        }

        Ok(Self {
            method: raw.method(),
            path,
            query,
            version: HttpVersion::from_h1_minor(raw.minor()),
            headers,
            body: Vec::new(),
            params: Params::new(),
        })
    }

    /// Retrieve a header value by key (case-insensitive).
    pub fn header(&self, key: &str) -> Option<&str> {
        self.headers.get(key)
    }

    /// Retrieve all headers as a slice of `(original_key, value)` pairs in insertion order.
    #[inline]
    pub fn headers(&self) -> &[(Cow<'static, str>, Cow<'static, str>)] {
        self.headers.all()
    }

    /// Retrieve a URL parameter by key (e.g., req.param("id")).
    pub fn param(&self, key: &str) -> Option<&str> {
        self.params.get(key)
    }

    /// Retrieve an input value by key from the request body or query string.
    ///
    /// Checks parsed `application/json` objects or `application/x-www-form-urlencoded`
    /// form payloads first, then falls back to URL query parameters (`self.query`).
    /// Route parameters remain separate via [`Self::param`].
    pub fn input(&self, key: &str) -> Option<serde_json::Value> {
        if !self.body.is_empty()
            && let Some(content_type) = self.header("Content-Type")
        {
            let media_type = content_type_media_type(content_type);
            if is_json_media_type(media_type) {
                if let Ok(serde_json::Value::Object(mut map)) =
                    serde_json::from_slice::<serde_json::Value>(&self.body)
                    && let Some(val) = map.remove(key)
                {
                    return Some(val);
                }
            } else if media_type.eq_ignore_ascii_case("application/x-www-form-urlencoded")
                && let Ok(form) = std::str::from_utf8(&self.body)
                && let Some(val) = lookup_form_urlencoded(form, key)
            {
                return Some(serde_json::Value::String(val));
            }
        }

        if !self.query.is_empty()
            && let Some(val) = lookup_form_urlencoded(&self.query, key)
        {
            return Some(serde_json::Value::String(val));
        }

        None
    }

    fn parse_unsigned<T>(&self, key: &str) -> Option<T>
    where
        T: TryFrom<u64> + std::str::FromStr,
    {
        match self.input(key)? {
            serde_json::Value::Number(n) => n.as_u64().and_then(|v| T::try_from(v).ok()),
            serde_json::Value::String(s) => s.trim().parse::<T>().ok(),
            _ => None,
        }
    }

    fn parse_signed<T>(&self, key: &str) -> Option<T>
    where
        T: TryFrom<i64> + std::str::FromStr,
    {
        match self.input(key)? {
            serde_json::Value::Number(n) => n.as_i64().and_then(|v| T::try_from(v).ok()),
            serde_json::Value::String(s) => s.trim().parse::<T>().ok(),
            _ => None,
        }
    }

    /// Retrieve an input value parsed as `u8`, or `None` if missing or invalid.
    pub fn u8(&self, key: &str) -> Option<u8> {
        self.parse_unsigned(key)
    }

    /// Retrieve an input value parsed as `u16`, or `None` if missing or invalid.
    pub fn u16(&self, key: &str) -> Option<u16> {
        self.parse_unsigned(key)
    }

    /// Retrieve an input value parsed as `u32`, or `None` if missing or invalid.
    pub fn u32(&self, key: &str) -> Option<u32> {
        self.parse_unsigned(key)
    }

    /// Retrieve an input value parsed as `u64`, or `None` if missing or invalid.
    pub fn u64(&self, key: &str) -> Option<u64> {
        self.parse_unsigned(key)
    }

    /// Retrieve an input value parsed as `usize`, or `None` if missing or invalid.
    pub fn usize(&self, key: &str) -> Option<usize> {
        self.parse_unsigned(key)
    }

    /// Retrieve an input value parsed as `i8`, or `None` if missing or invalid.
    pub fn i8(&self, key: &str) -> Option<i8> {
        self.parse_signed(key)
    }

    /// Retrieve an input value parsed as `i16`, or `None` if missing or invalid.
    pub fn i16(&self, key: &str) -> Option<i16> {
        self.parse_signed(key)
    }

    /// Retrieve an input value parsed as `i32`, or `None` if missing or invalid.
    pub fn i32(&self, key: &str) -> Option<i32> {
        self.parse_signed(key)
    }

    /// Retrieve an input value parsed as `i64`, or `None` if missing or invalid.
    pub fn i64(&self, key: &str) -> Option<i64> {
        self.parse_signed(key)
    }

    /// Retrieve an input value parsed as `isize`, or `None` if missing or invalid.
    pub fn isize(&self, key: &str) -> Option<isize> {
        self.parse_signed(key)
    }

    /// Retrieve an input value parsed as finite `f32`, or `None` if missing or invalid.
    pub fn f32(&self, key: &str) -> Option<f32> {
        let val = match self.input(key)? {
            serde_json::Value::Number(n) => n.as_f64()? as f32,
            serde_json::Value::String(s) => s.trim().parse::<f32>().ok()?,
            _ => return None,
        };
        val.is_finite().then_some(val)
    }

    /// Retrieve an input value parsed as finite `f64`, or `None` if missing or invalid.
    pub fn f64(&self, key: &str) -> Option<f64> {
        let val = match self.input(key)? {
            serde_json::Value::Number(n) => n.as_f64()?,
            serde_json::Value::String(s) => s.trim().parse::<f64>().ok()?,
            _ => return None,
        };
        val.is_finite().then_some(val)
    }

    /// Retrieve an input value as `bool`, or `None` if missing or not a boolean representation.
    pub fn bool(&self, key: &str) -> Option<bool> {
        match self.input(key)? {
            serde_json::Value::Bool(b) => Some(b),
            serde_json::Value::String(s) => {
                let trimmed = s.trim();
                if trimmed.eq_ignore_ascii_case("true")
                    || trimmed == "1"
                    || trimmed.eq_ignore_ascii_case("yes")
                    || trimmed.eq_ignore_ascii_case("on")
                {
                    Some(true)
                } else if trimmed.eq_ignore_ascii_case("false")
                    || trimmed == "0"
                    || trimmed.eq_ignore_ascii_case("no")
                    || trimmed.eq_ignore_ascii_case("off")
                {
                    Some(false)
                } else {
                    None
                }
            }
            _ => None,
        }
    }

    /// Retrieve an input value as `String`, or `None` if missing or not a string value.
    pub fn string(&self, key: &str) -> Option<String> {
        match self.input(key)? {
            serde_json::Value::String(s) => Some(s),
            _ => None,
        }
    }
}

/// Intern common exact-cased HTTP request header names to `'static` slices without
/// altering wire casing.
#[inline]
fn intern_header_name(name: &str) -> Cow<'static, str> {
    let static_name: Option<&'static str> = match name {
        "TE" => Some("TE"),
        "te" => Some("te"),
        "Host" => Some("Host"),
        "host" => Some("host"),
        "Date" => Some("Date"),
        "date" => Some("date"),
        "From" => Some("From"),
        "from" => Some("from"),
        "Range" => Some("Range"),
        "range" => Some("range"),
        "Accept" => Some("Accept"),
        "accept" => Some("accept"),
        "Cookie" => Some("Cookie"),
        "cookie" => Some("cookie"),
        "Origin" => Some("Origin"),
        "origin" => Some("origin"),
        "Expect" => Some("Expect"),
        "expect" => Some("expect"),
        "Pragma" => Some("Pragma"),
        "pragma" => Some("pragma"),
        "Referer" => Some("Referer"),
        "referer" => Some("referer"),
        "Upgrade" => Some("Upgrade"),
        "upgrade" => Some("upgrade"),
        "If-Match" => Some("If-Match"),
        "if-match" => Some("if-match"),
        "If-Range" => Some("If-Range"),
        "if-range" => Some("if-range"),
        "X-Real-IP" => Some("X-Real-IP"),
        "x-real-ip" => Some("x-real-ip"),
        "Forwarded" => Some("Forwarded"),
        "forwarded" => Some("forwarded"),
        "User-Agent" => Some("User-Agent"),
        "user-agent" => Some("user-agent"),
        "Connection" => Some("Connection"),
        "connection" => Some("connection"),
        "Keep-Alive" => Some("Keep-Alive"),
        "keep-alive" => Some("keep-alive"),
        "Content-Type" => Some("Content-Type"),
        "content-type" => Some("content-type"),
        "X-Request-Id" => Some("X-Request-Id"),
        "x-request-id" => Some("x-request-id"),
        "Max-Forwards" => Some("Max-Forwards"),
        "max-forwards" => Some("max-forwards"),
        "Authorization" => Some("Authorization"),
        "authorization" => Some("authorization"),
        "Cache-Control" => Some("Cache-Control"),
        "cache-control" => Some("cache-control"),
        "If-None-Match" => Some("If-None-Match"),
        "if-none-match" => Some("if-none-match"),
        "Content-Length" => Some("Content-Length"),
        "content-length" => Some("content-length"),
        "Sec-Fetch-Mode" => Some("Sec-Fetch-Mode"),
        "sec-fetch-mode" => Some("sec-fetch-mode"),
        "Sec-Fetch-Site" => Some("Sec-Fetch-Site"),
        "sec-fetch-site" => Some("sec-fetch-site"),
        "Sec-Fetch-Dest" => Some("Sec-Fetch-Dest"),
        "sec-fetch-dest" => Some("sec-fetch-dest"),
        "Sec-Fetch-User" => Some("Sec-Fetch-User"),
        "sec-fetch-user" => Some("sec-fetch-user"),
        "Accept-Encoding" => Some("Accept-Encoding"),
        "accept-encoding" => Some("accept-encoding"),
        "Accept-Language" => Some("Accept-Language"),
        "accept-language" => Some("accept-language"),
        "X-Forwarded-For" => Some("X-Forwarded-For"),
        "x-forwarded-for" => Some("x-forwarded-for"),
        "X-Requested-With" => Some("X-Requested-With"),
        "x-requested-with" => Some("x-requested-with"),
        "X-Forwarded-Host" => Some("X-Forwarded-Host"),
        "x-forwarded-host" => Some("x-forwarded-host"),
        "Content-Encoding" => Some("Content-Encoding"),
        "content-encoding" => Some("content-encoding"),
        "Proxy-Connection" => Some("Proxy-Connection"),
        "proxy-connection" => Some("proxy-connection"),
        "If-Modified-Since" => Some("If-Modified-Since"),
        "if-modified-since" => Some("if-modified-since"),
        "Transfer-Encoding" => Some("Transfer-Encoding"),
        "transfer-encoding" => Some("transfer-encoding"),
        "X-Forwarded-Proto" => Some("X-Forwarded-Proto"),
        "x-forwarded-proto" => Some("x-forwarded-proto"),
        "If-Unmodified-Since" => Some("If-Unmodified-Since"),
        "if-unmodified-since" => Some("if-unmodified-since"),
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

#[inline]
fn content_type_media_type(header: &str) -> &str {
    header.split(';').next().unwrap_or(header).trim()
}

#[inline]
fn is_json_media_type(media_type: &str) -> bool {
    let bytes = media_type.as_bytes();
    media_type.eq_ignore_ascii_case("application/json")
        || (bytes.len() > b"application/+json".len()
            && bytes[..12].eq_ignore_ascii_case(b"application/")
            && bytes[bytes.len() - 5..].eq_ignore_ascii_case(b"+json"))
}

fn lookup_form_urlencoded(raw: &str, key: &str) -> Option<String> {
    let mut matched: Option<&str> = None;

    for pair in raw.split('&') {
        if pair.is_empty() {
            continue;
        }
        let (raw_key, raw_val) = pair.split_once('=').unwrap_or((pair, ""));
        if raw_key.is_empty() {
            continue;
        }

        let is_match = if !raw_key.as_bytes().iter().any(|&b| b == b'%' || b == b'+') {
            raw_key == key
        } else {
            percent_decode(raw_key) == key
        };

        if is_match {
            matched = Some(raw_val);
        }
    }

    matched.map(percent_decode)
}

#[inline]
fn hex_nibble(byte: u8) -> Option<u8> {
    match byte {
        b'0'..=b'9' => Some(byte - b'0'),
        b'a'..=b'f' => Some(byte - b'a' + 10),
        b'A'..=b'F' => Some(byte - b'A' + 10),
        _ => None,
    }
}

fn percent_decode(input: &str) -> String {
    let bytes = input.as_bytes();
    if !bytes.iter().any(|&b| b == b'%' || b == b'+') {
        return input.to_owned();
    }

    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            b'+' => {
                out.push(b' ');
                i += 1;
            }
            b'%' if i + 2 < bytes.len() => {
                if let (Some(hi), Some(lo)) = (hex_nibble(bytes[i + 1]), hex_nibble(bytes[i + 2])) {
                    out.push((hi << 4) | lo);
                    i += 3;
                } else {
                    out.push(b'%');
                    i += 1;
                }
            }
            other => {
                out.push(other);
                i += 1;
            }
        }
    }

    String::from_utf8(out).unwrap_or_else(|e| String::from_utf8_lossy(e.as_bytes()).into_owned())
}
