use crate::contracts::support::Responsable;
use crate::http::{BoxFuture, Next};
use crate::http::{Method, Middleware, Request, Response};
use matchit::Router as MatchitRouter;
use std::borrow::Cow;
use std::collections::HashMap;
use std::future::Future;
use std::sync::Arc;

pub type HandlerFn = Arc<dyn Fn(Request) -> BoxFuture<'static, Response> + Send + Sync + 'static>;

#[derive(Clone)]
struct RouteEntry {
    handler: HandlerFn,
    /// Pre-resolved, flattened pipeline stored natively on the route.
    pipeline: Arc<[Arc<dyn Middleware>]>,
}

pub struct Router {
    // Map each HTTP Method to its own Radix Tree of routes
    routes: HashMap<Method, MatchitRouter<RouteEntry>>,
    global: Vec<Arc<dyn Middleware>>,
    groups: HashMap<String, Vec<Arc<dyn Middleware>>>,
    aliases: HashMap<String, Arc<dyn Middleware>>,
    /// Names applied to routes registered inside [`Router::group`].
    stack: Vec<String>,

    // Cached 404 Fallbacks to prevent allocations on unmatched routes
    fallback_handler: HandlerFn,
    fallback_pipeline: Arc<[Arc<dyn Middleware>]>,
}

impl Default for Router {
    fn default() -> Self {
        Self::new()
    }
}

impl Router {
    pub fn new() -> Self {
        Self {
            routes: HashMap::new(),
            global: Vec::new(),
            groups: HashMap::new(),
            aliases: HashMap::new(),
            stack: Vec::new(),

            // Allocate the 404 handler ONCE at application boot
            fallback_handler: Arc::new(|_req| {
                Box::pin(async { Response::not_found().text("404 Not Found") })
            }),
            fallback_pipeline: Arc::new([]),
        }
    }

    /// Install global middleware, groups, and aliases from application configuration.
    pub(crate) fn install_middleware(
        &mut self,
        global: Vec<Arc<dyn Middleware>>,
        groups: HashMap<String, Vec<Arc<dyn Middleware>>>,
        aliases: HashMap<String, Arc<dyn Middleware>>,
    ) {
        self.global = global.clone();
        self.groups = groups;
        self.aliases = aliases;

        // Ensure 404 routes still pass through global middleware
        self.fallback_pipeline = global.into();
    }

    /// Register `configure`'s routes with these group or alias names.
    pub fn group(&mut self, middleware: &[&str], configure: impl FnOnce(&mut Self)) {
        let start = self.stack.len();
        self.stack
            .extend(middleware.iter().copied().map(str::to_string));
        configure(self);
        self.stack.truncate(start);
    }

    /// Pre-compiles the pipeline for a route at registration time.
    fn compile_pipeline(&self, local_names: &[String]) -> Arc<[Arc<dyn Middleware>]> {
        let mut layers = self.global.clone();

        for name in local_names {
            if let Some(group) = self.groups.get(name) {
                layers.extend(group.iter().cloned());
            } else if let Some(alias) = self.aliases.get(name) {
                layers.push(Arc::clone(alias));
            } else {
                panic!("Unknown middleware [{name}]");
            }
        }

        layers.into() // Convert Vec to fixed-size Arc slice
    }

    pub fn add_route<F, Fut, R>(&mut self, method: Method, path: &str, handler: F)
    where
        F: Fn(Request) -> Fut + Send + Sync + 'static,
        Fut: Future<Output = R> + Send + 'static,
        R: Responsable,
    {
        let h: HandlerFn = Arc::new(move |req| {
            let fut = handler(req);
            Box::pin(async move { fut.await.to_response() })
        });

        // Resolve all middleware during registration, NOT during dispatch
        let pipeline = self.compile_pipeline(&self.stack);

        let entry = RouteEntry {
            handler: h,
            pipeline,
        };

        // Get the radix tree for this HTTP Method, or create one if it doesn't exist
        let method_router = self.routes.entry(method).or_insert_with(MatchitRouter::new);

        // Insert the route. Using unwrap() because a syntax error in route
        // definitions (e.g., overlapping params) should panic at startup.
        method_router.insert(path, entry).unwrap();
    }

    pub fn get<F, Fut, R>(&mut self, path: &str, handler: F)
    where
        F: Fn(Request) -> Fut + Send + Sync + 'static,
        Fut: Future<Output = R> + Send + 'static,
        R: Responsable,
    {
        self.add_route(Method::GET, path, handler);
    }

    pub fn post<F, Fut, R>(&mut self, path: &str, handler: F)
    where
        F: Fn(Request) -> Fut + Send + Sync + 'static,
        Fut: Future<Output = R> + Send + 'static,
        R: Responsable,
    {
        self.add_route(Method::POST, path, handler);
    }

    pub fn put<F, Fut, R>(&mut self, path: &str, handler: F)
    where
        F: Fn(Request) -> Fut + Send + Sync + 'static,
        Fut: Future<Output = R> + Send + 'static,
        R: Responsable,
    {
        self.add_route(Method::PUT, path, handler);
    }

    pub fn delete<F, Fut, R>(&mut self, path: &str, handler: F)
    where
        F: Fn(Request) -> Fut + Send + Sync + 'static,
        Fut: Future<Output = R> + Send + 'static,
        R: Responsable,
    {
        self.add_route(Method::DELETE, path, handler);
    }

    pub fn patch<F, Fut, R>(&mut self, path: &str, handler: F)
    where
        F: Fn(Request) -> Fut + Send + Sync + 'static,
        Fut: Future<Output = R> + Send + 'static,
        R: Responsable,
    {
        self.add_route(Method::PATCH, path, handler);
    }

    pub fn dispatch(&self, mut req: Request) -> BoxFuture<'_, Response> {
        let (handler, pipeline) = if let Some(method_router) = self.routes.get(&req.method) {
            if let Ok(matched) = method_router.at(&req.path) {
                // Populate inline RouteParams without allocating dynamic HashMap nodes
                for (key, value) in matched.params.iter() {
                    req.params.insert(Cow::Owned(key.to_string()), Cow::Owned(value.to_string()));
                }

                (&matched.value.handler, &matched.value.pipeline)
            } else {
                (&self.fallback_handler, &self.fallback_pipeline)
            }
        } else {
            (&self.fallback_handler, &self.fallback_pipeline)
        };

        // FAST-PATH: Bypass middleware machinery completely if there are no layers
        if pipeline.is_empty() {
            return (handler)(req);
        }

        // We explicitly wrap the `Next` run in a BoxFuture here so it matches the return type
        Box::pin(async move {
            let next = Next {
                layers: pipeline,
                index: 0,
                handler,
            };

            next.run(req).await
        })
    }
}
