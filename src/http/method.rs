#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum Method {
    GET,
    POST,
    PUT,
    DELETE,
    PATCH,
    HEAD,
    OPTIONS,
    TRACE,
    CONNECT,
    UNKNOWN(String),
}

impl std::fmt::Display for Method {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Method::GET => f.write_str("GET"),
            Method::POST => f.write_str("POST"),
            Method::PUT => f.write_str("PUT"),
            Method::DELETE => f.write_str("DELETE"),
            Method::PATCH => f.write_str("PATCH"),
            Method::HEAD => f.write_str("HEAD"),
            Method::OPTIONS => f.write_str("OPTIONS"),
            Method::TRACE => f.write_str("TRACE"),
            Method::CONNECT => f.write_str("CONNECT"),
            Method::UNKNOWN(other) => f.write_str(other),
        }
    }
}
