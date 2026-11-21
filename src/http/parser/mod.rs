mod error;
pub(crate) mod semantics;
mod status;

pub mod h1;
pub mod h2;
pub mod h3;

pub use error::Error;
pub use h1::{Body, Conn, Frame, Head, Limits, Span};
pub use h2::H2;
pub use h3::H3;
pub use status::Status;

const RV_MAX_HEADERS: usize = 64;

/// Maximum number of header fields accepted per HTTP request.
pub const MAX_HEADERS: usize = match option_env!("RV_MAX_HEADERS") {
    Some(val) => match usize::from_str_radix(val, 10) {
        Ok(v) if v > 0 => v,
        _ => RV_MAX_HEADERS,
    },
    None => RV_MAX_HEADERS,
};

/// Protocol version detected or negotiated by a parser.
#[derive(Debug, Copy, Clone, PartialEq, Eq)]
pub enum HttpVersion {
    Http1_0,
    Http1_1,
    Http2,
    Http3,
    Unknown,
}

impl HttpVersion {
    /// Convert an HTTP/1.`minor` version number into [`HttpVersion`].
    #[inline]
    pub const fn from_h1_minor(minor: u8) -> Self {
        match minor {
            0 => Self::Http1_0,
            1 => Self::Http1_1,
            _ => Self::Unknown,
        }
    }
}

impl From<&str> for HttpVersion {
    fn from(value: &str) -> Self {
        match value {
            "HTTP/1.0" => Self::Http1_0,
            "HTTP/1.1" => Self::Http1_1,
            "HTTP/2" => Self::Http2,
            "HTTP/3" => Self::Http3,
            _ => Self::Unknown,
        }
    }
}
