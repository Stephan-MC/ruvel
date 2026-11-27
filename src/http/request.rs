use crate::http::parser::{Status as ParseStatus, h1};
use crate::http::{Method, Status, Version};
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
    pub version: Version,
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
            version: Version::Http1_1,
            headers: Headers::new(),
            body: Vec::new(),
            params: Params::new(),
        }
    }

    /// Capture one complete HTTP/1.x request from the leading bytes of `buf`
    /// enforcing [`crate::constants::REQUEST_HEADER_SIZE`] and
    /// [`crate::constants::MAX_REQUEST_BODY_SIZE`].
    pub fn capture(buf: &[u8]) -> Result<ParseStatus<Self>, Status> {
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
    pub fn from_h1(raw: h1::Request<'_>) -> Result<Self, Status> {
        let mut req = Self::from_h1_head_only(&raw)?;
        let body = raw.body();
        if !body.is_empty() {
            req.body = body.to_vec();
        }
        Ok(req)
    }

    fn from_h1_head_only(raw: &h1::Request<'_>) -> Result<Self, Status> {
        let path_bytes = raw.path();
        let path = if path_bytes == b"/" {
            Cow::Borrowed("/")
        } else {
            let s = std::str::from_utf8(path_bytes).map_err(|_| Status::BadRequest)?;
            Cow::Owned(s.to_owned())
        };

        let query_bytes = raw.query();
        let query = if query_bytes.is_empty() {
            Cow::Borrowed("")
        } else {
            let s = std::str::from_utf8(query_bytes).map_err(|_| Status::BadRequest)?;
            Cow::Owned(s.to_owned())
        };

        let mut headers = Headers::with_capacity(raw.header_count());
        let mut seen_singletons: u16 = 0;
        let mut saw_unknown_singleton = false;
        for (name_bytes, value_bytes) in raw.headers() {
            let name_str = std::str::from_utf8(name_bytes).map_err(|_| Status::BadRequest)?;
            let value_str = std::str::from_utf8(value_bytes).map_err(|_| Status::BadRequest)?;
            let (name, kind) = intern_header_name(name_str);
            let value = intern_common_header_value(value_str);
            match kind {
                HeaderKind::Multi => headers.push_direct(name, value),
                HeaderKind::Single(bit) => {
                    if !saw_unknown_singleton && (seen_singletons & bit) == 0 {
                        seen_singletons |= bit;
                        headers.push_direct(name, value);
                    } else {
                        headers.insert(name, value);
                    }
                }
                HeaderKind::Unknown => {
                    if Headers::is_single_value(&name) {
                        saw_unknown_singleton = true;
                        headers.insert(name, value);
                    } else {
                        headers.push_direct(name, value);
                    }
                }
            }
        }

        Ok(Self {
            method: raw.method(),
            path,
            query,
            version: Version::from_h1_minor(raw.minor()),
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

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum HeaderKind {
    Multi,
    Single(u16),
    Unknown,
}

const SINGLE_HOST: u16 = 1 << 0;
const SINGLE_DATE: u16 = 1 << 1;
const SINGLE_FROM: u16 = 1 << 2;
const SINGLE_RANGE: u16 = 1 << 3;
const SINGLE_ORIGIN: u16 = 1 << 4;
const SINGLE_REFERER: u16 = 1 << 5;
const SINGLE_IF_RANGE: u16 = 1 << 6;
const SINGLE_USER_AGENT: u16 = 1 << 7;
const SINGLE_CONTENT_TYPE: u16 = 1 << 8;
const SINGLE_MAX_FORWARDS: u16 = 1 << 9;
const SINGLE_AUTHORIZATION: u16 = 1 << 10;
const SINGLE_CONTENT_LENGTH: u16 = 1 << 11;
const SINGLE_IF_MODIFIED_SINCE: u16 = 1 << 12;
const SINGLE_IF_UNMODIFIED_SINCE: u16 = 1 << 13;

/// Intern common exact-cased HTTP request header names to `'static` slices without
/// altering wire casing, also returning pre-classified singleton metadata so
/// `from_h1_head_only` avoids redundant linear duplicate scans on common headers.
#[inline]
fn intern_header_name(name: &str) -> (Cow<'static, str>, HeaderKind) {
    match name {
        "TE" => (Cow::Borrowed("TE"), HeaderKind::Multi),
        "te" => (Cow::Borrowed("te"), HeaderKind::Multi),
        "Host" => (Cow::Borrowed("Host"), HeaderKind::Single(SINGLE_HOST)),
        "host" => (Cow::Borrowed("host"), HeaderKind::Single(SINGLE_HOST)),
        "Date" => (Cow::Borrowed("Date"), HeaderKind::Single(SINGLE_DATE)),
        "date" => (Cow::Borrowed("date"), HeaderKind::Single(SINGLE_DATE)),
        "From" => (Cow::Borrowed("From"), HeaderKind::Single(SINGLE_FROM)),
        "from" => (Cow::Borrowed("from"), HeaderKind::Single(SINGLE_FROM)),
        "Range" => (Cow::Borrowed("Range"), HeaderKind::Single(SINGLE_RANGE)),
        "range" => (Cow::Borrowed("range"), HeaderKind::Single(SINGLE_RANGE)),
        "Accept" => (Cow::Borrowed("Accept"), HeaderKind::Multi),
        "accept" => (Cow::Borrowed("accept"), HeaderKind::Multi),
        "Cookie" => (Cow::Borrowed("Cookie"), HeaderKind::Multi),
        "cookie" => (Cow::Borrowed("cookie"), HeaderKind::Multi),
        "Origin" => (Cow::Borrowed("Origin"), HeaderKind::Single(SINGLE_ORIGIN)),
        "origin" => (Cow::Borrowed("origin"), HeaderKind::Single(SINGLE_ORIGIN)),
        "Expect" => (Cow::Borrowed("Expect"), HeaderKind::Multi),
        "expect" => (Cow::Borrowed("expect"), HeaderKind::Multi),
        "Pragma" => (Cow::Borrowed("Pragma"), HeaderKind::Multi),
        "pragma" => (Cow::Borrowed("pragma"), HeaderKind::Multi),
        "Referer" => (Cow::Borrowed("Referer"), HeaderKind::Single(SINGLE_REFERER)),
        "referer" => (Cow::Borrowed("referer"), HeaderKind::Single(SINGLE_REFERER)),
        "Upgrade" => (Cow::Borrowed("Upgrade"), HeaderKind::Multi),
        "upgrade" => (Cow::Borrowed("upgrade"), HeaderKind::Multi),
        "If-Match" => (Cow::Borrowed("If-Match"), HeaderKind::Multi),
        "if-match" => (Cow::Borrowed("if-match"), HeaderKind::Multi),
        "If-Range" => (
            Cow::Borrowed("If-Range"),
            HeaderKind::Single(SINGLE_IF_RANGE),
        ),
        "if-range" => (
            Cow::Borrowed("if-range"),
            HeaderKind::Single(SINGLE_IF_RANGE),
        ),
        "X-Real-IP" => (Cow::Borrowed("X-Real-IP"), HeaderKind::Multi),
        "x-real-ip" => (Cow::Borrowed("x-real-ip"), HeaderKind::Multi),
        "Forwarded" => (Cow::Borrowed("Forwarded"), HeaderKind::Multi),
        "forwarded" => (Cow::Borrowed("forwarded"), HeaderKind::Multi),
        "User-Agent" => (
            Cow::Borrowed("User-Agent"),
            HeaderKind::Single(SINGLE_USER_AGENT),
        ),
        "user-agent" => (
            Cow::Borrowed("user-agent"),
            HeaderKind::Single(SINGLE_USER_AGENT),
        ),
        "Connection" => (Cow::Borrowed("Connection"), HeaderKind::Multi),
        "connection" => (Cow::Borrowed("connection"), HeaderKind::Multi),
        "Keep-Alive" => (Cow::Borrowed("Keep-Alive"), HeaderKind::Multi),
        "keep-alive" => (Cow::Borrowed("keep-alive"), HeaderKind::Multi),
        "Content-Type" => (
            Cow::Borrowed("Content-Type"),
            HeaderKind::Single(SINGLE_CONTENT_TYPE),
        ),
        "content-type" => (
            Cow::Borrowed("content-type"),
            HeaderKind::Single(SINGLE_CONTENT_TYPE),
        ),
        "X-Request-Id" => (Cow::Borrowed("X-Request-Id"), HeaderKind::Multi),
        "x-request-id" => (Cow::Borrowed("x-request-id"), HeaderKind::Multi),
        "Max-Forwards" => (
            Cow::Borrowed("Max-Forwards"),
            HeaderKind::Single(SINGLE_MAX_FORWARDS),
        ),
        "max-forwards" => (
            Cow::Borrowed("max-forwards"),
            HeaderKind::Single(SINGLE_MAX_FORWARDS),
        ),
        "Authorization" => (
            Cow::Borrowed("Authorization"),
            HeaderKind::Single(SINGLE_AUTHORIZATION),
        ),
        "authorization" => (
            Cow::Borrowed("authorization"),
            HeaderKind::Single(SINGLE_AUTHORIZATION),
        ),
        "Cache-Control" => (Cow::Borrowed("Cache-Control"), HeaderKind::Multi),
        "cache-control" => (Cow::Borrowed("cache-control"), HeaderKind::Multi),
        "If-None-Match" => (Cow::Borrowed("If-None-Match"), HeaderKind::Multi),
        "if-none-match" => (Cow::Borrowed("if-none-match"), HeaderKind::Multi),
        "Content-Length" => (
            Cow::Borrowed("Content-Length"),
            HeaderKind::Single(SINGLE_CONTENT_LENGTH),
        ),
        "content-length" => (
            Cow::Borrowed("content-length"),
            HeaderKind::Single(SINGLE_CONTENT_LENGTH),
        ),
        "Sec-Fetch-Mode" => (Cow::Borrowed("Sec-Fetch-Mode"), HeaderKind::Multi),
        "sec-fetch-mode" => (Cow::Borrowed("sec-fetch-mode"), HeaderKind::Multi),
        "Sec-Fetch-Site" => (Cow::Borrowed("Sec-Fetch-Site"), HeaderKind::Multi),
        "sec-fetch-site" => (Cow::Borrowed("sec-fetch-site"), HeaderKind::Multi),
        "Sec-Fetch-Dest" => (Cow::Borrowed("Sec-Fetch-Dest"), HeaderKind::Multi),
        "sec-fetch-dest" => (Cow::Borrowed("sec-fetch-dest"), HeaderKind::Multi),
        "Sec-Fetch-User" => (Cow::Borrowed("Sec-Fetch-User"), HeaderKind::Multi),
        "sec-fetch-user" => (Cow::Borrowed("sec-fetch-user"), HeaderKind::Multi),
        "Accept-Encoding" => (Cow::Borrowed("Accept-Encoding"), HeaderKind::Multi),
        "accept-encoding" => (Cow::Borrowed("accept-encoding"), HeaderKind::Multi),
        "Accept-Language" => (Cow::Borrowed("Accept-Language"), HeaderKind::Multi),
        "accept-language" => (Cow::Borrowed("accept-language"), HeaderKind::Multi),
        "X-Forwarded-For" => (Cow::Borrowed("X-Forwarded-For"), HeaderKind::Multi),
        "x-forwarded-for" => (Cow::Borrowed("x-forwarded-for"), HeaderKind::Multi),
        "X-Requested-With" => (Cow::Borrowed("X-Requested-With"), HeaderKind::Multi),
        "x-requested-with" => (Cow::Borrowed("x-requested-with"), HeaderKind::Multi),
        "X-Forwarded-Host" => (Cow::Borrowed("X-Forwarded-Host"), HeaderKind::Multi),
        "x-forwarded-host" => (Cow::Borrowed("x-forwarded-host"), HeaderKind::Multi),
        "Content-Encoding" => (Cow::Borrowed("Content-Encoding"), HeaderKind::Multi),
        "content-encoding" => (Cow::Borrowed("content-encoding"), HeaderKind::Multi),
        "Proxy-Connection" => (Cow::Borrowed("Proxy-Connection"), HeaderKind::Multi),
        "proxy-connection" => (Cow::Borrowed("proxy-connection"), HeaderKind::Multi),
        "If-Modified-Since" => (
            Cow::Borrowed("If-Modified-Since"),
            HeaderKind::Single(SINGLE_IF_MODIFIED_SINCE),
        ),
        "if-modified-since" => (
            Cow::Borrowed("if-modified-since"),
            HeaderKind::Single(SINGLE_IF_MODIFIED_SINCE),
        ),
        "Transfer-Encoding" => (Cow::Borrowed("Transfer-Encoding"), HeaderKind::Multi),
        "transfer-encoding" => (Cow::Borrowed("transfer-encoding"), HeaderKind::Multi),
        "X-Forwarded-Proto" => (Cow::Borrowed("X-Forwarded-Proto"), HeaderKind::Multi),
        "x-forwarded-proto" => (Cow::Borrowed("x-forwarded-proto"), HeaderKind::Multi),
        "If-Unmodified-Since" => (
            Cow::Borrowed("If-Unmodified-Since"),
            HeaderKind::Single(SINGLE_IF_UNMODIFIED_SINCE),
        ),
        "if-unmodified-since" => (
            Cow::Borrowed("if-unmodified-since"),
            HeaderKind::Single(SINGLE_IF_UNMODIFIED_SINCE),
        ),
        _ => (Cow::Owned(name.to_owned()), HeaderKind::Unknown),
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn interned_header_kinds_match_single_value_rules_and_have_unique_bits() {
        let interned_names = [
            ("TE", "te"),
            ("Host", "host"),
            ("Date", "date"),
            ("From", "from"),
            ("Range", "range"),
            ("Accept", "accept"),
            ("Cookie", "cookie"),
            ("Origin", "origin"),
            ("Expect", "expect"),
            ("Pragma", "pragma"),
            ("Referer", "referer"),
            ("Upgrade", "upgrade"),
            ("If-Match", "if-match"),
            ("If-Range", "if-range"),
            ("X-Real-IP", "x-real-ip"),
            ("Forwarded", "forwarded"),
            ("User-Agent", "user-agent"),
            ("Connection", "connection"),
            ("Keep-Alive", "keep-alive"),
            ("Content-Type", "content-type"),
            ("X-Request-Id", "x-request-id"),
            ("Max-Forwards", "max-forwards"),
            ("Authorization", "authorization"),
            ("Cache-Control", "cache-control"),
            ("If-None-Match", "if-none-match"),
            ("Content-Length", "content-length"),
            ("Sec-Fetch-Mode", "sec-fetch-mode"),
            ("Sec-Fetch-Site", "sec-fetch-site"),
            ("Sec-Fetch-Dest", "sec-fetch-dest"),
            ("Sec-Fetch-User", "sec-fetch-user"),
            ("Accept-Encoding", "accept-encoding"),
            ("Accept-Language", "accept-language"),
            ("X-Forwarded-For", "x-forwarded-for"),
            ("X-Requested-With", "x-requested-with"),
            ("X-Forwarded-Host", "x-forwarded-host"),
            ("Content-Encoding", "content-encoding"),
            ("Proxy-Connection", "proxy-connection"),
            ("If-Modified-Since", "if-modified-since"),
            ("Transfer-Encoding", "transfer-encoding"),
            ("X-Forwarded-Proto", "x-forwarded-proto"),
            ("If-Unmodified-Since", "if-unmodified-since"),
        ];

        let mut seen_bits = 0u16;
        let mut singleton_count = 0usize;

        for (title, lower) in interned_names {
            let (cow_t, kind_t) = intern_header_name(title);
            let (cow_l, kind_l) = intern_header_name(lower);
            assert!(
                matches!(cow_t, Cow::Borrowed(_)),
                "{title} must be borrowed"
            );
            assert!(
                matches!(cow_l, Cow::Borrowed(_)),
                "{lower} must be borrowed"
            );
            assert_eq!(kind_t, kind_l, "pair ({title}, {lower}) kind mismatch");
            assert_ne!(kind_t, HeaderKind::Unknown, "{title} must not be Unknown");

            let is_single = Headers::is_single_value(title);
            assert_eq!(is_single, Headers::is_single_value(lower));
            match kind_t {
                HeaderKind::Multi => assert!(!is_single, "{title} should be Single"),
                HeaderKind::Single(bit) => {
                    assert!(is_single, "{title} should be Multi");
                    assert_eq!(bit.count_ones(), 1, "{title} bit {bit:#x} not single-bit");
                    assert_eq!(
                        seen_bits & bit,
                        0,
                        "{title} collides with an existing singleton bit {bit:#x}"
                    );
                    seen_bits |= bit;
                    singleton_count += 1;
                }
                HeaderKind::Unknown => unreachable!(),
            }
        }

        assert_eq!(singleton_count, 14);
        assert_eq!(intern_header_name("X-Unlisted").1, HeaderKind::Unknown);
    }

    #[test]
    fn duplicate_singleton_headers_in_request_overwrite_in_place() {
        for raw in [
            &b"GET / HTTP/1.1\r\nHost: localhost\r\nUser-Agent: first\r\nuser-agent: second\r\n\r\n"[..],
            &b"GET / HTTP/1.1\r\nHost: localhost\r\nUSER-AGENT: first\r\nUser-Agent: second\r\n\r\n"[..],
            &b"GET / HTTP/1.1\r\nHost: localhost\r\nUser-Agent: first\r\nUSER-AGENT: second\r\n\r\n"[..],
        ] {
            let ParseStatus::Complete(req, _) = Request::capture(raw).unwrap() else {
                panic!("expected complete");
            };
            assert_eq!(req.headers.get_all("user-agent").collect::<Vec<_>>(), vec!["second"]);
            assert_eq!(req.headers().len(), 2);
        }
    }
}
