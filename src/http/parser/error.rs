use std::io;

use thiserror::Error;

/// Errors emitted when wire bytes violate HTTP framing, grammar, or safety limits.
#[derive(Debug, Error)]
pub enum Error {
    #[error("I/O error: {0}")]
    Io(#[from] io::Error),
    /// Peer closed the socket in the middle of a request.
    #[error("unexpected EOF while reading HTTP request")]
    Eof,
    /// 400 Bad Request
    #[error("malformed HTTP request")]
    Malformed,
    /// 413 Payload Too Large (answer, then close: the body was not read)
    #[error("HTTP request body too large")]
    BodyTooLarge,
    /// 417 Expectation Failed (unsupported `Expect` header value per RFC 9110 §10.1.1)
    #[error("HTTP expectation failed")]
    ExpectationFailed,
    /// 431 Request Header Fields Too Large
    #[error("HTTP request headers too large")]
    HeaderTooLarge,
    /// 501 Not Implemented (Transfer-Encoding other than supported `chunked`/`identity`)
    #[error("unsupported HTTP transfer-encoding")]
    UnsupportedEncoding,
    /// 505 HTTP Version Not Supported
    #[error("HTTP version not supported")]
    VersionNotSupported,
}

impl Error {
    /// Status code to answer with, if a response makes sense at all.
    #[inline]
    pub const fn status(&self) -> Option<u16> {
        match self {
            Error::Io(_) | Error::Eof => None,
            Error::Malformed => Some(400),
            Error::BodyTooLarge => Some(413),
            Error::ExpectationFailed => Some(417),
            Error::HeaderTooLarge => Some(431),
            Error::UnsupportedEncoding => Some(501),
            Error::VersionNotSupported => Some(505),
        }
    }
}
