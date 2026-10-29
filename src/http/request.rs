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
    pub params: HashMap<String, String>,
}

impl Request {
    pub fn parse(buffer: &[u8]) -> Option<Self> {
        let req_str = String::from_utf8_lossy(buffer);
        let mut lines = req_str.split("\r\n");

        let first_line = lines.next()?;
        let mut parts = first_line.split_whitespace();
        let method = parts.next().unwrap_or("GET");
        let path = parts.next().unwrap_or("/");

        Some(Self {
            method: Method::from(method),
            path: path.to_string(),
            headers: HashMap::new(),
            body: Vec::new(),
            params: HashMap::new(), // Initialize empty
        })
    }

    /// Retrieve a header value by key (case-insensitive)
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

    /// Retrieve a URL parameter by key (e.g., req.param("id"))
    pub fn param(&self, key: &str) -> Option<&str> {
        self.params.get(key).map(|s| s.as_str())
    }
}
