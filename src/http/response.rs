use super::headers::Headers;
use super::status::Status;
use crate::http::Responsable;
use std::borrow::Cow;

#[derive(Debug, Clone)]
pub struct Response {
    /// The HTTP status (code + message) of the response.
    pub status: Status,
    /// Case-insensitive HTTP response headers.
    pub headers: Headers,
    pub body: Vec<u8>,
}

// 1. Identity implementation for Response
impl Responsable for Response {
    fn to_response(self) -> Response {
        self
    }
}

impl Response {
    // 1xx Informational constants
    pub const HTTP_CONTINUE: u16 = 100;
    pub const HTTP_SWITCHING_PROTOCOLS: u16 = 101;
    pub const HTTP_PROCESSING: u16 = 102;
    pub const HTTP_EARLY_HINTS: u16 = 103;

    // 2xx Success constants
    pub const HTTP_OK: u16 = 200;
    pub const HTTP_CREATED: u16 = 201;
    pub const HTTP_ACCEPTED: u16 = 202;
    pub const HTTP_NON_AUTHORITATIVE_INFORMATION: u16 = 203;
    pub const HTTP_NO_CONTENT: u16 = 204;
    pub const HTTP_RESET_CONTENT: u16 = 205;
    pub const HTTP_PARTIAL_CONTENT: u16 = 206;

    // 3xx Redirection constants
    pub const HTTP_MULTIPLE_CHOICES: u16 = 300;
    pub const HTTP_MOVED_PERMANENTLY: u16 = 301;
    pub const HTTP_FOUND: u16 = 302;
    pub const HTTP_SEE_OTHER: u16 = 303;
    pub const HTTP_NOT_MODIFIED: u16 = 304;
    pub const HTTP_TEMPORARY_REDIRECT: u16 = 307;
    pub const HTTP_PERMANENTLY_REDIRECT: u16 = 308;

    // 4xx Client Errors constants
    pub const HTTP_BAD_REQUEST: u16 = 400;
    pub const HTTP_UNAUTHORIZED: u16 = 401;
    pub const HTTP_PAYMENT_REQUIRED: u16 = 402;
    pub const HTTP_FORBIDDEN: u16 = 403;
    pub const HTTP_NOT_FOUND: u16 = 404;
    pub const HTTP_METHOD_NOT_ALLOWED: u16 = 405;
    pub const HTTP_NOT_ACCEPTABLE: u16 = 406;
    pub const HTTP_PROXY_AUTHENTICATION_REQUIRED: u16 = 407;
    pub const HTTP_REQUEST_TIMEOUT: u16 = 408;
    pub const HTTP_CONFLICT: u16 = 409;
    pub const HTTP_GONE: u16 = 410;
    pub const HTTP_LENGTH_REQUIRED: u16 = 411;
    pub const HTTP_PRECONDITION_FAILED: u16 = 412;
    pub const HTTP_PAYLOAD_TOO_LARGE: u16 = 413;
    pub const HTTP_URI_TOO_LONG: u16 = 414;
    pub const HTTP_UNSUPPORTED_MEDIA_TYPE: u16 = 415;
    pub const HTTP_RANGE_NOT_SATISFIABLE: u16 = 416;
    pub const HTTP_EXPECTATION_FAILED: u16 = 417;
    pub const HTTP_I_AM_A_TEAPOT: u16 = 418;
    pub const HTTP_UNPROCESSABLE_ENTITY: u16 = 422;
    pub const HTTP_LOCKED: u16 = 423;
    pub const HTTP_FAILED_DEPENDENCY: u16 = 424;
    pub const HTTP_TOO_EARLY: u16 = 425;
    pub const HTTP_UPGRADE_REQUIRED: u16 = 426;
    pub const HTTP_PRECONDITION_REQUIRED: u16 = 428;
    pub const HTTP_TOO_MANY_REQUESTS: u16 = 429;
    pub const HTTP_REQUEST_HEADER_FIELDS_TOO_LARGE: u16 = 431;
    pub const HTTP_UNAVAILABLE_FOR_LEGAL_REASONS: u16 = 451;

    // 5xx Server Errors constants
    pub const HTTP_INTERNAL_SERVER_ERROR: u16 = 500;
    pub const HTTP_NOT_IMPLEMENTED: u16 = 501;
    pub const HTTP_BAD_GATEWAY: u16 = 502;
    pub const HTTP_SERVICE_UNAVAILABLE: u16 = 503;
    pub const HTTP_GATEWAY_TIMEOUT: u16 = 504;
    pub const HTTP_VERSION_NOT_SUPPORTED: u16 = 505;

    /// Create a new Response initialized from an [`Status`].
    /// Defaults to `Content-Type: text/html; charset=utf-8`.
    pub fn new(status: Status) -> Self {
        Self {
            status,
            headers: Headers::with_single("Content-Type", "text/html; charset=utf-8"),
            body: Vec::new(),
        }
    }

