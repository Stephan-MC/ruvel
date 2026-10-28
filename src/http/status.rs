/// Standard HTTP Status Code enum mirroring RFC specifications.
///
/// Each variant represents a standardized HTTP status code with its numeric value as discriminant.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[repr(u16)]
pub enum HttpStatus {
    // -------------------------------------------------------------------------
    // 1xx Informational
    // -------------------------------------------------------------------------

    /// `100 Continue` - The server has received the request headers and the client should proceed to send the request body.
    Continue = 100,

    /// `101 Switching Protocols` - The requester has asked the server to switch protocols and the server has agreed to do so.
    SwitchingProtocols = 101,

    /// `102 Processing` - WebDAV (RFC 2518). The server has received and is processing the request, but no response is available yet.
    Processing = 102,

    /// `103 Early Hints` - Used to return some response headers before final HTTP message (RFC 8297).
    EarlyHints = 103,

    // -------------------------------------------------------------------------
    // 2xx Success
    // -------------------------------------------------------------------------

    /// `200 OK` - Standard response for successful HTTP requests.
    Ok = 200,

    /// `201 Created` - The request has been fulfilled, resulting in the creation of a new resource.
    Created = 201,

    /// `202 Accepted` - The request has been accepted for processing, but the processing has not been completed.
    Accepted = 202,

    /// `203 Non-Authoritative Information` - The server is a transforming proxy that received a 200 OK from its origin.
    NonAuthoritativeInformation = 203,

    /// `204 No Content` - The server successfully processed the request and is not returning any content.
    NoContent = 204,

    /// `205 Reset Content` - The server successfully processed the request, but is not returning any content and requires the requester reset the document view.
    ResetContent = 205,

    /// `206 Partial Content` - The server is delivering only part of the resource due to a range header sent by the client.
    PartialContent = 206,

    /// `207 Multi-Status` - WebDAV (RFC 4918). Message body provides status for multiple independent operations.
    MultiStatus = 207,

    /// `208 Already Reported` - WebDAV (RFC 5842). Members of a DAV binding have already been enumerated.
    AlreadyReported = 208,

    /// `226 IM Used` - RFC 3229. The server has fulfilled a GET request for the resource and response is a delta representation.
    ImUsed = 226,

    // -------------------------------------------------------------------------
    // 3xx Redirection
    // -------------------------------------------------------------------------

    /// `300 Multiple Choices` - Indicates multiple options for the resource from which the client may choose.
    MultipleChoices = 300,

    /// `301 Moved Permanently` - This and all future requests should be directed to the given URI.
    MovedPermanently = 301,

    /// `302 Found` - Tells the client to look at (browse to) another URL temporarily.
    Found = 302,

    /// `303 See Other` - The response to the request can be found under another URI using the GET method.
    SeeOther = 303,

    /// `304 Not Modified` - Indicates that the resource has not been modified since the version specified in the request headers.
    NotModified = 304,

    /// `305 Use Proxy` - The requested resource is available only through a proxy.
    UseProxy = 305,

    /// `307 Temporary Redirect` - The request should be repeated with another URI, but future requests should still use the original URI.
    TemporaryRedirect = 307,

    /// `308 Permanent Redirect` - The request and all future requests should be repeated using another URI.
    PermanentRedirect = 308,

    // -------------------------------------------------------------------------
    // 4xx Client Errors
    // -------------------------------------------------------------------------

    /// `400 Bad Request` - The server cannot or will not process the request due to an apparent client error.
    BadRequest = 400,

    /// `401 Unauthorized` - Authentication is required and has failed or has not yet been provided.
    Unauthorized = 401,

    /// `402 Payment Required` - Reserved for future use.
    PaymentRequired = 402,

    /// `403 Forbidden` - The request contained valid data and was understood by the server, but the server is refusing action.
    Forbidden = 403,

    /// `404 Not Found` - The requested resource could not be found but may be available in the future.
    NotFound = 404,

    /// `405 Method Not Allowed` - A request method is not supported for the requested resource.
    MethodNotAllowed = 405,

    /// `406 Not Acceptable` - The requested resource is capable of generating only content not acceptable according to Accept headers.
    NotAcceptable = 406,

    /// `407 Proxy Authentication Required` - The client must first authenticate itself with the proxy.
    ProxyAuthenticationRequired = 407,

    /// `408 Request Timeout` - The server timed out waiting for the request.
    RequestTimeout = 408,

