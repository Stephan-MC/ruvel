use crate::contracts;
use crate::http::{BoxFuture, Middleware, Request, Response};
use crate::pipeline::Pipeline;
use crate::routing::Router;
use std::sync::Arc;

pub struct Kernel {
    router: Arc<Router>,
    middleware: Arc<[Arc<dyn Middleware>]>,
}

impl Kernel {
    pub fn new(router: Arc<Router>, global_middleware: Vec<Arc<dyn Middleware>>) -> Self {
        Self {
            router,
            middleware: global_middleware.into(),
        }
    }

    pub fn handle<'a>(&'a self, req: Request) -> BoxFuture<'a, Response> {
        if self.middleware.is_empty() {
            return self.router.dispatch(req);
        }

        let router = Arc::clone(&self.router);
        let middleware = Arc::clone(&self.middleware);

        Box::pin(async move {
            Pipeline::new()
                .send(req)
                .through(middleware.iter().cloned())
                .then(move |r| {
                    let router = Arc::clone(&router);
                    async move { router.dispatch(r).await }
                })
                .await
        })
    }

    pub fn router(&self) -> &Arc<Router> {
        &self.router
    }
}

impl Default for Kernel {
    fn default() -> Self {
        Self::new(Arc::new(Router::new()), Vec::new())
    }
}

impl contracts::http::Kernel for Kernel {
    fn handle<'a>(&'a self, req: Request) -> BoxFuture<'a, Response> {
        self.handle(req)
    }
}
