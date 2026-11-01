use crate::http::{Responsable, Response, Status};

pub struct Model {}

impl Responsable for Model {
    fn to_response(self) -> Response {
        Response::new(Status::Ok)
    }
}