    /// `409 Conflict` - Indicates that the request could not be processed because of conflict in the current state of the resource.
    Conflict = 409,

    /// `410 Gone` - Indicates that the resource requested is no longer available and will not be available again.
    Gone = 410,

    /// `411 Length Required` - The request did not specify the length of its content, which is required by the requested resource.
    LengthRequired = 411,

    /// `412 Precondition Failed` - The server does not meet one of the preconditions that the requester put on the request headers.
    PreconditionFailed = 412,

    /// `413 Payload Too Large` - The request is larger than the server is willing or able to process.
    PayloadTooLarge = 413,

    /// `414 URI Too Long` - The URI provided was too long for the server to process.
    UriTooLong = 414,

    /// `415 Unsupported Media Type` - The request entity has a media type which the server or resource does not support.
    UnsupportedMediaType = 415,

    /// `416 Range Not Satisfiable` - The client has asked for a portion of the file (byte serving), but the server cannot supply that portion.
    RangeNotSatisfiable = 416,

    /// `417 Expectation Failed` - The server cannot meet the requirements of the Expect request-header field.
    ExpectationFailed = 417,

    /// `418 I'm a teapot` - RFC 2324, RFC 7168. Hyper Text Coffee Pot Control Protocol.
    ImATeapot = 418,

    /// `421 Misdirected Request` - The request was directed at a server that is not able to produce a response.
    MisdirectedRequest = 421,

    /// `422 Unprocessable Entity` - WebDAV (RFC 4918). The request was well-formed but was unable to be followed due to semantic errors.
    UnprocessableEntity = 422,

    /// `423 Locked` - WebDAV (RFC 4918). The resource that is being accessed is locked.
    Locked = 423,

    /// `424 Failed Dependency` - WebDAV (RFC 4918). The request failed because it depended on another request and that request failed.
    FailedDependency = 424,

    /// `425 Too Early` - RFC 8470. Indicates that the server is unwilling to risk processing a request that might be replayed.
    TooEarly = 425,

    /// `426 Upgrade Required` - The client should switch to a different protocol such as TLS/1.3.
    UpgradeRequired = 426,

    /// `428 Precondition Required` - RFC 6585. The origin server requires the request to be conditional.
    PreconditionRequired = 428,

    /// `429 Too Many Requests` - RFC 6585. The user has sent too many requests in a given amount of time (rate limiting).
    TooManyRequests = 429,

    /// `431 Request Header Fields Too Large` - RFC 6585. The server is unwilling to process the request because its header fields are too large.
    RequestHeaderFieldsTooLarge = 431,

    /// `451 Unavailable For Legal Reasons` - RFC 7725. A server operator has received a legal demand to deny access to a resource.
    UnavailableForLegalReasons = 451,

    // -------------------------------------------------------------------------
    // 5xx Server Errors
    // -------------------------------------------------------------------------

    /// `500 Internal Server Error` - A generic error message given when an unexpected condition was encountered.
    InternalServerError = 500,

    /// `501 Not Implemented` - The server either does not recognize the request method, or it lacks the ability to fulfil the request.
    NotImplemented = 501,

    /// `502 Bad Gateway` - The server was acting as a gateway or proxy and received an invalid response from the upstream server.
    BadGateway = 502,

    /// `503 Service Unavailable` - The server cannot handle the request because it is overloaded or down for maintenance.
    ServiceUnavailable = 503,

    /// `504 Gateway Timeout` - The server was acting as a gateway or proxy and did not receive a timely response from the upstream server.
    GatewayTimeout = 504,

    /// `505 HTTP Version Not Supported` - The server does not support the HTTP protocol version used in the request.
    HttpVersionNotSupported = 505,

    /// `506 Variant Also Negotiates` - RFC 2295. Transparent content negotiation for the request results in a circular reference.
    VariantAlsoNegotiates = 506,

    /// `507 Insufficient Storage` - WebDAV (RFC 4918). The server is unable to store the representation needed to complete the request.
    InsufficientStorage = 507,

    /// `508 Loop Detected` - WebDAV (RFC 5842). The server detected an infinite loop while processing a request.
    LoopDetected = 508,

    /// `510 Not Extended` - RFC 2774. Further extensions to the request are required for the server to fulfil it.
    NotExtended = 510,

    /// `511 Network Authentication Required` - RFC 6585. The client needs to authenticate to gain network access.
    NetworkAuthenticationRequired = 511,
}

