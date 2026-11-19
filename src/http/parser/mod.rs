mod error;
pub(crate) mod semantics;
mod status;
use std::{borrow::Cow, mem::MaybeUninit, ops::Deref};

use crate::http::{Method, parser};
pub use error::Error;
pub use status::Status;

pub mod h1;
pub mod h2;
pub mod h3;

pub use h1::H1;
pub use h2::H2;
pub use h3::H3;

const RV_MAX_HEADERS: usize = 64;
const RV_MAX_HEADERS_SIZE: usize = 8 * 1024;

/// The maximum number of headers accepted by an given request
pub const MAX_HEADERS: usize = match option_env!("RV_MAX_HEADERS") {
    Some(val) => match usize::from_str_radix(val, 10) {
        Ok(v) if v > 0 => v,
        _ => RV_MAX_HEADERS,
    },
    None => RV_MAX_HEADERS,
};

/// The maximum size in bytes allowed for each requests to store headers
/// TODO: Add this parameter into the request parsing
pub const MAX_HEADERS_SIZE: usize = match option_env!("RV_MAX_HEADERS_SIZE") {
    Some(val) => match usize::from_str_radix(val, 10) {
        Ok(v) if v > 0 => v,
        _ => RV_MAX_HEADERS_SIZE,
    },
    None => RV_MAX_HEADERS_SIZE,
};

/// Universal Protocol Parser Trait for Ruvel.
/// const `H` shows the number of headers per request
pub trait Parser<const H: usize> {
    type Request<'a>;

    /// Parses a raw byte buffer into a request representation.
    /// Returns `Ok(Status::Complete(Request, bytes_consumed))`,
    /// `Ok(Status::Incomplete)` on fragmented TCP reads, or `Err(ParseError)`.
    fn parse<'a>(buf: &'a [u8]) -> Result<Status<Self::Request<'a>>, parser::Error<'a>>;
}

/// Protocol version detected or negotiated by a parser.
#[derive(Debug, Copy, Clone, PartialEq, Eq)]
pub enum HttpVersion {
    Http1_0,
    Http1_1,
    Http2,
    Http3,
    Unknown,
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

/// Single HTTP header name/value pair borrowed directly from the socket read buffer.
#[derive(Debug, Copy, Clone, PartialEq, Eq)]
pub struct Header<'a> {
    pub name: &'a str,
    pub value: &'a str,
}

/// Fixed-capacity inline header storage without heap allocation or initializing unused slots.
#[derive(Copy, Clone)]
pub struct Headers<'a, const N: usize> {
    len: usize,
    entries: [MaybeUninit<Header<'a>>; N],
}

impl<'a, const N: usize> Default for Headers<'a, N> {
    fn default() -> Self {
        Self::new()
    }
}

impl<'a, const N: usize> Headers<'a, N> {
    /// Creates an empty inline header buffer without initializing backing array slots.
    #[inline]
    pub const fn new() -> Self {
        Self {
            len: 0,
            entries: [const { MaybeUninit::uninit() }; N],
        }
    }

    /// Appends a parsed header, returning `Err(Error::TooManyHeaders)` if at capacity `N`.
    #[inline]
    pub fn push(&mut self, header: Header<'a>) -> Result<(), Error<'a>> {
        if self.len >= N {
            return Err(Error::TooManyHeaders);
        }
        self.entries[self.len].write(header);
        self.len += 1;
        Ok(())
    }

    /// Returns the slice of headers actually parsed (`0..self.len`).
    #[inline]
    pub fn as_slice(&self) -> &[Header<'a>] {
        // SAFETY: `entries[..self.len]` are always initialized via `push`.
        unsafe { std::slice::from_raw_parts(self.entries.as_ptr().cast(), self.len) }
    }

    /// Looks up a header value by name (case-insensitive).
    #[inline]
    pub fn get(&self, name: &str) -> Option<&'a str> {
        self.as_slice()
            .iter()
            .find(|h| h.name.eq_ignore_ascii_case(name))
            .map(|h| h.value)
    }
}

impl<'a, const N: usize> Deref for Headers<'a, N> {
    type Target = [Header<'a>];

    #[inline]
    fn deref(&self) -> &Self::Target {
        self.as_slice()
    }
}

impl<'a, 'b, const N: usize> IntoIterator for &'b Headers<'a, N> {
    type Item = &'b Header<'a>;
    type IntoIter = std::slice::Iter<'b, Header<'a>>;

    #[inline]
    fn into_iter(self) -> Self::IntoIter {
        self.as_slice().iter()
    }
}

impl<'a, const N: usize> PartialEq for Headers<'a, N> {
    #[inline]
    fn eq(&self, other: &Self) -> bool {
        self.as_slice() == other.as_slice()
    }
}

impl<'a, const N: usize> Eq for Headers<'a, N> {}

impl<'a, const N: usize> std::fmt::Debug for Headers<'a, N> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.as_slice().fmt(f)
    }
}

/// Zero-copy parsed HTTP request backed by the caller's socket read buffer.
///
/// Stack size scales linearly with header capacity `N` (`~32 * N + 96` bytes on 64-bit targets,
/// ~1.1 KB for `N = 32` or ~2.1 KB for `N = 64`), so keep `N` bounded in async `.await` futures.
#[derive(Debug, PartialEq, Eq)]
pub struct Request<'a, const H: usize = MAX_HEADERS> {
    pub method: Method,
    pub query: &'a str,
    pub path: &'a str,
    pub version: HttpVersion,
    pub headers: Headers<'a, H>,
    pub body: Cow<'a, [u8]>,
}
