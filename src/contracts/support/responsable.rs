use crate::http::{Response, Status};
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
        Response::with_body(
            Status::Ok,
            "text/html; charset=utf-8",
            self.as_bytes().to_vec(),
        )
    }
}

impl Responsable for String {
    fn to_response(self) -> Response {
        Response::with_body(Status::Ok, "text/html; charset=utf-8", self.into_bytes())
    }
}

impl Responsable for Cow<'static, str> {
    fn to_response(self) -> Response {
        let bytes = match self {
            Cow::Borrowed(s) => s.as_bytes().to_vec(),
            Cow::Owned(s) => s.into_bytes(),
        };
        Response::with_body(Status::Ok, "text/html; charset=utf-8", bytes)
    }
}

impl Responsable for Box<str> {
    fn to_response(self) -> Response {
        Response::with_body(
            Status::Ok,
            "text/html; charset=utf-8",
            String::from(self).into_bytes(),
        )
    }
}

// 3. Status + Content tuples
impl<T: Responsable> Responsable for (Status, T) {
    fn to_response(self) -> Response {
        let mut res = self.1.to_response();
        res.set_status(self.0);
        res
    }
}

impl Responsable for () {
    fn to_response(self) -> Response {
        Response::ok()
    }
}

impl<T: Responsable> Responsable for (u16, T) {
    fn to_response(self) -> Response {
        let mut res = self.1.to_response();
        if let Some(status) = Status::from_code(self.0) {
            res.set_status(status);
        }
        res
    }
}

