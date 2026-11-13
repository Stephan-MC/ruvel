use crate::contracts::support::Responsable;
use crate::http::BoxFuture;
use crate::http::{Method, Middleware, Request, Response};
use crate::routing::pipeline::Pipeline;
use arc_swap::ArcSwap;
use std::borrow::Cow;
use std::collections::HashMap;
use std::future::Future;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, RwLock};

use super::into_middleware::{IntoMiddleware, MiddlewareItem};
use super::pending_resource_registrar::PendingResourceRegistration;
use super::pending_route_registrar::PendingRouteRegistrar;
pub use super::route::HandlerFn;
use super::route::{FxHashMap, MethodSlots, ParamBuf, Route, Segment, parse_param_name};
use super::route_registrar::RouteRegistrar;
use crate::http::Controller;

type MiddlewareList = Vec<Arc<dyn Middleware>>;
type MiddlewareGroupMap = HashMap<String, MiddlewareList>;
type MiddlewareAliasMap = HashMap<String, Arc<dyn Middleware>>;

#[derive(Clone)]
pub struct Router {
    static_routes: FxHashMap<Box<str>, MethodSlots>,
    routes: Segment,
    middlewares: Arc<RwLock<MiddlewareList>>,
    middleware_groups: Arc<RwLock<MiddlewareGroupMap>>,
    aliases: Arc<RwLock<MiddlewareAliasMap>>,

    // Cached 404 Fallback and pre-composed global middleware + 404 chain
    fallback_handler: HandlerFn,
    fallback_chain: Arc<ArcSwap<HandlerFn>>,
    has_global_middlewares: Arc<AtomicBool>,
}

impl Default for Router {
    fn default() -> Self {
        Self::new()
    }
}

impl Router {
    pub fn new() -> Self {
        // Allocate the 404 handler ONCE at application boot
        let fallback_handler: HandlerFn =
            Arc::new(|_req| Box::pin(async { Response::not_found().text("404 Not Found") }));
        let fallback_chain = Arc::new(ArcSwap::from_pointee(Arc::clone(&fallback_handler)));

        Self {
            static_routes: FxHashMap::default(),
            routes: Segment::new(),
            middlewares: Arc::new(RwLock::new(Vec::new())),
            middleware_groups: Arc::new(RwLock::new(HashMap::new())),
            aliases: Arc::new(RwLock::new(HashMap::new())),
            fallback_handler,
            fallback_chain,
            has_global_middlewares: Arc::new(AtomicBool::new(false)),
        }
    }

    fn rebuild_fallback_chain(&self, global: &[Arc<dyn Middleware>]) {
        if global.is_empty() {
            self.fallback_chain
                .store(Arc::new(Arc::clone(&self.fallback_handler)));
            self.has_global_middlewares.store(false, Ordering::Release);
        } else {
            let chain = Pipeline::new()
                .append(global.iter().cloned())
                .compose(Arc::clone(&self.fallback_handler));
            self.fallback_chain.store(Arc::new(chain));
            self.has_global_middlewares.store(true, Ordering::Release);
        }
    }

    /// Set global middlewares on the router.
    pub fn set_middlewares(&self, global: Vec<Arc<dyn Middleware>>) {
        let mut guard = self.middlewares.write().unwrap();
        *guard = global;
        self.rebuild_fallback_chain(&guard);
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
        {
            let mut guard = self.middlewares.write().unwrap();
            *guard = global;
            self.rebuild_fallback_chain(&guard);
        }
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

    #[allow(dead_code)]
    pub(crate) fn insert_route(&mut self, method: Method, path: &str, route: Route) {
        self.insert_route_arc(&[method], path, Arc::new(route));
    }

    pub(crate) fn insert_route_for_methods(
        &mut self,
        methods: &[Method],
        path: &str,
        route: Route,
    ) {
        if methods.is_empty() {
            return;
        }
        self.insert_route_arc(methods, path, Arc::new(route));
    }

    fn insert_route_arc(&mut self, methods: &[Method], path: &str, route: Arc<Route>) {
        let trimmed = path.trim_matches('/');

        if trimmed.is_empty() {
            let slots = self.static_routes.entry(Box::from("")).or_default();
            for method in methods {
                slots.insert(method.clone(), Arc::clone(&route));
                self.routes
                    .insert_arc(method.clone(), &[], Arc::clone(&route));
            }
            return;
        }

        let segments: Vec<&str> = trimmed.split('/').filter(|s| !s.is_empty()).collect();
        let is_static = segments.iter().all(|s| parse_param_name(s).is_none());

        if is_static {
            let key: Box<str> = if !trimmed.contains("//") {
                Box::from(trimmed)
            } else {
                Box::from(segments.join("/"))
            };
            let slots = self.static_routes.entry(key).or_default();
            for method in methods {
                slots.insert(method.clone(), Arc::clone(&route));
            }
        }

        for method in methods {
            self.routes
                .insert_arc(method.clone(), &segments, Arc::clone(&route));
        }
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

    pub fn dispatch(&self, mut req: Request) -> BoxFuture<'static, Response> {
        let trimmed = req.path.trim_matches('/');

        // Tier 1: O(1) static fast-path. If the path exists in the static map for another method,
        // fall through to the trie so dynamic parameter routes can still match.
        if let Some(slots) = self.static_routes.get(trimmed)
            && let Some(route) = slots.get(&req.method)
        {
            return (route.chain)(req);
        }

        // Tier 2: Segment trie lookup on `&str` slices with a stack-allocated parameter buffer.
        let mut params = ParamBuf::new();
        if let Some(route) = self.routes.lookup(&req.method, trimmed, &mut params) {
            for &(key, value) in params.as_slice() {
                req.params
                    .insert(Cow::Borrowed(key), Cow::Owned(value.to_owned()));
            }
            return (route.chain)(req);
        }

        // 404 fallback: avoid ArcSwap load overhead when no global middleware is registered.
        if !self.has_global_middlewares.load(Ordering::Acquire) {
            return (self.fallback_handler)(req);
        }
        let guard = self.fallback_chain.load();
        (***guard)(req)
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
