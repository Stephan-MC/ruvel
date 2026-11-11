use crate::contracts::support::Responsable;
use crate::http::BoxFuture;
use crate::http::{Method, Middleware, Request, Response};
use crate::routing::pipeline::Pipeline;
use matchit::Router as MatchitRouter;
use std::borrow::Cow;
use std::collections::HashMap;
use std::future::Future;
use std::sync::{Arc, RwLock};

use super::into_middleware::{IntoMiddleware, MiddlewareItem};
use super::pending_route::PendingRouteRegistrar;
use super::route_registrar::RouteRegistrar;

pub type HandlerFn = Arc<dyn Fn(Request) -> BoxFuture<'static, Response> + Send + Sync + 'static>;
type MiddlewareList = Vec<Arc<dyn Middleware>>;
type MiddlewareGroupMap = HashMap<String, MiddlewareList>;
type MiddlewareAliasMap = HashMap<String, Arc<dyn Middleware>>;

#[derive(Clone)]
pub(crate) struct RouteEntry {
    pub(crate) handler: HandlerFn,
    /// Pre-built middleware pipeline (global + group + route middlewares) stored on the route.
    pub(crate) middleware: Pipeline,
    #[allow(dead_code)]
    pub(crate) name: Option<String>,
    #[allow(dead_code)]
    pub(crate) domain: Option<String>,
    #[allow(dead_code)]
    pub(crate) wheres: HashMap<String, String>,
}

#[derive(Clone)]
pub struct Router {
    // Map each HTTP Method to its own Radix Tree of routes
    routes: HashMap<Method, MatchitRouter<RouteEntry>>,
    middlewares: Arc<RwLock<MiddlewareList>>,
    middleware_groups: Arc<RwLock<MiddlewareGroupMap>>,
    aliases: Arc<RwLock<MiddlewareAliasMap>>,

    // Group properties applied to routes
    pub(crate) group_prefix: Option<String>,
    pub(crate) group_domain: Option<String>,
    pub(crate) group_name: Option<String>,
    pub(crate) group_wheres: HashMap<String, String>,
    pub(crate) group_middleware: Vec<MiddlewareItem>,
    pub(crate) group_excluded_middleware: Vec<String>,

    // Cached 404 Fallback
    fallback_handler: HandlerFn,
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
            middlewares: Arc::new(RwLock::new(Vec::new())),
            middleware_groups: Arc::new(RwLock::new(HashMap::new())),
            aliases: Arc::new(RwLock::new(HashMap::new())),
            group_prefix: None,
            group_domain: None,
            group_name: None,
            group_wheres: HashMap::new(),
            group_middleware: Vec::new(),
            group_excluded_middleware: Vec::new(),

