use super::headers::HeaderMap;
use super::status::HttpStatus;
use crate::http::Responsable;

#[derive(Debug, Clone)]
pub struct Response {
    /// The HTTP status (code + message) of the response.
    pub status: HttpStatus,
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

    /// Create a new Response initialized from an [`HttpStatus`].
    /// Defaults to `Content-Type: text/html; charset=utf-8`.
    pub fn new(status: HttpStatus) -> Self {
        let mut headers = HeaderMap::new();
        headers.insert("Content-Type", "text/html; charset=utf-8");
        Self {
            status,
            headers,
            body: Vec::new(),
        }
    }

    /// Create a new Response initialized from a numeric status code.
    /// Returns [`None`] if the code is not a recognized HTTP status code.
    pub fn from_code(code: u16) -> Option<Self> {
        HttpStatus::from_code(code).map(Self::new)
    }

    /// Convenience constructor with [`HttpStatus`].
    pub fn status(status: HttpStatus) -> Self {
        Self::new(status)
    }

    pub fn ok() -> Self {
        Self::new(HttpStatus::Ok)
    }

    pub fn created() -> Self {
        Self::new(HttpStatus::Created)
    }

    pub fn no_content() -> Self {
        Self::new(HttpStatus::NoContent)
    }

    pub fn bad_request() -> Self {
        Self::new(HttpStatus::BadRequest)
    }

    pub fn not_found() -> Self {
        Self::new(HttpStatus::NotFound)
    }

    pub fn not_implemented() -> Self {
        Self::new(HttpStatus::NotImplemented)
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
    pub fn set_status(&mut self, status: HttpStatus) -> &mut Self {
        self.status = status;
        self
    }

    /// Insert a header key and value (case-insensitive key).
    pub fn header(mut self, key: impl Into<String>, value: impl Into<String>) -> Self {
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

    /// Convert struct back to raw HTTP/1.1 bytes for TCP transmission
    pub fn to_bytes(&self) -> Vec<u8> {
        let mut raw = Vec::with_capacity(128 + self.body.len());

        raw.extend_from_slice(b"HTTP/1.1 ");
        raw.extend_from_slice(self.status_code().to_string().as_bytes());
        raw.extend_from_slice(b" ");
        raw.extend_from_slice(self.status_message().as_bytes());
        raw.extend_from_slice(b"\r\nContent-Length: ");
        raw.extend_from_slice(self.body.len().to_string().as_bytes());
        raw.extend_from_slice(b"\r\n");

        for (k, v) in &self.headers {
            raw.extend_from_slice(k.as_bytes());
            raw.extend_from_slice(b": ");
            raw.extend_from_slice(v.as_bytes());
            raw.extend_from_slice(b"\r\n");
        }

        raw.extend_from_slice(b"\r\n");
        raw.extend_from_slice(&self.body);

        raw
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_response_status_property_code_and_message() {
        let res = Response::ok();
        assert_eq!(res.status, HttpStatus::Ok);
        assert_eq!(res.status.code(), 200);
        assert_eq!(res.status.message(), "OK");

        let created_res = Response::created();
        assert_eq!(created_res.status, HttpStatus::Created);
        assert_eq!(created_res.status.code(), 201);
        assert_eq!(created_res.status.message(), "Created");

        let not_found_res = Response::not_found();
        assert_eq!(not_found_res.status, HttpStatus::NotFound);
        assert_eq!(not_found_res.status.code(), 404);
        assert_eq!(not_found_res.status.message(), "Not Found");
    }

    #[test]
    fn test_http_constants() {
        assert_eq!(Response::HTTP_OK, 200);
        assert_eq!(Response::HTTP_CREATED, 201);
        assert_eq!(Response::HTTP_ACCEPTED, 202);
        assert_eq!(Response::HTTP_NO_CONTENT, 204);
        assert_eq!(Response::HTTP_BAD_REQUEST, 400);
        assert_eq!(Response::HTTP_NOT_FOUND, 404);
        assert_eq!(Response::HTTP_INTERNAL_SERVER_ERROR, 500);
        assert_eq!(Response::HTTP_NOT_IMPLEMENTED, 501);
    }

    #[test]
    fn test_response_new_with_http_status_enum() {
        let res = Response::new(HttpStatus::Accepted).text("Task queued");
        assert_eq!(res.status, HttpStatus::Accepted);
        assert_eq!(res.status.code(), 202);
        assert_eq!(res.status.message(), "Accepted");
    }

    #[test]
    fn test_response_from_code() {
        let res = Response::from_code(Response::HTTP_CREATED).unwrap();
        assert_eq!(res.status, HttpStatus::Created);
        assert_eq!(res.status.code(), 201);
        assert_eq!(res.status.message(), "Created");

        let custom_res = Response::from_code(999);
        assert!(custom_res.is_none());
    }

    #[test]
    fn test_response_set_status() {
        let mut res = Response::ok();
        assert_eq!(res.status, HttpStatus::Ok);

        res.set_status(HttpStatus::Forbidden);
        assert_eq!(res.status, HttpStatus::Forbidden);
        assert_eq!(res.status.code(), 403);
        assert_eq!(res.status.message(), "Forbidden");
    }

    #[test]
    fn test_response_default_content_type_is_html() {
        let res = Response::ok();
        assert_eq!(
            res.headers.get("content-type"),
            Some("text/html; charset=utf-8")
        );
        assert_eq!(
            res.headers.get("CONTENT-TYPE"),
            Some("text/html; charset=utf-8")
        );
        assert_eq!(
            res.headers.get("Content-Type"),
            Some("text/html; charset=utf-8")
        );
    }

    #[test]
    fn test_case_insensitive_response_headers() {
        let res = Response::ok().header("X-Api-Key", "secret999");
        assert_eq!(res.headers.get("x-api-key"), Some("secret999"));
        assert_eq!(res.headers.get("X-API-KEY"), Some("secret999"));
        assert_eq!(res.headers.get("X-Api-Key"), Some("secret999"));
        assert!(res.headers.contains_key("x-api-key"));
    }

    #[test]
    fn test_response_to_bytes() {
        let res = Response::new(HttpStatus::Created).html("Created entity");
        let bytes = res.to_bytes();
        let s = String::from_utf8(bytes).unwrap();
        assert!(s.starts_with("HTTP/1.1 201 Created\r\n"));
        assert!(s.contains("Content-Length: 14\r\n"));
        assert!(s.contains("Content-Type: text/html; charset=utf-8\r\n"));
        assert!(s.ends_with("\r\n\r\nCreated entity"));
    }
}
