use crate::contracts;
use crate::http::{BoxFuture, HandlerFn, Middleware, Next, Request, Response};
use crate::routing::Router;
use std::sync::Arc;

pub struct Kernel {
    router: Arc<Router>,
    middleware: Arc<[Arc<dyn Middleware>]>,
    router_handler: HandlerFn,
}

impl Kernel {
    pub fn new(router: Arc<Router>, global_middleware: Vec<Arc<dyn Middleware>>) -> Self {
        let r = Arc::clone(&router);
        let router_handler: HandlerFn = Arc::new(move |req| {
            let r = Arc::clone(&r);
            Box::pin(async move { r.dispatch(req).await })
        });

        Self {
            router,
            middleware: global_middleware.into(),
            router_handler,
        }
    }

    pub fn handle<'a>(&'a self, req: Request) -> BoxFuture<'a, Response> {
        if self.middleware.is_empty() {
            return self.router.dispatch(req);
        }

        Box::pin(async move {
            let next = Next {
                layers: &self.middleware,
                index: 0,
                handler: &self.router_handler,
            };

            next.run(req).await
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
