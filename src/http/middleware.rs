use crate::contracts::pipeline::Pipeable;
use crate::http::{Request, Response};
use crate::pipeline::Next as PipelineNext;
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;

pub type BoxFuture<'a, T = Response> = Pin<Box<dyn Future<Output = T> + Send + 'a>>;

pub type HandlerFn = Arc<dyn Fn(Request) -> BoxFuture<'static, Response> + Send + Sync + 'static>;

/// Alias to pipeline's Next type for HTTP Request and Response.
pub type Next = PipelineNext<Request, Response>;

/// HTTP Middleware trait that acts as a pipeable item in the HTTP pipeline.
pub trait Middleware: Send + Sync + 'static {
    fn handle<'a>(&'a self, request: Request, next: Next) -> BoxFuture<'a, Response>;
}

/// Allow any `Middleware` to implement `Pipeable<Request, Response>` when wrapped in Arc.
/// This completely avoids conflicting with other Pipeable blanket implementations.
impl Pipeable<Request, Response> for dyn Middleware {
    fn handle<'a>(&'a self, passable: Request, next: Next) -> BoxFuture<'a, Response> {
        Middleware::handle(self, passable, next)
    }
}