    /// Internal fast-path constructor with an explicit `Content-Type` and pre-built body.
    pub(crate) fn with_body(status: Status, content_type: &'static str, body: Vec<u8>) -> Self {
        Self {
            status,
            headers: Headers::with_single("Content-Type", content_type),
            body,
        }
    }

    /// Create a new Response initialized from a numeric status code.
    /// Returns [`None`] if the code is not a recognized HTTP status code.
    pub fn from_code(code: u16) -> Option<Self> {
        Status::from_code(code).map(Self::new)
    }

    /// Convenience constructor with [`Status`].
    pub fn status(status: Status) -> Self {
        Self::new(status)
    }

    pub fn ok() -> Self {
        Self::new(Status::Ok)
    }

    pub fn created() -> Self {
        Self::new(Status::Created)
    }

    pub fn no_content() -> Self {
        Self::new(Status::NoContent)
    }

    pub fn bad_request() -> Self {
        Self::new(Status::BadRequest)
    }

    pub fn not_found() -> Self {
        Self::new(Status::NotFound)
    }

    pub fn not_implemented() -> Self {
        Self::new(Status::NotImplemented)
    }

    pub fn method_not_allowed() -> Self {
        Self::new(Status::MethodNotAllowed)
    }

    /// Helper to get the status code as u16.
    pub fn status_code(&self) -> u16 {
        self.status.code()
    }

