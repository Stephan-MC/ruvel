use super::Header;

/// Errors emitted when wire bytes violate HTTP grammar, framing, or safety limits.
#[derive(Debug, PartialEq, Eq)]
pub enum Error<'a> {
    /// Request line structure is malformed (empty line, bare CR/LF, or invalid SP delimiters).
    InvalidRequestLine,
    /// HTTP method is empty or contains bytes outside RFC 9110 `tchar`.
    InvalidMethod,
    /// Request target is empty, contains illegal/unencoded bytes, or uses an invalid target form.
    InvalidUri,
    /// HTTP version token is malformed or fails UTF-8/version syntax checks.
    InvalidVersion,
    /// A header line or framing header violates HTTP grammar or framing rules.
    InvalidHeader(Header<'a>),
    /// An HTTP/1.1 request is missing its required `Host` header.
    MissingHost,
    /// A `Host` header is duplicated, empty, or contains an invalid host/port authority value.
    InvalidHost,
    /// Message body framing or chunked transfer payload is malformed or overflows bounds.
    InvalidBody,
    /// Number of header fields exceeds the parser's fixed capacity `N`.
    TooManyHeaders,
    /// Request line and header block exceed `MAX_HEADERS_SIZE`.
    HeadersTooLarge,
    /// Request contains an unsupported `Expect` header value (RFC 9110 §10.1.1).
    ExpectationFailed,
    /// Wire protocol version is syntactically recognized but unsupported by this parser.
    UnsupportedProtocol,
}