impl HttpStatus {
    /// Returns the numerical HTTP status code (e.g. 200, 201, 404).
    #[inline]
    pub const fn code(&self) -> u16 {
        *self as u16
    }

    /// Returns the canonical status reason phrase (e.g. "OK", "Created", "Not Found").
    pub const fn message(&self) -> &'static str {
        match self {
            Self::Continue => "Continue",
            Self::SwitchingProtocols => "Switching Protocols",
            Self::Processing => "Processing",
            Self::EarlyHints => "Early Hints",

            Self::Ok => "OK",
            Self::Created => "Created",
            Self::Accepted => "Accepted",
            Self::NonAuthoritativeInformation => "Non-Authoritative Information",
            Self::NoContent => "No Content",
            Self::ResetContent => "Reset Content",
            Self::PartialContent => "Partial Content",
            Self::MultiStatus => "Multi-Status",
            Self::AlreadyReported => "Already Reported",
            Self::ImUsed => "IM Used",

            Self::MultipleChoices => "Multiple Choices",
            Self::MovedPermanently => "Moved Permanently",
            Self::Found => "Found",
            Self::SeeOther => "See Other",
            Self::NotModified => "Not Modified",
            Self::UseProxy => "Use Proxy",
            Self::TemporaryRedirect => "Temporary Redirect",
            Self::PermanentRedirect => "Permanent Redirect",

            Self::BadRequest => "Bad Request",
            Self::Unauthorized => "Unauthorized",
            Self::PaymentRequired => "Payment Required",
            Self::Forbidden => "Forbidden",
            Self::NotFound => "Not Found",
            Self::MethodNotAllowed => "Method Not Allowed",
            Self::NotAcceptable => "Not Acceptable",
            Self::ProxyAuthenticationRequired => "Proxy Authentication Required",
            Self::RequestTimeout => "Request Timeout",
            Self::Conflict => "Conflict",
            Self::Gone => "Gone",
            Self::LengthRequired => "Length Required",
            Self::PreconditionFailed => "Precondition Failed",
            Self::PayloadTooLarge => "Payload Too Large",
            Self::UriTooLong => "URI Too Long",
            Self::UnsupportedMediaType => "Unsupported Media Type",
            Self::RangeNotSatisfiable => "Range Not Satisfiable",
            Self::ExpectationFailed => "Expectation Failed",
            Self::ImATeapot => "I'm a teapot",
            Self::MisdirectedRequest => "Misdirected Request",
            Self::UnprocessableEntity => "Unprocessable Entity",
            Self::Locked => "Locked",
            Self::FailedDependency => "Failed Dependency",
            Self::TooEarly => "Too Early",
            Self::UpgradeRequired => "Upgrade Required",
            Self::PreconditionRequired => "Precondition Required",
            Self::TooManyRequests => "Too Many Requests",
            Self::RequestHeaderFieldsTooLarge => "Request Header Fields Too Large",
            Self::UnavailableForLegalReasons => "Unavailable For Legal Reasons",

            Self::InternalServerError => "Internal Server Error",
            Self::NotImplemented => "Not Implemented",
            Self::BadGateway => "Bad Gateway",
            Self::ServiceUnavailable => "Service Unavailable",
            Self::GatewayTimeout => "Gateway Timeout",
            Self::HttpVersionNotSupported => "HTTP Version Not Supported",
            Self::VariantAlsoNegotiates => "Variant Also Negotiates",
            Self::InsufficientStorage => "Insufficient Storage",
            Self::LoopDetected => "Loop Detected",
            Self::NotExtended => "Not Extended",
            Self::NetworkAuthenticationRequired => "Network Authentication Required",
        }
    }

    /// Try resolving an [`HttpStatus`] from a numeric code. Returns [`None`] if unrecognized.
    pub fn from_code(code: u16) -> Option<Self> {
        Self::try_from(code).ok()
    }
}

impl From<HttpStatus> for u16 {
    #[inline]
    fn from(status: HttpStatus) -> Self {
        status.code()
    }
}

impl TryFrom<u16> for HttpStatus {
    type Error = ();

