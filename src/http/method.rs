/// Maximum byte length of a custom HTTP method stored inline in [`Method::Other`].
pub const MAX_CUSTOM_METHOD_LEN: usize = 23;

/// HTTP request method (`Copy`, zero heap allocation).
///
/// The 9 standard methods use unit variants; custom/extension HTTP tokens up to
/// [`MAX_CUSTOM_METHOD_LEN`] bytes are stored inline in [`Method::Other`].
#[derive(Clone, Copy)]
pub enum Method {
    Get,
    Head,
    Post,
    Put,
    Delete,
    Patch,
    Options,
    Trace,
    Connect,
    Other([u8; MAX_CUSTOM_METHOD_LEN], u8),
}

impl Method {
    /// Construct a [`Method`] from raw wire bytes.
    ///
    /// Returns `None` if `bytes` is empty, exceeds [`MAX_CUSTOM_METHOD_LEN`], or is not valid UTF-8.
    #[inline]
    pub fn from_bytes(bytes: &[u8]) -> Option<Self> {
        match bytes {
            b"GET" => Some(Method::Get),
            b"HEAD" => Some(Method::Head),
            b"POST" => Some(Method::Post),
            b"PUT" => Some(Method::Put),
            b"DELETE" => Some(Method::Delete),
            b"PATCH" => Some(Method::Patch),
            b"OPTIONS" => Some(Method::Options),
            b"TRACE" => Some(Method::Trace),
            b"CONNECT" => Some(Method::Connect),
            _ => Self::other_from_bytes(bytes),
        }
    }

    /// Construct a [`Method`] from a token byte slice (mapping standard uppercase methods to
    /// standard variants and custom tokens up to [`MAX_CUSTOM_METHOD_LEN`] bytes to [`Method::Other`]).
    #[inline]
    pub fn other(bytes: &[u8]) -> Option<Self> {
        Self::from_bytes(bytes)
    }

    #[inline]
    pub(crate) fn other_from_bytes(bytes: &[u8]) -> Option<Self> {
        if bytes.is_empty()
            || bytes.len() > MAX_CUSTOM_METHOD_LEN
            || std::str::from_utf8(bytes).is_err()
        {
            return None;
        }
        let mut buf = [0u8; MAX_CUSTOM_METHOD_LEN];
        buf[..bytes.len()].copy_from_slice(bytes);
        Some(Method::Other(buf, bytes.len() as u8))
    }

    /// Returns the HTTP method token as `&str`.
    #[inline]
    pub fn as_str(&self) -> &str {
        match self {
            Method::Get => "GET",
            Method::Head => "HEAD",
            Method::Post => "POST",
            Method::Put => "PUT",
            Method::Delete => "DELETE",
            Method::Patch => "PATCH",
            Method::Options => "OPTIONS",
            Method::Trace => "TRACE",
            Method::Connect => "CONNECT",
            Method::Other(buf, len) => {
                let n = (*len as usize).min(MAX_CUSTOM_METHOD_LEN);
                std::str::from_utf8(&buf[..n]).unwrap_or("")
            }
        }
    }

    /// Returns the HTTP method token as raw wire bytes.
    #[inline]
    pub fn as_bytes(&self) -> &[u8] {
        match self {
            Method::Get => b"GET",
            Method::Head => b"HEAD",
            Method::Post => b"POST",
            Method::Put => b"PUT",
            Method::Delete => b"DELETE",
            Method::Patch => b"PATCH",
            Method::Options => b"OPTIONS",
            Method::Trace => b"TRACE",
            Method::Connect => b"CONNECT",
            Method::Other(buf, len) => {
                let n = (*len as usize).min(MAX_CUSTOM_METHOD_LEN);
                &buf[..n]
            }
        }
    }

    /// Returns `true` if this is a non-standard / extension method ([`Method::Other`]).
    #[inline]
    pub const fn is_other(&self) -> bool {
        matches!(self, Method::Other(..))
    }
}

impl PartialEq for Method {
    #[inline]
    fn eq(&self, other: &Self) -> bool {
        match (self, other) {
            (Method::Get, Method::Get)
            | (Method::Head, Method::Head)
            | (Method::Post, Method::Post)
            | (Method::Put, Method::Put)
            | (Method::Delete, Method::Delete)
            | (Method::Patch, Method::Patch)
            | (Method::Options, Method::Options)
            | (Method::Trace, Method::Trace)
            | (Method::Connect, Method::Connect) => true,
            (Method::Other(a, al), Method::Other(b, bl)) => {
                let an = (*al as usize).min(MAX_CUSTOM_METHOD_LEN);
                let bn = (*bl as usize).min(MAX_CUSTOM_METHOD_LEN);
                a[..an] == b[..bn]
            }
            _ => false,
        }
    }
}

impl Eq for Method {}

impl std::hash::Hash for Method {
    #[inline]
    fn hash<H: std::hash::Hasher>(&self, state: &mut H) {
        std::mem::discriminant(self).hash(state);
        if let Method::Other(buf, len) = self {
            let n = (*len as usize).min(MAX_CUSTOM_METHOD_LEN);
            buf[..n].hash(state);
        }
    }
}

impl std::fmt::Debug for Method {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Method::Get => f.write_str("Get"),
            Method::Head => f.write_str("Head"),
            Method::Post => f.write_str("Post"),
            Method::Put => f.write_str("Put"),
            Method::Delete => f.write_str("Delete"),
            Method::Patch => f.write_str("Patch"),
            Method::Options => f.write_str("Options"),
            Method::Trace => f.write_str("Trace"),
            Method::Connect => f.write_str("Connect"),
            Method::Other(..) => f.debug_tuple("Other").field(&self.as_str()).finish(),
        }
    }
}

impl std::fmt::Display for Method {
    #[inline]
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

impl From<&str> for Method {
    #[inline]
    fn from(s: &str) -> Self {
        match s {
            "GET" => Method::Get,
            "HEAD" => Method::Head,
            "POST" => Method::Post,
            "PUT" => Method::Put,
            "DELETE" => Method::Delete,
            "PATCH" => Method::Patch,
            "OPTIONS" => Method::Options,
            "TRACE" => Method::Trace,
            "CONNECT" => Method::Connect,
            other => {
                let len = other.floor_char_boundary(MAX_CUSTOM_METHOD_LEN);
                let mut buf = [0u8; MAX_CUSTOM_METHOD_LEN];
                buf[..len].copy_from_slice(&other.as_bytes()[..len]);
                Method::Other(buf, len as u8)
            }
        }
    }
}

impl TryFrom<&[u8]> for Method {
    type Error = ();

    #[inline]
    fn try_from(bytes: &[u8]) -> Result<Self, Self::Error> {
        Self::from_bytes(bytes).ok_or(())
    }
}