    /// Helper to get the status message as &'static str.
    pub fn status_message(&self) -> &'static str {
        self.status.message()
    }

    /// Set or change the response status.
    pub fn set_status(&mut self, status: Status) -> &mut Self {
        self.status = status;
        self
    }

    /// Insert a header key and value (case-insensitive key).
    pub fn header(
        mut self,
        key: impl Into<Cow<'static, str>>,
        value: impl Into<Cow<'static, str>>,
    ) -> Self {
        self.headers.insert(key, value);
        self
    }

    /// Set response body as plain text (`Content-Type: text/plain; charset=utf-8`).
    pub fn text(mut self, body: impl Into<String>) -> Self {
        self.headers
            .insert("Content-Type", "text/plain; charset=utf-8");
        self.body = body.into().into_bytes();
        self
    }

    /// Set response body as JSON (`Content-Type: application/json`).
    pub fn json(mut self, json_str: impl Into<String>) -> Self {
        self.headers.insert("Content-Type", "application/json");
        self.body = json_str.into().into_bytes();
        self
    }

    /// Set response body as HTML (`Content-Type: text/html; charset=utf-8`).
    pub fn html(mut self, html_str: impl Into<String>) -> Self {
        self.headers
            .insert("Content-Type", "text/html; charset=utf-8");
        self.body = html_str.into().into_bytes();
        self
    }

    /// Set response body as raw binary bytes (`Content-Type: application/octet-stream`).
    pub fn bytes(mut self, body: impl Into<Vec<u8>>) -> Self {
        self.headers
            .insert("Content-Type", "application/octet-stream");
        self.body = body.into();
        self
    }

    /// Set response body bytes without altering the `Content-Type` header.
    pub fn body(mut self, body: impl Into<Vec<u8>>) -> Self {
        self.body = body.into();
        self
    }

    /// Serialize this response into a new buffer.
    pub fn to_bytes(&self) -> Vec<u8> {
        self.to_bytes_for(false)
    }

    /// Serialize this response into a new buffer, suppressing body bytes for `HEAD` requests.
    pub(crate) fn to_bytes_for(&self, is_head: bool) -> Vec<u8> {
        let cap = if is_head { 128 } else { 128 + self.body.len() };
        let mut buffer = Vec::with_capacity(cap);
        self.write_to_for(&mut buffer, is_head);
        buffer
    }

    /// Fast response serialization avoiding `std::fmt::write!` format string parsing
    /// and dynamic formatter dispatch.
    pub fn write_to(&self, buffer: &mut Vec<u8>) {
        self.write_to_for(buffer, false);
    }

    /// Fast response serialization supporting `HEAD` body suppression.
    pub(crate) fn write_to_for(&self, buffer: &mut Vec<u8>, is_head: bool) {
        // Reserve enough space up front to avoid multiple growth reallocations on fresh buffers.
        let extra = if is_head { 96 } else { 96 + self.body.len() };
        buffer.reserve(extra);
        let _ = self.write_to_sink(buffer, is_head);
    }

    /// Serialize this response directly into a fixed byte slice without heap allocation.
    /// Returns `Some(bytes_written)` if the serialized response fits in `dst`, or `None` otherwise.
    #[allow(dead_code)]
    pub(crate) fn write_to_slice(&self, dst: &mut [u8]) -> Option<usize> {
        self.write_to_slice_for(dst, false)
    }

    /// Serialize this response directly into a fixed byte slice without heap allocation,
    /// suppressing body bytes for `HEAD` requests.
    pub(crate) fn write_to_slice_for(&self, dst: &mut [u8], is_head: bool) -> Option<usize> {
        let mut sink = SliceSink { dst, pos: 0 };
        self.write_to_sink(&mut sink, is_head).then_some(sink.pos)
    }

    #[inline]
    fn write_to_sink<S: ByteSink>(&self, sink: &mut S, is_head: bool) -> bool {
        let static_status: Option<&[u8]> = match self.status {
            Status::Ok => Some(b"HTTP/1.1 200 OK\r\n"),
            Status::Created => Some(b"HTTP/1.1 201 Created\r\n"),
            Status::Accepted => Some(b"HTTP/1.1 202 Accepted\r\n"),
            Status::NoContent => Some(b"HTTP/1.1 204 No Content\r\n"),
            Status::BadRequest => Some(b"HTTP/1.1 400 Bad Request\r\n"),
            Status::Unauthorized => Some(b"HTTP/1.1 401 Unauthorized\r\n"),
            Status::Forbidden => Some(b"HTTP/1.1 403 Forbidden\r\n"),
            Status::NotFound => Some(b"HTTP/1.1 404 Not Found\r\n"),
            Status::MethodNotAllowed => Some(b"HTTP/1.1 405 Method Not Allowed\r\n"),
            Status::InternalServerError => Some(b"HTTP/1.1 500 Internal Server Error\r\n"),
            Status::NotImplemented => Some(b"HTTP/1.1 501 Not Implemented\r\n"),
            Status::BadGateway => Some(b"HTTP/1.1 502 Bad Gateway\r\n"),
            Status::ServiceUnavailable => Some(b"HTTP/1.1 503 Service Unavailable\r\n"),
            _ => None,
        };

        let mut num_buf = [0u8; 20];
        if let Some(status_line) = static_status {
            if !sink.write_bytes(status_line) {
                return false;
            }
        } else if !sink.write_bytes(b"HTTP/1.1 ")
            || !sink.write_bytes(format_usize_ascii(
                self.status_code() as usize,
                &mut num_buf,
            ))
            || !sink.write_bytes(b" ")
            || !sink.write_bytes(self.status_message().as_bytes())
            || !sink.write_bytes(b"\r\n")
        {
            return false;
        }

        let code = self.status_code();
        let forbid_content_length = (100..200).contains(&code) || code == 204;
        let suppress_body = is_head || forbid_content_length || code == 304;
        let preserve_manual_cl = !forbid_content_length && (is_head || code == 304);
        let has_manual_cl = preserve_manual_cl && self.headers.contains_key("Content-Length");

        if !forbid_content_length
            && !has_manual_cl
            && (!sink.write_bytes(b"Content-Length: ")
                || !sink.write_bytes(format_usize_ascii(self.body.len(), &mut num_buf))
                || !sink.write_bytes(b"\r\n"))
        {
            return false;
        }

        let skip_header_cl = forbid_content_length || !has_manual_cl;
        let mut emitted_manual_cl = false;
        for (k, v) in self.headers.iter() {
            if k.eq_ignore_ascii_case("content-length") {
                if skip_header_cl || emitted_manual_cl {
                    continue;
                }
                emitted_manual_cl = true;
            }
            if !sink.write_bytes(k.as_bytes())
                || !sink.write_bytes(b": ")
                || !sink.write_bytes(v.as_bytes())
                || !sink.write_bytes(b"\r\n")
            {
                return false;
            }
        }

        if !sink.write_bytes(b"\r\n") {
            return false;
        }
        if suppress_body {
            true
        } else {
            sink.write_bytes(&self.body)
        }
    }

    // TODO: Send the response to the client
    pub fn send(self) {
        //TODO: Add date to be sent to every response's header
        self.header("Date", "");
    }
}

trait ByteSink {
    fn write_bytes(&mut self, bytes: &[u8]) -> bool;
}

impl ByteSink for Vec<u8> {
    #[inline]
    fn write_bytes(&mut self, bytes: &[u8]) -> bool {
        self.extend_from_slice(bytes);
        true
    }
}

struct SliceSink<'a> {
    dst: &'a mut [u8],
    pos: usize,
}

impl ByteSink for SliceSink<'_> {
    #[inline]
    fn write_bytes(&mut self, bytes: &[u8]) -> bool {
        let Some(end) = self.pos.checked_add(bytes.len()) else {
            return false;
        };
        if end > self.dst.len() {
            return false;
        }
        self.dst[self.pos..end].copy_from_slice(bytes);
        self.pos = end;
        true
    }
}

/// Zero-allocation, fast usize to ASCII formatting into a fixed stack buffer.
#[inline]
fn format_usize_ascii(mut n: usize, temp: &mut [u8; 20]) -> &[u8] {
    if n == 0 {
        temp[19] = b'0';
        return &temp[19..];
    }
    let mut i = 20;
    while n > 0 {
        i -= 1;
        temp[i] = b'0' + (n % 10) as u8;
        n /= 10;
    }
    &temp[i..]
}