            // Allocate the 404 handler ONCE at application boot
            fallback_handler: Arc::new(|_req| {
                Box::pin(async { Response::not_found().text("404 Not Found") })
            }),
        }
    }

    /// Set global middlewares on the router.
    pub fn set_middlewares(&self, global: Vec<Arc<dyn Middleware>>) {
        *self.middlewares.write().unwrap() = global;
    }

    /// Set route middleware groups on the router.
    pub fn set_middleware_groups(&self, groups: HashMap<String, Vec<Arc<dyn Middleware>>>) {
        *self.middleware_groups.write().unwrap() = groups;
    }

    /// Set route middleware aliases on the router.
    pub fn set_middleware_aliases(&self, aliases: HashMap<String, Arc<dyn Middleware>>) {
        *self.aliases.write().unwrap() = aliases;
    }

    /// Install global middleware, groups, and aliases from application configuration.
    pub fn install_middleware(
        &self,
        global: Vec<Arc<dyn Middleware>>,
        groups: HashMap<String, Vec<Arc<dyn Middleware>>>,
        aliases: HashMap<String, Arc<dyn Middleware>>,
    ) {
        *self.middlewares.write().unwrap() = global;
        *self.middleware_groups.write().unwrap() = groups;
        *self.aliases.write().unwrap() = aliases;
    }

    pub(crate) fn combine_prefix(&self, path: &str) -> String {
        let clean_path = path.trim_matches('/');
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

    /// Pre-compiles and resolves all middlewares (global + route-specific) into a [`Pipeline`] at registration time.
    pub(crate) fn compile_route_middlewares(
        &self,
        local_items: &[MiddlewareItem],
        excluded_aliases: &[String],
    ) -> Pipeline {
        let mut pipeline = Pipeline::new();

        // 1. Append global middlewares from the router
        let global = self.middlewares.read().unwrap();
        if !global.is_empty() {
            pipeline = pipeline.append(global.iter().cloned());
        }

        // 2. Local route-specific middleware (aliases, groups, or direct instances)
        for item in local_items {
            pipeline = self.resolve_middleware_item(pipeline, item, excluded_aliases);
        }

        pipeline
    }

    fn resolve_middleware_item(
        &self,
        mut pipeline: Pipeline,
        item: &MiddlewareItem,
        excluded_aliases: &[String],
    ) -> Pipeline {
        match item {
            MiddlewareItem::Alias(name) => {
                if excluded_aliases.contains(name) {
                    return pipeline;
                }
                let aliases = self.aliases.read().unwrap();
                if let Some(alias) = aliases.get(name) {
                    pipeline = pipeline.pipe(Arc::clone(alias));
                    return pipeline;
                }
                let groups = self.middleware_groups.read().unwrap();
                if let Some(group) = groups.get(name) {
                    pipeline = pipeline.append(group.iter().cloned());
                } else {
                    panic!("Unknown middleware [{name}]");
                }
            }
            MiddlewareItem::Instance(instance) => {
                pipeline = pipeline.pipe(Arc::clone(instance));
            }
        }
        pipeline
    }

    /// Register routes defined within the closure.
    pub fn group(&mut self, configure: impl FnOnce(&mut Self)) {
        configure(self);
    }

    pub fn get<F, Fut, R>(&mut self, path: &str, handler: F) -> PendingRouteRegistrar<'_>
    where
        F: Fn(Request) -> Fut + Send + Sync + 'static,
        Fut: Future<Output = R> + Send + 'static,
        R: Responsable,
    {
        <Self as RouteRegistrar>::get(self, path, handler)
    }

    pub fn post<F, Fut, R>(&mut self, path: &str, handler: F) -> PendingRouteRegistrar<'_>
    where
        F: Fn(Request) -> Fut + Send + Sync + 'static,
        Fut: Future<Output = R> + Send + 'static,
        R: Responsable,
    {
        <Self as RouteRegistrar>::post(self, path, handler)
    }

    pub fn put<F, Fut, R>(&mut self, path: &str, handler: F) -> PendingRouteRegistrar<'_>
    where
        F: Fn(Request) -> Fut + Send + Sync + 'static,
        Fut: Future<Output = R> + Send + 'static,
        R: Responsable,
    {
        <Self as RouteRegistrar>::put(self, path, handler)
    }

    pub fn patch<F, Fut, R>(&mut self, path: &str, handler: F) -> PendingRouteRegistrar<'_>
    where
        F: Fn(Request) -> Fut + Send + Sync + 'static,
        Fut: Future<Output = R> + Send + 'static,
        R: Responsable,
    {
        <Self as RouteRegistrar>::patch(self, path, handler)
    }

    pub fn delete<F, Fut, R>(&mut self, path: &str, handler: F) -> PendingRouteRegistrar<'_>
    where
        F: Fn(Request) -> Fut + Send + Sync + 'static,
        Fut: Future<Output = R> + Send + 'static,
        R: Responsable,
    {
        <Self as RouteRegistrar>::delete(self, path, handler)
    }

    pub fn any<F, Fut, R>(&mut self, path: &str, handler: F) -> PendingRouteRegistrar<'_>
    where
        F: Fn(Request) -> Fut + Send + Sync + 'static,
        Fut: Future<Output = R> + Send + 'static,
        R: Responsable,
    {
        <Self as RouteRegistrar>::any(self, path, handler)
    }

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
        <Self as RouteRegistrar>::r#match(self, methods, path, handler)
    }

    pub fn domain(&mut self, domain: impl Into<String>) -> PendingRouteRegistrar<'_> {
        <Self as RouteRegistrar>::domain(self, domain)
    }

    pub fn prefix(&mut self, prefix: impl Into<String>) -> PendingRouteRegistrar<'_> {
        <Self as RouteRegistrar>::prefix(self, prefix)
    }

    pub fn name(&mut self, name: impl Into<String>) -> PendingRouteRegistrar<'_> {
        <Self as RouteRegistrar>::name(self, name)
    }

    pub fn middleware(&mut self, middleware: impl IntoMiddleware) -> PendingRouteRegistrar<'_> {
        <Self as RouteRegistrar>::middleware(self, middleware)
    }

    pub fn without_middleware(
        &mut self,
        middleware: impl IntoMiddleware,
    ) -> PendingRouteRegistrar<'_> {
        <Self as RouteRegistrar>::without_middleware(self, middleware)
    }

    pub fn dispatch(&self, mut req: Request) -> BoxFuture<'_, Response> {
        let lookup_path = {
            let trimmed = req.path.trim_end_matches('/');
            if trimmed.is_empty() { "/" } else { trimmed }
        };

        if let Some(method_router) = self.routes.get(&req.method)
            && let Ok(matched) = method_router.at(lookup_path)
        {
            // Populate inline RouteParams without allocating dynamic HashMap nodes
            for (key, value) in matched.params.iter() {
                req.params
                    .insert(Cow::Owned(key.to_string()), Cow::Owned(value.to_string()));
            }

            let handler = &matched.value.handler;
            let pipeline = &matched.value.middleware;

            // FAST-PATH: Bypass middleware machinery completely if there are no layers
            if pipeline.is_empty() {
                return (handler)(req);
            }

            let pipeline_cloned = pipeline.clone();
            let handler_cloned = Arc::clone(handler);
            return Box::pin(async move {
                pipeline_cloned
                    .send(req)
                    .then(move |r| {
                        let h = Arc::clone(&handler_cloned);
                        async move { (h)(r).await }
                    })
                    .await
            });
        }

        let global = self.middlewares.read().unwrap().clone();
        if global.is_empty() {
            return (self.fallback_handler)(req);
        }

        let fallback = Arc::clone(&self.fallback_handler);
        Box::pin(async move {
            Pipeline::new()
                .send(req)
                .append(global)
                .then(move |r| {
                    let h = Arc::clone(&fallback);
                    async move { (h)(r).await }
                })
                .await
        })
    }
}

