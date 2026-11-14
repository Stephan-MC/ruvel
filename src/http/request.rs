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
    HEAD,
    OPTIONS,
    TRACE,
    CONNECT,
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
            Method::HEAD => f.write_str("HEAD"),
            Method::OPTIONS => f.write_str("OPTIONS"),
            Method::TRACE => f.write_str("TRACE"),
            Method::CONNECT => f.write_str("CONNECT"),
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
            "HEAD" => Method::HEAD,
            "OPTIONS" => Method::OPTIONS,
            "TRACE" => Method::TRACE,
            "CONNECT" => Method::CONNECT,
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

    /// Parse one complete HTTP/1.x request from the leading bytes of `buf`.
    ///
    /// Designed for streaming TCP reads and HTTP/1.1 pipelining:
    /// - `Ok(None)`: headers or body (`Content-Length` or `Transfer-Encoding: chunked`)
    ///   are still fragmented / incomplete in `buf`. Performs **zero heap allocations**
    ///   so the caller can keep appending TCP bytes and retry safely across arbitrary
    ///   split boundaries (including 1-byte reads).
    /// - `Ok(Some((request, consumed)))`: one complete request and the exact wire byte
    ///   count consumed (`header_len + framed_body_len`). Any bytes in `&buf[consumed..]`
    ///   belong to subsequent pipelined requests on the same connection.
    /// - `Err(())`: syntactically malformed request line, headers, conflicting/unsupported
    ///   framing (`Transfer-Encoding` + `Content-Length`, conflicting `Content-Length`,
    ///   unknown transfer-coding), or malformed chunked body syntax.
    pub fn parse(buf: &[u8]) -> Result<Option<(Self, usize)>, ()> {
        let mut headers = [httparse::EMPTY_HEADER; MAX_HEADERS];
        let mut parser = httparse::Request::new(&mut headers);

        let header_len = match parser.parse(buf) {
            Ok(httparse::Status::Complete(len)) => len,
            Ok(httparse::Status::Partial) => return Ok(None),
            Err(_) => return Err(()),
        };

        // Frame and validate the body before allocating any `Request` fields.
        // A fragmented read returns `Ok(None)` here with zero allocations.
        let Some(framing) = frame_body(header_len, parser.headers, buf)? else {
            return Ok(None);
        };

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

        let (body, consumed) = match framing {
            BodyFraming::ContentLength { total_len } => {
                let body = if total_len == header_len {
                    Vec::new()
                } else {
                    buf[header_len..total_len].to_vec()
                };
                (body, total_len)
            }
            BodyFraming::Chunked {
                total_len,
                decoded_len,
            } => (
                extract_chunked_body(&buf[header_len..total_len], decoded_len),
                total_len,
            ),
        };

        let method = Method::from(parser.method.ok_or(())?);

        Ok(Some((
            Self {
                method,
                path,
                query,
                headers: header_map,
                body,
                params: RouteParams::new(),
            },
            consumed,
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

/// Determined message framing for a parsed HTTP/1.x request header block.
enum BodyFraming {
    /// Fixed-length body framed by `Content-Length` (or zero-length when absent).
    ContentLength { total_len: usize },
    /// `Transfer-Encoding: chunked` body with total wire byte span
    /// (`total_len = header_len + chunked_wire_len`) and decoded payload length (`decoded_len`).
    Chunked {
        total_len: usize,
        decoded_len: usize,
    },
}

/// Determine how the request body is framed from its parsed headers (RFC 9112 Sections 6.1, 6.3, 7.1).
///
/// Validates framing headers against request-smuggling hazards before checking payload completeness:
/// - Rejects duplicate `Content-Length` headers whose numeric values disagree.
/// - Rejects multiple `Transfer-Encoding` headers or any coding other than `chunked` or `identity`.
/// - Rejects requests specifying both `Transfer-Encoding: chunked` and `Content-Length` (RFC 9112 §6.3).
///
/// Returns `Ok(None)` without heap allocation if the body bytes in `buf` are still incomplete.
fn frame_body(
    header_len: usize,
    headers: &[httparse::Header<'_>],
    buf: &[u8],
) -> Result<Option<BodyFraming>, ()> {
    let mut content_length: Option<usize> = None;
    let mut is_chunked = false;
    let mut has_transfer_encoding = false;

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
        } else if header.name.eq_ignore_ascii_case("transfer-encoding") {
            if has_transfer_encoding {
                return Err(());
            }
            has_transfer_encoding = true;

            let coding = header.value.trim_ascii();
            if coding.eq_ignore_ascii_case(b"chunked") {
                is_chunked = true;
            } else if !coding.eq_ignore_ascii_case(b"identity") {
                return Err(());
            }
        }
    }

    if is_chunked {
        // RFC 9112 §6.3: A message containing both Transfer-Encoding: chunked and Content-Length is invalid.
        if content_length.is_some() {
            return Err(());
        }
        return match frame_chunked_body(&buf[header_len..])? {
            Some((chunked_wire_len, decoded_len)) => {
                let total_len = header_len.checked_add(chunked_wire_len).ok_or(())?;
                Ok(Some(BodyFraming::Chunked {
                    total_len,
                    decoded_len,
                }))
            }
            None => Ok(None),
        };
    }

    let total_len = header_len
        .checked_add(content_length.unwrap_or(0))
        .ok_or(())?;
    if buf.len() < total_len {
        return Ok(None);
    }

    Ok(Some(BodyFraming::ContentLength { total_len }))
}

/// First zero-allocation pass over a `Transfer-Encoding: chunked` body stream (RFC 9112 Section 7.1).
///
/// Walks every chunk (`<hex-size>[;ext]\r\n<data>\r\n ... 0[;ext]\r\n[trailers]\r\n`) to:
/// - Return `Ok(None)` when `src` ends mid-chunk-size, mid-extension, mid-CRLF, mid-data, or mid-trailer.
/// - Return `Err(())` immediately on invalid hex sizes, bare LF, missing data CRLF, overflow, or malformed trailers.
/// - Return `Ok(Some((wire_consumed, decoded_len)))` once the terminating empty line completes.
fn frame_chunked_body(src: &[u8]) -> Result<Option<(usize, usize)>, ()> {
    let mut pos = 0usize;
    let mut decoded_len = 0usize;

    loop {
        let Some((chunk_size, data_start)) = parse_chunk_size_line(src, pos)? else {
            return Ok(None);
        };

        if chunk_size == 0 {
            let Some(wire_end) = consume_chunked_trailers(src, data_start)? else {
                return Ok(None);
            };
            return Ok(Some((wire_end, decoded_len)));
        }

        let data_end = data_start.checked_add(chunk_size).ok_or(())?;
        let chunk_end = data_end.checked_add(2).ok_or(())?;
        if src.len() < chunk_end {
            // Fail fast if a non-CR byte immediately follows the chunk payload before EOF.
            if src.len() > data_end && src[data_end] != b'\r' {
                return Err(());
            }
            return Ok(None);
        }
        if src[data_end] != b'\r' || src[data_end + 1] != b'\n' {
            return Err(());
        }

        decoded_len = decoded_len.checked_add(chunk_size).ok_or(())?;
        pos = chunk_end;
    }
}

/// Parse one `<hex-size>[;chunk-ext]\r\n` line starting at `pos`.
///
/// Returns `Ok(Some((chunk_size, pos_after_crlf)))`, `Ok(None)` if truncated mid-line,
/// or `Err(())` on non-hex/empty size or bare LF.
fn parse_chunk_size_line(src: &[u8], mut pos: usize) -> Result<Option<(usize, usize)>, ()> {
    let mut chunk_size = 0usize;
    let mut has_digits = false;

    while pos < src.len() {
        let Some(nibble) = hex_nibble(src[pos]) else {
            break;
        };
        has_digits = true;
        chunk_size = chunk_size
            .checked_mul(16)
            .ok_or(())?
            .checked_add(usize::from(nibble))
            .ok_or(())?;
        pos += 1;
    }

    if !has_digits {
        return if pos == src.len() { Ok(None) } else { Err(()) };
    }
    if pos == src.len() {
        return Ok(None);
    }

    // Optional chunk extension / BWS (`;` or linear whitespace) up to `\r\n`.
    if matches!(src[pos], b';' | b' ' | b'\t') {
        while pos < src.len() && src[pos] != b'\r' {
            if src[pos] == b'\n' {
                return Err(());
            }
            pos += 1;
        }
        if pos == src.len() {
            return Ok(None);
        }
    }

    if src[pos] != b'\r' {
        return Err(());
    }
    pos += 1;
    if pos == src.len() {
        return Ok(None);
    }
    if src[pos] != b'\n' {
        return Err(());
    }

    Ok(Some((chunk_size, pos + 1)))
}

/// Consume zero or more optional HTTP/1.1 trailer field lines following a `0`-size
/// chunk and the final empty `\r\n` terminator (RFC 9112 Section 7.1.2).
fn consume_chunked_trailers(src: &[u8], mut pos: usize) -> Result<Option<usize>, ()> {
    loop {
        if pos == src.len() {
            return Ok(None);
        }
        if src[pos] == b'\r' {
            if pos + 1 == src.len() {
                return Ok(None);
            }
            if src[pos + 1] != b'\n' {
                return Err(());
            }
            return Ok(Some(pos + 2));
        }

        let mut has_colon = false;
        while pos < src.len() && src[pos] != b'\r' {
            if src[pos] == b'\n' {
                return Err(());
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
            return Err(());
        }
        if pos + 1 == src.len() {
            return Ok(None);
        }
        if src[pos + 1] != b'\n' {
            return Err(());
        }
        pos += 2;
    }
}

/// Second pass over an already-validated chunked wire slice, assembling all chunk data slices
/// into a single `Vec<u8>` pre-sized to `decoded_len`.
fn extract_chunked_body(src: &[u8], decoded_len: usize) -> Vec<u8> {
    if decoded_len == 0 {
        return Vec::new();
    }

    let mut out = Vec::with_capacity(decoded_len);
    let mut pos = 0usize;

    while let Ok(Some((chunk_size, data_start))) = parse_chunk_size_line(src, pos) {
        if chunk_size == 0 {
            break;
        }
        let data_end = data_start + chunk_size;
        out.extend_from_slice(&src[data_start..data_end]);
        pos = data_end + 2;
    }

    out
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
