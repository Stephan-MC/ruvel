use crate::configuration::Middleware as MiddlewareConfig;
use crate::contracts;
use crate::foundation::Application;
use crate::http::{BoxFuture, HandlerFn, Middleware, Next, Request, Response};
use crate::routing::Router;
use std::collections::HashMap;
use std::sync::{Arc, RwLock};

pub struct Kernel {
    pub router: Arc<Router>,
    pub middleware: Arc<[Arc<dyn Middleware>]>,
    pub middleware_groups: Arc<RwLock<HashMap<String, Vec<Arc<dyn Middleware>>>>>,
    pub middleware_aliases: Arc<RwLock<HashMap<String, Arc<dyn Middleware>>>>,
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
            middleware_groups: Arc::new(RwLock::new(HashMap::new())),
            middleware_aliases: Arc::new(RwLock::new(HashMap::new())),
            router_handler,
        }
    }

    pub fn sync_router_handler(&mut self) {
        let r = Arc::clone(&self.router);
        self.router_handler = Arc::new(move |req| {
            let r = Arc::clone(&r);
            Box::pin(async move { r.dispatch(req).await })
        });
    }

    /// Set global middleware on the kernel.
    pub fn set_global_middleware(&mut self, middleware: Vec<Arc<dyn Middleware>>) -> &mut Self {
        self.middleware = middleware.into();
        self
    }

    /// Set route middleware groups on the kernel and sync to the router.
    pub fn set_middleware_groups(
        &mut self,
        groups: HashMap<String, Vec<Arc<dyn Middleware>>>,
    ) -> &mut Self {
        *self.middleware_groups.write().unwrap() = groups;
        self.sync_middleware_to_router();
        self
    }

    /// Set route middleware aliases on the kernel and sync to the router.
    pub fn set_middleware_aliases(
        &mut self,
        aliases: HashMap<String, Arc<dyn Middleware>>,
    ) -> &mut Self {
        *self.middleware_aliases.write().unwrap() = aliases;
        self.sync_middleware_to_router();
        self
    }

    /// Configure middleware via the Laravel-style Middleware configuration object.
    pub fn configure_middleware<F>(&mut self, callback: F) -> &mut Self
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
        self
    }

    /// Sync the current state of route middleware groups and aliases to the router.
    pub fn sync_middleware_to_router(&self) {
        let groups = self.middleware_groups.read().unwrap().clone();
        let aliases = self.middleware_aliases.read().unwrap().clone();
        self.router.install_middleware(groups, aliases);
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
            middleware_groups: Arc::new(RwLock::new(HashMap::new())),
            middleware_aliases: Arc::new(RwLock::new(HashMap::new())),
            router_handler,
        }
    }
}

impl contracts::http::Kernel for Kernel {
    fn handle<'a>(&'a self, req: Request) -> BoxFuture<'a, Response> {
        self.handle(req)
    }
}
