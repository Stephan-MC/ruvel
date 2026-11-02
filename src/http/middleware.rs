use crate::http::{Request, Response};
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;

pub type BoxFuture<'a, T = Response> = Pin<Box<dyn Future<Output = T> + Send + 'a>>;

pub type HandlerFn = Arc<dyn Fn(Request) -> BoxFuture<'static, Response> + Send + Sync + 'static>;

pub struct Next<'a> {
    pub(crate) layers: &'a [Arc<dyn Middleware>],
    pub(crate) index: usize,
    pub(crate) handler: &'a HandlerFn,
}

impl<'a> Next<'a> {
    pub async fn run(self, request: Request) -> Response {
        if self.index < self.layers.len() {
            // ZERO ATOMIC CONTENTION: Borrow the Arc instead of cloning it.
            // No memory bus locks, no CPU cache invalidation.
            let layer = &self.layers[self.index];

            let next = Next {
                layers: self.layers,
                index: self.index + 1,
                handler: self.handler,
            };

            layer.handle(request, next).await
        } else {
            (self.handler)(request).await
        }
    }
}

pub trait Middleware: Send + Sync {
    // Standard fn returning a BoxFuture preserves object safety for `dyn Middleware`
    fn handle<'a>(&'a self, request: Request, next: Next<'a>) -> BoxFuture<'a, Response>;
}
