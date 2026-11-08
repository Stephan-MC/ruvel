use crate::contracts::pipeline::Pipeable;
use crate::http::{Request, Response};
use crate::pipeline::Pipeline as BasePipeline;
use std::future::Future;

/// A specialized routing pipeline extending Ruvel's base pipeline for HTTP Request and Response.
pub struct Pipeline {
    inner: BasePipeline<Request, Response>,
}

impl Default for Pipeline {
    fn default() -> Self {
        Self::new()
    }
}

impl Pipeline {
    pub fn new() -> Self {
        Self {
            inner: BasePipeline::new(),
        }
    }

    pub fn send(mut self, request: Request) -> Self {
        self.inner = self.inner.send(request);
        self
    }

    pub fn through<I, P>(mut self, pipes: I) -> Self
    where
        I: IntoIterator<Item = P>,
        P: Pipeable<Request, Response> + 'static,
    {
        self.inner = self.inner.through(pipes);
        self
    }

    pub fn pipe<P>(mut self, pipe: P) -> Self
    where
        P: Pipeable<Request, Response> + 'static,
    {
        self.inner = self.inner.pipe(pipe);
        self
    }

    pub async fn then<F, Fut>(self, destination: F) -> Response
    where
        F: Fn(Request) -> Fut + Send + Sync + 'static,
        Fut: Future<Output = Response> + Send + 'static,
    {
        self.inner.then(destination).await
    }
}
