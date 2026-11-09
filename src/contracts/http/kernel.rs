use crate::http::middleware::BoxFuture;
use crate::http::{Request, Response};

pub trait Kernel: Send + Sync + 'static {
    fn handle<'a>(&'a self, req: Request) -> BoxFuture<'a, Response>;
}
