use super::headers::HeaderMap;
use super::status::Status;
use crate::http::Responsable;
use std::borrow::Cow;

#[derive(Debug, Clone)]
pub struct Response {
    /// The HTTP status (code + message) of the response.
    pub status: Status,
    /// Case-insensitive HTTP response headers.
    pub headers: HeaderMap,
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
            headers: HeaderMap::with_single("Content-Type", "text/html; charset=utf-8"),
            body: Vec::new(),
        }
    }

    /// Internal fast-path constructor with an explicit `Content-Type` and pre-built body.
    pub(crate) fn with_body(status: Status, content_type: &'static str, body: Vec<u8>) -> Self {
        Self {
            status,
            headers: HeaderMap::with_single("Content-Type", content_type),
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

    /// Serialize this response into a new buffer.
    pub fn to_bytes(&self) -> Vec<u8> {
        let mut buffer = Vec::with_capacity(128 + self.body.len());
        self.write_to(&mut buffer);
        buffer
    }

    /// Fast response serialization avoiding `std::fmt::write!` format string parsing
    /// and dynamic formatter dispatch.
    pub fn write_to(&self, buffer: &mut Vec<u8>) {
        // Reserve enough space up front to avoid multiple growth reallocations on fresh buffers.
        buffer.reserve(96 + self.body.len());

        // Fast static status line dispatch
        let status_line = match self.status {
            Status::Ok => b"HTTP/1.1 200 OK\r\n" as &[u8],
            Status::Created => b"HTTP/1.1 201 Created\r\n",
            Status::Accepted => b"HTTP/1.1 202 Accepted\r\n",
            Status::NoContent => b"HTTP/1.1 204 No Content\r\n",
            Status::BadRequest => b"HTTP/1.1 400 Bad Request\r\n",
            Status::Unauthorized => b"HTTP/1.1 401 Unauthorized\r\n",
            Status::Forbidden => b"HTTP/1.1 403 Forbidden\r\n",
            Status::NotFound => b"HTTP/1.1 404 Not Found\r\n",
            Status::MethodNotAllowed => b"HTTP/1.1 405 Method Not Allowed\r\n",
            Status::InternalServerError => b"HTTP/1.1 500 Internal Server Error\r\n",
            Status::NotImplemented => b"HTTP/1.1 501 Not Implemented\r\n",
            Status::BadGateway => b"HTTP/1.1 502 Bad Gateway\r\n",
            Status::ServiceUnavailable => b"HTTP/1.1 503 Service Unavailable\r\n",
            _ => {
                buffer.extend_from_slice(b"HTTP/1.1 ");
                append_usize_fast(buffer, self.status_code() as usize);
                buffer.extend_from_slice(b" ");
                buffer.extend_from_slice(self.status_message().as_bytes());
                b"\r\n"
            }
        };
        buffer.extend_from_slice(status_line);

        // Fast Content-Length line
        buffer.extend_from_slice(b"Content-Length: ");
        append_usize_fast(buffer, self.body.len());
        buffer.extend_from_slice(b"\r\n");

        // Headers
        for (k, v) in self.headers.iter() {
            buffer.extend_from_slice(k.as_bytes());
            buffer.extend_from_slice(b": ");
            buffer.extend_from_slice(v.as_bytes());
            buffer.extend_from_slice(b"\r\n");
        }

        // End of headers and body payload
        buffer.extend_from_slice(b"\r\n");
        buffer.extend_from_slice(&self.body);
    }
}

/// Zero-allocation, fast usize to ASCII serialization directly appending to buffer
#[inline]
fn append_usize_fast(buffer: &mut Vec<u8>, mut n: usize) {
    if n == 0 {
        buffer.push(b'0');
        return;
    }
    let mut temp = [0u8; 20];
    let mut i = 20;
    while n > 0 {
        i -= 1;
        temp[i] = b'0' + (n % 10) as u8;
        n /= 10;
    }
    buffer.extend_from_slice(&temp[i..]);
}
