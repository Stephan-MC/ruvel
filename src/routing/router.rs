use crate::contracts::support::Responsable;
use crate::http::{BoxFuture, Next};
use crate::http::{Method, Middleware, Request, Response};
use matchit::Router as MatchitRouter;
use std::borrow::Cow;
use std::collections::HashMap;
use std::future::Future;
use std::sync::Arc;

use super::into_middleware::{IntoMiddleware, MiddlewareItem};
use super::pending_route::{PendingRouteRegistrar, RouteGroupRegistrar};

pub type HandlerFn = Arc<dyn Fn(Request) -> BoxFuture<'static, Response> + Send + Sync + 'static>;

#[derive(Clone)]
pub(crate) struct RouteEntry {
    pub(crate) handler: HandlerFn,
    /// Pre-resolved, flattened pipeline stored natively on the route.
    pub(crate) pipeline: Arc<[Arc<dyn Middleware>]>,
    #[allow(dead_code)]
    pub(crate) name: Option<String>,
    #[allow(dead_code)]
    pub(crate) domain: Option<String>,
    #[allow(dead_code)]
    pub(crate) wheres: HashMap<String, String>,
}

pub struct Router {
    // Map each HTTP Method to its own Radix Tree of routes
    routes: HashMap<Method, MatchitRouter<RouteEntry>>,
    global: Vec<Arc<dyn Middleware>>,
    groups: HashMap<String, Vec<Arc<dyn Middleware>>>,
    aliases: HashMap<String, Arc<dyn Middleware>>,

    // Group properties applied to routes
    pub(crate) group_prefix: Option<String>,
    pub(crate) group_middleware: Vec<MiddlewareItem>,
    pub(crate) group_domain: Option<String>,

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
            group_prefix: None,
            group_middleware: Vec::new(),
            group_domain: None,

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

    pub(crate) fn combine_prefix(&self, path: &str) -> String {
        let clean_path = path.trim_start_matches('/');
        match &self.group_prefix {
            Some(prefix) => {
                let trimmed = prefix.trim_matches('/');
                if trimmed.is_empty() {
                    if clean_path.is_empty() {
                        "/".to_string()
                    } else {
                        format!("/{}", clean_path)
                    }
                } else if clean_path.is_empty() {
                    format!("/{}", trimmed)
                } else {
                    format!("/{}/{}", trimmed, clean_path)
                }
            }
            None => {
                if clean_path.is_empty() {
                    "/".to_string()
                } else {
                    format!("/{}", clean_path)
                }
            }
        }
    }

    pub(crate) fn insert_route(&mut self, method: Method, path: &str, entry: RouteEntry) {
        let method_router = self.routes.entry(method).or_default();
        method_router.insert(path, entry).unwrap();
    }

    /// Pre-compiles the pipeline for a route at registration time.
    pub(crate) fn compile_route_pipeline(
        &self,
        local_items: &[MiddlewareItem],
        excluded_aliases: &[String],
    ) -> Arc<[Arc<dyn Middleware>]> {
        let mut layers = self.global.clone();

        // 1. Group middleware on the router
        for item in &self.group_middleware {
            self.resolve_middleware_item(item, &mut layers, excluded_aliases);
        }

        // 2. Local route-specific middleware
        for item in local_items {
            self.resolve_middleware_item(item, &mut layers, excluded_aliases);
        }

        layers.into()
    }

    fn resolve_middleware_item(
        &self,
        item: &MiddlewareItem,
        layers: &mut Vec<Arc<dyn Middleware>>,
        excluded_aliases: &[String],
    ) {
        match item {
            MiddlewareItem::Alias(name) => {
                if excluded_aliases.contains(name) {
                    return;
                }
                if let Some(group) = self.groups.get(name) {
                    layers.extend(group.iter().cloned());
                } else if let Some(alias) = self.aliases.get(name) {
                    layers.push(Arc::clone(alias));
                } else {
                    panic!("Unknown middleware [{name}]");
                }
            }
            MiddlewareItem::Instance(instance) => {
                layers.push(Arc::clone(instance));
            }
        }
    }

    /// Register routes defined within the closure.
    pub fn group(&mut self, configure: impl FnOnce(&mut Self)) {
        configure(self);
    }

