use std::collections::HashMap;

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum Method {
    GET,
    POST,
    PUT,
    DELETE,
    PATCH,
    UNKNOWN(String),
}

impl From<&str> for Method {
    fn from(s: &str) -> Self {
        match s {
            "GET" => Method::GET,
            "POST" => Method::POST,
            "PUT" => Method::PUT,
            "DELETE" => Method::DELETE,
            "PATCH" => Method::PATCH,
            other => Method::UNKNOWN(other.to_string()),
        }
    }
}

pub struct Request {
    pub method: Method,
    pub path: String,
    pub headers: HashMap<String, String>,
    pub body: Vec<u8>,
}

impl Request {
    /// Primitive byte parser reading directly from the TCP stream buffer
    pub fn parse(buffer: &[u8]) -> Option<Self> {
        let req_str = String::from_utf8_lossy(buffer);
        let mut lines = req_str.split("\r\n");

        let first_line = lines.next()?;
        let mut parts = first_line.split_whitespace();
        let method = parts.next().unwrap_or("GET");
        let path = parts.next().unwrap_or("/");

        // In a full implementation, you'd parse headers and body here
        Some(Self {
            method: Method::from(method),
            path: path.to_string(),
            headers: HashMap::new(),
            body: Vec::new(),
        })
    }

    /// Retrieve a header value by key in a case-insensitive manner.
    pub fn header(&self, key: &str) -> Option<&str> {
        let lower = key.to_ascii_lowercase();
        self.headers.iter().find_map(|(k, v)| {
            if k.to_ascii_lowercase() == lower {
                Some(v.as_str())
            } else {
                None
            }
        })
    }
}
