use crate::configuration::Middleware as MiddlewareConfig;
use crate::contracts;
use crate::http::{BoxFuture, HandlerFn, Middleware, Next, Request, Response};
use crate::routing::Router;
use std::collections::HashMap;
use std::sync::{Arc, RwLock};

pub struct Kernel {
    pub router: Arc<RwLock<Arc<Router>>>,
    pub middleware: Arc<RwLock<Vec<Arc<dyn Middleware>>>>,
    pub middleware_groups: Arc<RwLock<HashMap<String, Vec<Arc<dyn Middleware>>>>>,
    pub middleware_aliases: Arc<RwLock<HashMap<String, Arc<dyn Middleware>>>>,
}

impl Kernel {
    pub fn new(router: Arc<Router>) -> Self {
        Self {
            router: Arc::new(RwLock::new(router)),
            middleware: Arc::new(RwLock::new(Vec::new())),
            middleware_groups: Arc::new(RwLock::new(HashMap::new())),
            middleware_aliases: Arc::new(RwLock::new(HashMap::new())),
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

    /// Set global middleware on the kernel.
    pub fn set_global_middleware(&self, middleware: Vec<Arc<dyn Middleware>>) {
        *self.middleware.write().unwrap() = middleware;
    }

    /// Set route middleware groups on the kernel and sync to the router.
    pub fn set_middleware_groups(
        &self,
        groups: HashMap<String, Vec<Arc<dyn Middleware>>>,
    ) {
        *self.middleware_groups.write().unwrap() = groups;
        self.sync_middleware_to_router();
    }

    /// Set route middleware aliases on the kernel and sync to the router.
    pub fn set_middleware_aliases(
        &self,
        aliases: HashMap<String, Arc<dyn Middleware>>,
    ) {
        *self.middleware_aliases.write().unwrap() = aliases;
        self.sync_middleware_to_router();
    }

    /// Configure middleware via the Laravel-style Middleware configuration object.
    pub fn configure_middleware<F>(&self, callback: F)
    where
        F: FnOnce(&mut MiddlewareConfig),
    {
        let mut config = MiddlewareConfig::new();
        callback(&mut config);
        let resolved = config.resolve();

        self.set_global_middleware(resolved.global);
        *self.middleware_groups.write().unwrap() = resolved.groups;
        *self.middleware_aliases.write().unwrap() = resolved.aliases;
        self.sync_middleware_to_router();
    }

    /// Sync the current state of route middleware groups and aliases to the router.
    pub fn sync_middleware_to_router(&self) {
        let groups = self.middleware_groups.read().unwrap().clone();
        let aliases = self.middleware_aliases.read().unwrap().clone();
        self.get_router().install_middleware(groups, aliases);
    }

    pub fn handle<'a>(&'a self, req: Request) -> BoxFuture<'a, Response> {
        let layers = self.middleware.read().unwrap().clone();
        let router = self.get_router();
        if layers.is_empty() {
            let r = Arc::clone(&router);
            return Box::pin(async move { r.dispatch(req).await });
        }

        let router_clone = Arc::clone(&router);
        let router_handler: HandlerFn = Arc::new(move |req| {
            let r = Arc::clone(&router_clone);
            Box::pin(async move { r.dispatch(req).await })
        });

        Box::pin(async move {
            let next = Next {
                layers: &layers,
                index: 0,
                handler: &router_handler,
            };

            next.run(req).await
        })
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
