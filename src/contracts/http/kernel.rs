use crate::http::{BoxFuture, Request, Response};

pub trait Kernel: Send + Sync + 'static {
    fn handle<'a>(&'a self, req: Request) -> BoxFuture<'a, Response>;
}
