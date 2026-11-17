use std::borrow::Cow;

use crate::http::Method;

/// Protocol version detected or negotiated by a parser.
#[derive(Debug, Copy, Clone, PartialEq, Eq)]
pub enum HttpVersion {
    Http1_0,
    Http1_1,
    Http2,
    Http3,
}

/// Parsing outcome distinguishing complete frames from fragmented TCP reads.
#[derive(Debug, PartialEq, Eq)]
pub enum Status<T> {
    /// A complete frame was parsed (`(request, bytes_consumed)`).
    Complete(T, usize),
    /// More bytes are needed from the socket before parsing can finish.
    Incomplete,
}

/// Errors emitted when wire bytes violate HTTP grammar, framing, or safety limits.
#[derive(Debug, PartialEq, Eq)]
pub enum ParseError {
    InvalidRequestLine,
    InvalidMethod,
    InvalidUri,
    InvalidVersion,
    InvalidHeader,
    InvalidBody,
    TooManyHeaders,
    UnsupportedProtocol,
}

/// Universal Protocol Parser Trait for Ruvel.
pub trait Parser<const N: usize> {
    type Request<'a>;

    /// Parses a raw byte buffer into a request representation.
    /// Returns `Ok(Status::Complete(Request, bytes_consumed))`,
    /// `Ok(Status::Incomplete)` on fragmented TCP reads, or `Err(ParseError)`.
    fn parse<'a>(buf: &'a [u8]) -> Result<Status<Self::Request<'a>>, ParseError>;
}

/// Single HTTP header name/value pair borrowed directly from the socket read buffer.
#[derive(Debug, Copy, Clone, PartialEq, Eq)]
pub struct Header<'a> {
    pub name: &'a str,
    pub value: &'a str,
}

/// Zero-copy parsed HTTP request backed by the caller's socket read buffer.
#[derive(Debug, PartialEq, Eq)]
pub struct Request<'a, const N: usize> {
    pub method: Method,
    pub query: &'a str,
    pub path: &'a str,
    pub version: &'a str,
    pub headers: [Header<'a>; N],
    pub header_count: usize,
    pub body: Cow<'a, [u8]>,
}

impl<'a, const N: usize> Request<'a, N> {
    /// Returns the slice of headers actually parsed for this request (`0..=self.header_count`).
    #[inline]
    pub fn headers(&self) -> &[Header<'a>] {
        &self.headers[..self.header_count]
    }
}

pub mod h1;
pub mod h2;
pub mod h3;

pub use h1::H1;
pub use h2::H2;
pub use h3::H3;
