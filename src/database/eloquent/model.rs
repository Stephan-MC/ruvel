use crate::http::{HttpStatus, Responsable, Response};

pub struct Model {}

impl Responsable for Model {
    fn to_response(self) -> Response {
        Response::new(HttpStatus::Ok)
    }
}
