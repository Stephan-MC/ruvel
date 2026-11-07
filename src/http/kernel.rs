use crate::contracts;
use crate::foundation::Application;
use crate::http::{BoxFuture, HandlerFn, Middleware, Next, Request, Response};
use crate::routing::Router;
use std::sync::Arc;

pub struct Kernel {
    pub router: Arc<Router>,
    pub middleware: Arc<[Arc<dyn Middleware>]>,
    router_handler: HandlerFn,
}

impl Kernel {
    pub fn new(_app: &Application, router: Arc<Router>) -> Self {
        let r = Arc::clone(&router);
        let router_handler: HandlerFn = Arc::new(move |req| {
            let r = Arc::clone(&r);
            Box::pin(async move { r.dispatch(req).await })
        });

        Self {
            router,
            middleware: Arc::new([]),
            router_handler,
        }
    }

    pub fn with_middleware(mut self, middleware: Vec<Arc<dyn Middleware>>) -> Self {
        self.middleware = middleware.into();
        self
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
}

impl Default for Kernel {
    fn default() -> Self {
        let r: Arc<Router> = Arc::new(Router::new());
        let r_clone = Arc::clone(&r);
        let router_handler: HandlerFn = Arc::new(move |req| {
            let r = Arc::clone(&r_clone);
            Box::pin(async move { r.dispatch(req).await })
        });

        Self {
            router: r,
            middleware: Arc::new([]),
            router_handler,
        }
    }
}

impl contracts::http::Kernel for Kernel {
    fn handle<'a>(&'a self, req: Request) -> BoxFuture<'a, Response> {
        self.handle(req)
    }
}
