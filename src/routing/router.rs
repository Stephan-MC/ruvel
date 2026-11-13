use crate::contracts::support::Responsable;
use crate::http::BoxFuture;
use crate::http::{Method, Middleware, Request, Response};
use crate::routing::pipeline::Pipeline;
use std::borrow::Cow;
use std::collections::HashMap;
use std::future::Future;
use std::sync::{Arc, RwLock};

use super::into_middleware::{IntoMiddleware, MiddlewareItem};
use super::pending_resource_registrar::PendingResourceRegistration;
use super::pending_route_registrar::PendingRouteRegistrar;
pub use super::route::HandlerFn;
use super::route::{Route, Segment};
use super::route_registrar::RouteRegistrar;
use crate::http::Controller;

type MiddlewareList = Vec<Arc<dyn Middleware>>;
type MiddlewareGroupMap = HashMap<String, MiddlewareList>;
type MiddlewareAliasMap = HashMap<String, Arc<dyn Middleware>>;

#[derive(Clone)]
pub struct Router {
    routes: Segment,
    middlewares: Arc<RwLock<MiddlewareList>>,
    middleware_groups: Arc<RwLock<MiddlewareGroupMap>>,
    aliases: Arc<RwLock<MiddlewareAliasMap>>,

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
            routes: Segment::new(),
            middlewares: Arc::new(RwLock::new(Vec::new())),
            middleware_groups: Arc::new(RwLock::new(HashMap::new())),
            aliases: Arc::new(RwLock::new(HashMap::new())),

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

    /// Check whether a middleware group is registered on the router.
    pub fn has_middleware_group(&self, name: &str) -> bool {
        self.middleware_groups.read().unwrap().contains_key(name)
    }

    /// Check whether a middleware alias is registered on the router.
    pub fn has_middleware_alias(&self, name: &str) -> bool {
        self.aliases.read().unwrap().contains_key(name)
    }

    pub(crate) fn insert_route(&mut self, method: Method, path: &str, route: Route) {
        let clean_path = if path != "/" {
            path.trim_end_matches('/')
        } else {
            path
        };
        let segments: Vec<&str> = if clean_path == "/" || clean_path.is_empty() {
            Vec::new()
        } else {
            clean_path.trim_matches('/').split('/').collect()
        };
        self.routes.insert(method, &segments, route);
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

    /// Register routes defined within the closure, passing `&mut Router` directly.
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

    pub fn resource<C: Controller + 'static>(
        &mut self,
        name: &str,
        controller: C,
    ) -> PendingResourceRegistration<'_> {
        <Self as RouteRegistrar>::resource(self, name, controller)
    }

    pub fn api_resource<C: Controller + 'static>(
        &mut self,
        name: &str,
        controller: C,
    ) -> PendingResourceRegistration<'_> {
        <Self as RouteRegistrar>::api_resource(self, name, controller)
    }

    pub fn dispatch(&self, mut req: Request) -> BoxFuture<'_, Response> {
        let matched = {
            let clean_path = if req.path != "/" {
                req.path.trim_end_matches('/')
            } else {
                req.path.as_ref()
            };
            let segments: Vec<&str> = if clean_path == "/" || clean_path.is_empty() {
                Vec::new()
            } else {
                clean_path.trim_matches('/').split('/').collect()
            };

            let mut params = Vec::new();
            self.routes
                .lookup(&req.method, &segments, &mut params)
                .map(|route| {
                    let extracted_params: Vec<(String, String)> = params
                        .into_iter()
                        .map(|(k, v)| (k.to_string(), v.to_string()))
                        .collect();
                    (
                        route.handler.clone(),
                        route.middleware.clone(),
                        extracted_params,
                    )
                })
        };

        if let Some((handler, pipeline, extracted_params)) = matched {
            // Populate inline RouteParams without allocating dynamic HashMap nodes
            for (key, value) in extracted_params {
                req.params.insert(Cow::Owned(key), Cow::Owned(value));
            }

            // FAST-PATH: Bypass middleware machinery completely if there are no layers
            if pipeline.is_empty() {
                return (handler)(req);
            }

            return Box::pin(async move {
                pipeline
                    .send(req)
                    .then(move |r| {
                        let h = Arc::clone(&handler);
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

    fn resource<C: Controller + 'static>(
        &mut self,
        name: &str,
        controller: C,
    ) -> PendingResourceRegistration<'_> {
        PendingResourceRegistration::new(
            self,
            name,
            controller,
            None,
            None,
            None,
            Vec::new(),
            Vec::new(),
            HashMap::new(),
        )
    }
}
