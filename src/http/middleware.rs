use crate::http::Request;

pub trait Middleware: Send + Sync {
    fn handle(&self, _req: &Request, _next: Box<dyn FnOnce(&Request) + Send>);
}