impl RouteRegistrar for Router {
    fn get<F, Fut, R>(&mut self, path: &str, handler: F) -> PendingRouteRegistrar<'_>
    where
        F: Fn(Request) -> Fut + Send + Sync + 'static,
        Fut: Future<Output = R> + Send + 'static,
        R: Responsable,
    {
        PendingRouteRegistrar::new(self).get(path, handler)
    }

    fn post<F, Fut, R>(&mut self, path: &str, handler: F) -> PendingRouteRegistrar<'_>
    where
        F: Fn(Request) -> Fut + Send + Sync + 'static,
        Fut: Future<Output = R> + Send + 'static,
        R: Responsable,
    {
        PendingRouteRegistrar::new(self).post(path, handler)
    }

    fn put<F, Fut, R>(&mut self, path: &str, handler: F) -> PendingRouteRegistrar<'_>
    where
        F: Fn(Request) -> Fut + Send + Sync + 'static,
        Fut: Future<Output = R> + Send + 'static,
        R: Responsable,
    {
        PendingRouteRegistrar::new(self).put(path, handler)
    }

    fn patch<F, Fut, R>(&mut self, path: &str, handler: F) -> PendingRouteRegistrar<'_>
    where
        F: Fn(Request) -> Fut + Send + Sync + 'static,
        Fut: Future<Output = R> + Send + 'static,
        R: Responsable,
    {
        PendingRouteRegistrar::new(self).patch(path, handler)
    }

    fn delete<F, Fut, R>(&mut self, path: &str, handler: F) -> PendingRouteRegistrar<'_>
    where
        F: Fn(Request) -> Fut + Send + Sync + 'static,
        Fut: Future<Output = R> + Send + 'static,
        R: Responsable,
    {
        PendingRouteRegistrar::new(self).delete(path, handler)
    }

    fn any<F, Fut, R>(&mut self, path: &str, handler: F) -> PendingRouteRegistrar<'_>
    where
        F: Fn(Request) -> Fut + Send + Sync + 'static,
        Fut: Future<Output = R> + Send + 'static,
        R: Responsable,
    {
        PendingRouteRegistrar::new(self).any(path, handler)
    }

    fn r#match<F, Fut, R>(
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
        PendingRouteRegistrar::new(self).r#match(methods, path, handler)
    }

    fn domain(&mut self, domain: impl Into<String>) -> PendingRouteRegistrar<'_> {
        PendingRouteRegistrar::new(self).domain(domain)
    }

    fn prefix(&mut self, prefix: impl Into<String>) -> PendingRouteRegistrar<'_> {
        PendingRouteRegistrar::new(self).prefix(prefix)
    }

    fn name(&mut self, name: impl Into<String>) -> PendingRouteRegistrar<'_> {
        PendingRouteRegistrar::new(self).name(name)
    }

    fn middleware(&mut self, middleware: impl IntoMiddleware) -> PendingRouteRegistrar<'_> {
        PendingRouteRegistrar::new(self).middleware(middleware)
    }

    fn without_middleware(&mut self, middleware: impl IntoMiddleware) -> PendingRouteRegistrar<'_> {
        PendingRouteRegistrar::new(self).without_middleware(middleware)
    }
}