    /// Begin a route group with a path prefix.
    pub fn prefix(&mut self, prefix: impl Into<String>) -> RouteGroupRegistrar<'_> {
        RouteGroupRegistrar::new(self).prefix(prefix)
    }

    /// Begin a route group with middleware.
    pub fn middleware(&mut self, middleware: impl IntoMiddleware) -> RouteGroupRegistrar<'_> {
        RouteGroupRegistrar::new(self).middleware(middleware)
    }

    /// Begin a route group with a domain.
    pub fn domain(&mut self, domain: impl Into<String>) -> RouteGroupRegistrar<'_> {
        RouteGroupRegistrar::new(self).domain(domain)
    }

    pub fn add_route<F, Fut, R>(
        &mut self,
        method: Method,
        path: &str,
        handler: F,
    ) -> PendingRouteRegistrar<'_>
    where
        F: Fn(Request) -> Fut + Send + Sync + 'static,
        Fut: Future<Output = R> + Send + 'static,
        R: Responsable,
    {
        let h: HandlerFn = Arc::new(move |req| {
            let fut = handler(req);
            Box::pin(async move { fut.await.to_response() })
        });

        PendingRouteRegistrar::new(self, vec![method], path, h)
    }

    pub fn get<F, Fut, R>(&mut self, path: &str, handler: F) -> PendingRouteRegistrar<'_>
    where
        F: Fn(Request) -> Fut + Send + Sync + 'static,
        Fut: Future<Output = R> + Send + 'static,
        R: Responsable,
    {
        self.add_route(Method::GET, path, handler)
    }

    pub fn post<F, Fut, R>(&mut self, path: &str, handler: F) -> PendingRouteRegistrar<'_>
    where
        F: Fn(Request) -> Fut + Send + Sync + 'static,
        Fut: Future<Output = R> + Send + 'static,
        R: Responsable,
    {
        self.add_route(Method::POST, path, handler)
    }

    pub fn put<F, Fut, R>(&mut self, path: &str, handler: F) -> PendingRouteRegistrar<'_>
    where
        F: Fn(Request) -> Fut + Send + Sync + 'static,
        Fut: Future<Output = R> + Send + 'static,
        R: Responsable,
    {
        self.add_route(Method::PUT, path, handler)
    }

    pub fn delete<F, Fut, R>(&mut self, path: &str, handler: F) -> PendingRouteRegistrar<'_>
    where
        F: Fn(Request) -> Fut + Send + Sync + 'static,
        Fut: Future<Output = R> + Send + 'static,
        R: Responsable,
    {
        self.add_route(Method::DELETE, path, handler)
    }

    pub fn patch<F, Fut, R>(&mut self, path: &str, handler: F) -> PendingRouteRegistrar<'_>
    where
        F: Fn(Request) -> Fut + Send + Sync + 'static,
        Fut: Future<Output = R> + Send + 'static,
        R: Responsable,
    {
        self.add_route(Method::PATCH, path, handler)
    }

    /// Register a route that responds to all standard HTTP methods.
    pub fn any<F, Fut, R>(&mut self, path: &str, handler: F) -> PendingRouteRegistrar<'_>
    where
        F: Fn(Request) -> Fut + Send + Sync + 'static,
        Fut: Future<Output = R> + Send + 'static,
        R: Responsable,
    {
        let methods = vec![
            Method::GET,
            Method::POST,
            Method::PUT,
            Method::DELETE,
            Method::PATCH,
        ];
        let h: HandlerFn = Arc::new(move |req| {
            let fut = handler(req);
            Box::pin(async move { fut.await.to_response() })
        });

        PendingRouteRegistrar::new(self, methods, path, h)
    }

    /// Register a route with multiple specific HTTP methods.
    pub fn r#match<F, Fut, R>(
        &mut self,
        methods: &[Method],
        path: &str,
        handler: F,
    ) -> PendingRouteRegistrar<'_>
    where
        F: Fn(Request) -> Fut + Send + Sync + 'static,
        Fut: Future<Output = R> + Send + 'static,
        R: Responsable,
    {
        let h: HandlerFn = Arc::new(move |req| {
            let fut = handler(req);
            Box::pin(async move { fut.await.to_response() })
        });

        PendingRouteRegistrar::new(self, methods.to_vec(), path, h)
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
