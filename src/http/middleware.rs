use crate::http::{Request, Response};
use async_trait::async_trait;
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;

pub type BoxFuture = Pin<Box<dyn Future<Output = Response> + Send + 'static>>;

/// Continuation of the middleware pipeline.
///
/// Call [`Next::run`] to pass the request to the next layer. Return a [`Response`]
/// without calling it to stop the pipeline.
pub struct Next {
    inner: Arc<dyn Fn(Request) -> BoxFuture + Send + Sync>,
}

impl Next {
    pub async fn run(self, request: Request) -> Response {
        (self.inner)(request).await
    }
}

/// Onion middleware. The first registered layer sees the request first and the response last.
#[async_trait]
pub trait Middleware: Send + Sync {
    async fn handle(&self, request: Request, next: Next) -> Response;
}

/// Runs `layers` around `destination`.
///
/// `layers[0]` is the outermost layer.
pub struct Pipeline {
    layers: Vec<Arc<dyn Middleware>>,
}

impl Pipeline {
    pub fn new(layers: Vec<Arc<dyn Middleware>>) -> Self {
        Self { layers }
    }

    pub fn run(
        self,
        request: Request,
        destination: Arc<dyn Fn(Request) -> BoxFuture + Send + Sync>,
    ) -> BoxFuture {
        let next = self
            .layers
            .into_iter()
            .rev()
            .fold(destination, |next, layer| {
                let wrapped: Arc<dyn Fn(Request) -> BoxFuture + Send + Sync> =
                    Arc::new(move |request| {
                        let layer = Arc::clone(&layer);
                        let next = Next {
                            inner: Arc::clone(&next),
                        };
                        Box::pin(async move { layer.handle(request, next).await })
                    });
                wrapped
            });

        next(request)
    }
}
