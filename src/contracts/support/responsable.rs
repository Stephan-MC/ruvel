use crate::http::{HttpStatus, Response};
use std::borrow::Cow;

/// Equivalent to Laravel's `Illuminate\Contracts\Support\Responsable`.
///
/// Any type implementing `Responsable` can be returned directly from a route handler.
/// When returning string types (`&'static str`, `String`, etc.), a `200 OK`
/// status code is automatically assigned with `Content-Type: text/plain`.
pub trait Responsable: Send + 'static {
    fn to_response(self) -> Response;
}

// 2. Implementations for String types (auto 200 OK with text/html)
impl Responsable for &'static str {
    fn to_response(self) -> Response {
        Response::ok().html(self)
    }
}

impl Responsable for String {
    fn to_response(self) -> Response {
        Response::ok().html(self)
    }
}

impl Responsable for Cow<'static, str> {
    fn to_response(self) -> Response {
        Response::ok().html(self)
    }
}

impl Responsable for Box<str> {
    fn to_response(self) -> Response {
        Response::ok().html(self.to_string())
    }
}

// 3. Status + Content tuples
impl<T: Responsable> Responsable for (HttpStatus, T) {
    fn to_response(self) -> Response {
        let mut res = self.1.to_response();
        res.set_status(self.0);
        res
    }
}

impl<T: Responsable> Responsable for (u16, T) {
    fn to_response(self) -> Response {
        let mut res = self.1.to_response();
        if let Some(status) = HttpStatus::from_code(self.0) {
            res.set_status(status);
        }
        res
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_str_to_response() {
        let res = "<h1>Hello World</h1>".to_response();
        assert_eq!(res.status, HttpStatus::Ok);
        assert_eq!(res.status.code(), 200);
        assert_eq!(String::from_utf8(res.body).unwrap(), "<h1>Hello World</h1>");
        assert_eq!(
            res.headers.get("Content-Type").unwrap(),
            "text/html; charset=utf-8"
        );
    }

    #[test]
    fn test_string_to_response() {
        let text = format!("<p>Dynamic {}</p>", 42);
        let res = text.to_response();
        assert_eq!(res.status, HttpStatus::Ok);
        assert_eq!(res.status.code(), 200);
        assert_eq!(String::from_utf8(res.body).unwrap(), "<p>Dynamic 42</p>");
        assert_eq!(
            res.headers.get("Content-Type").unwrap(),
            "text/html; charset=utf-8"
        );
    }

    #[test]
    fn test_tuple_with_status_to_response() {
        let res = (HttpStatus::Created, "<p>Item Created</p>").to_response();
        assert_eq!(res.status, HttpStatus::Created);
        assert_eq!(res.status.code(), 201);
        assert_eq!(String::from_utf8(res.body).unwrap(), "<p>Item Created</p>");
        assert_eq!(
            res.headers.get("Content-Type").unwrap(),
            "text/html; charset=utf-8"
        );
    }
}
