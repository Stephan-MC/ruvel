/// Protocol version detected or negotiated by a parser.
#[derive(Debug, Copy, Clone, PartialEq, Eq)]
pub enum Version {
    Http1_0,
    Http1_1,
    Http2,
    Http3,
    Unknown,
}

impl Version {
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

impl From<&str> for Version {
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
