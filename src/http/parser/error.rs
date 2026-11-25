use std::{fmt, io};

use crate::http::Status as HttpStatus;

/// Transport or HTTP protocol error returned by stream-reading [`super::h1::Conn`] drivers.
#[derive(Debug)]
pub enum Error {
    Io(io::Error),
    /// Peer closed the socket in the middle of a request.
    Eof,
    /// Wire framing, grammar, or limit violation mapped to an HTTP response [`HttpStatus`].
    Http(HttpStatus),
}

impl Error {
    /// HTTP status to answer with, if a response makes sense (`None` on `Io` / `Eof`).
    #[inline]
    pub const fn status(&self) -> Option<HttpStatus> {
        match self {
            Error::Io(_) | Error::Eof => None,
            Error::Http(status) => Some(*status),
        }
    }
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Error::Io(err) => write!(f, "I/O error: {err}"),
            Error::Eof => f.write_str("unexpected EOF while reading HTTP request"),
            Error::Http(status) => write!(f, "HTTP parse error: {status}"),
        }
    }
}

impl std::error::Error for Error {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Error::Io(err) => Some(err),
            Error::Eof => None,
            Error::Http(status) => Some(status),
        }
    }
}

impl From<io::Error> for Error {
    #[inline]
    fn from(err: io::Error) -> Self {
        Error::Io(err)
    }
}

impl From<HttpStatus> for Error {
    #[inline]
    fn from(status: HttpStatus) -> Self {
        Error::Http(status)
    }
}
