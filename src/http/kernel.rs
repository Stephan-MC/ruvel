use crate::configuration::Middleware as MiddlewareConfig;
use crate::contracts;
use crate::http::middleware::{BoxFuture, Middleware};
use crate::http::{Request, Response};
use crate::routing::Router;
use std::collections::HashMap;
use std::sync::{Arc, RwLock};

pub struct Kernel {
    pub router: Arc<RwLock<Arc<Router>>>,
}

impl Kernel {
    pub fn new(router: Arc<Router>) -> Self {
        Self {
            router: Arc::new(RwLock::new(router)),
        }
    }

    /// Set the router instance on the kernel.
    pub fn set_router(&self, router: Arc<Router>) {
        *self.router.write().unwrap() = router;
    }

    /// Get a clone of the inner router Arc.
    pub fn get_router(&self) -> Arc<Router> {
        self.router.read().unwrap().clone()
    }

    /// Set global middleware on the router.
    pub fn set_global_middleware(&self, middleware: Vec<Arc<dyn Middleware>>) {
        self.get_router().set_middlewares(middleware);
    }

    /// Set route middleware groups on the router.
    pub fn set_middleware_groups(
        &self,
        groups: HashMap<String, Vec<Arc<dyn Middleware>>>,
    ) {
        self.get_router().set_middleware_groups(groups);
    }

    /// Set route middleware aliases on the router.
    pub fn set_middleware_aliases(
        &self,
        aliases: HashMap<String, Arc<dyn Middleware>>,
    ) {
        self.get_router().set_middleware_aliases(aliases);
    }

    /// Configure middleware via the Laravel-style Middleware configuration object.
    pub fn configure_middleware<F>(&self, callback: F)
    where
        F: FnOnce(&mut MiddlewareConfig),
    {
        let mut config = MiddlewareConfig::new();
        callback(&mut config);
        let resolved = config.resolve();

        self.get_router().install_middleware(
            resolved.global,
            resolved.groups,
            resolved.aliases,
        );
    }

    pub fn handle<'a>(&'a self, req: Request) -> BoxFuture<'a, Response> {
        let router = self.get_router();
        Box::pin(async move { router.dispatch(req).await })
    }
}

impl Default for Kernel {
    fn default() -> Self {
        Self::new(Arc::new(Router::new()))
    }
}

impl contracts::http::Kernel for Kernel {
    fn handle<'a>(&'a self, req: Request) -> BoxFuture<'a, Response> {
        self.handle(req)
    }
}