    fn try_from(code: u16) -> Result<Self, Self::Error> {
        match code {
            100 => Ok(Self::Continue),
            101 => Ok(Self::SwitchingProtocols),
            102 => Ok(Self::Processing),
            103 => Ok(Self::EarlyHints),

            200 => Ok(Self::Ok),
            201 => Ok(Self::Created),
            202 => Ok(Self::Accepted),
            203 => Ok(Self::NonAuthoritativeInformation),
            204 => Ok(Self::NoContent),
            205 => Ok(Self::ResetContent),
            206 => Ok(Self::PartialContent),
            207 => Ok(Self::MultiStatus),
            208 => Ok(Self::AlreadyReported),
            226 => Ok(Self::ImUsed),

            300 => Ok(Self::MultipleChoices),
            301 => Ok(Self::MovedPermanently),
            302 => Ok(Self::Found),
            303 => Ok(Self::SeeOther),
            304 => Ok(Self::NotModified),
            305 => Ok(Self::UseProxy),
            307 => Ok(Self::TemporaryRedirect),
            308 => Ok(Self::PermanentRedirect),

            400 => Ok(Self::BadRequest),
            401 => Ok(Self::Unauthorized),
            402 => Ok(Self::PaymentRequired),
            403 => Ok(Self::Forbidden),
            404 => Ok(Self::NotFound),
            405 => Ok(Self::MethodNotAllowed),
            406 => Ok(Self::NotAcceptable),
            407 => Ok(Self::ProxyAuthenticationRequired),
            408 => Ok(Self::RequestTimeout),
            409 => Ok(Self::Conflict),
            410 => Ok(Self::Gone),
            411 => Ok(Self::LengthRequired),
            412 => Ok(Self::PreconditionFailed),
            413 => Ok(Self::PayloadTooLarge),
            414 => Ok(Self::UriTooLong),
            415 => Ok(Self::UnsupportedMediaType),
            416 => Ok(Self::RangeNotSatisfiable),
            417 => Ok(Self::ExpectationFailed),
            418 => Ok(Self::ImATeapot),
            421 => Ok(Self::MisdirectedRequest),
            422 => Ok(Self::UnprocessableEntity),
            423 => Ok(Self::Locked),
            424 => Ok(Self::FailedDependency),
            425 => Ok(Self::TooEarly),
            426 => Ok(Self::UpgradeRequired),
            428 => Ok(Self::PreconditionRequired),
            429 => Ok(Self::TooManyRequests),
            431 => Ok(Self::RequestHeaderFieldsTooLarge),
            451 => Ok(Self::UnavailableForLegalReasons),

            500 => Ok(Self::InternalServerError),
            501 => Ok(Self::NotImplemented),
            502 => Ok(Self::BadGateway),
            503 => Ok(Self::ServiceUnavailable),
            504 => Ok(Self::GatewayTimeout),
            505 => Ok(Self::HttpVersionNotSupported),
            506 => Ok(Self::VariantAlsoNegotiates),
            507 => Ok(Self::InsufficientStorage),
            508 => Ok(Self::LoopDetected),
            510 => Ok(Self::NotExtended),
            511 => Ok(Self::NetworkAuthenticationRequired),

            _ => Err(()),
        }
    }
}

impl std::fmt::Display for HttpStatus {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{} {}", self.code(), self.message())
    }
}

pub type StatusCode = HttpStatus;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_status_code_and_message() {
        assert_eq!(HttpStatus::Ok.code(), 200);
        assert_eq!(HttpStatus::Ok.message(), "OK");

        assert_eq!(HttpStatus::Created.code(), 201);
        assert_eq!(HttpStatus::Created.message(), "Created");

        assert_eq!(HttpStatus::NotFound.code(), 404);
        assert_eq!(HttpStatus::NotFound.message(), "Not Found");

        assert_eq!(HttpStatus::InternalServerError.code(), 500);
        assert_eq!(HttpStatus::InternalServerError.message(), "Internal Server Error");
    }

    #[test]
    fn test_from_code() {
        assert_eq!(HttpStatus::from_code(200), Some(HttpStatus::Ok));
        assert_eq!(HttpStatus::from_code(201), Some(HttpStatus::Created));
        assert_eq!(HttpStatus::from_code(404), Some(HttpStatus::NotFound));
        assert_eq!(HttpStatus::from_code(999), None);
    }

    #[test]
    fn test_from_and_into_u16() {
        let code: u16 = HttpStatus::Accepted.into();
        assert_eq!(code, 202);

        let status = HttpStatus::try_from(204).unwrap();
        assert_eq!(status, HttpStatus::NoContent);
    }

    #[test]
    fn test_display() {
        assert_eq!(format!("{}", HttpStatus::Ok), "200 OK");
        assert_eq!(format!("{}", HttpStatus::NotFound), "404 Not Found");
    }
}
