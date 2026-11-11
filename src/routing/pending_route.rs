use super::into_middleware::{IntoMiddleware, MiddlewareItem};
use super::route_registrar::RouteRegistrar;
use super::router::{HandlerFn, RouteEntry, Router};
use crate::contracts::support::Responsable;
use crate::http::{Method, Request};
use std::collections::HashMap;
use std::future::Future;
use std::sync::Arc;

/// A fluent builder for configuring a pending route or route group before it is
/// committed to the [`Router`].
///
/// Returned by [`RouteRegistrar`] methods on [`Router`] and [`PendingRouteRegistrar`].
/// When configuring a single route (where a handler is attached via `.get(...)`, `.post(...)`, etc.),
/// dropping `PendingRouteRegistrar` automatically commits the route into the underlying `Router`.
/// When configuring a route group, call `.group(|router| { ... })`, which passes `&mut PendingRouteRegistrar`
/// into the closure so child routes and nested groups inherit the group's attributes directly.
pub struct PendingRouteRegistrar<'a> {
    router: &'a mut Router,
    methods: Vec<Method>,
    path: Option<String>,
    prefix: Option<String>,
    name: Option<String>,
    domain: Option<String>,
    wheres: HashMap<String, String>,
    handler: Option<HandlerFn>,
    middleware: Vec<MiddlewareItem>,
    excluded_middleware: Vec<String>,
    committed: bool,
}

impl<'a> PendingRouteRegistrar<'a> {
    /// Create a new empty `PendingRouteRegistrar` bound to the given [`Router`].
    pub fn new(router: &'a mut Router) -> Self {
        Self {
            router,
            methods: Vec::new(),
            path: None,
            prefix: None,
            name: None,
            domain: None,
            wheres: HashMap::new(),
            handler: None,
            middleware: Vec::new(),
            excluded_middleware: Vec::new(),
            committed: false,
        }
    }

    /// Create a child `PendingRouteRegistrar` that inherits group attributes
    /// (`prefix`, `name`, `domain`, `wheres`, `middleware`, `excluded_middleware`)
    /// from the current `PendingRouteRegistrar`.
    fn inherit_from<'b>(&'b mut self) -> PendingRouteRegistrar<'b> {
        PendingRouteRegistrar {
            router: &mut *self.router,
            methods: Vec::new(),
            path: None,
            prefix: self.prefix.clone(),
            name: self.name.clone(),
            domain: self.domain.clone(),
            wheres: self.wheres.clone(),
            handler: None,
            middleware: self.middleware.clone(),
            excluded_middleware: self.excluded_middleware.clone(),
            committed: false,
        }
    }

    /// Register a route with a single HTTP method on this registrar.
    pub fn add_route<F, Fut, R>(mut self, method: Method, path: &str, handler: F) -> Self
    where
        F: Fn(Request) -> Fut + Send + Sync + 'static,
        Fut: Future<Output = R> + Send + 'static,
        R: Responsable,
    {
        let h: HandlerFn = Arc::new(move |req| {
            let fut = handler(req);
            Box::pin(async move { fut.await.to_response() })
        });

        self.methods = vec![method];
        self.path = Some(path.to_string());
        self.handler = Some(h);
        self
    }

    /// Register a GET route on this registrar.
    pub fn get<F, Fut, R>(self, path: &str, handler: F) -> Self
    where
        F: Fn(Request) -> Fut + Send + Sync + 'static,
        Fut: Future<Output = R> + Send + 'static,
        R: Responsable,
    {
        self.add_route(Method::GET, path, handler)
    }

    /// Register a POST route on this registrar.
    pub fn post<F, Fut, R>(self, path: &str, handler: F) -> Self
    where
        F: Fn(Request) -> Fut + Send + Sync + 'static,
        Fut: Future<Output = R> + Send + 'static,
        R: Responsable,
    {
        self.add_route(Method::POST, path, handler)
    }

    /// Register a PUT route on this registrar.
    pub fn put<F, Fut, R>(self, path: &str, handler: F) -> Self
    where
        F: Fn(Request) -> Fut + Send + Sync + 'static,
        Fut: Future<Output = R> + Send + 'static,
        R: Responsable,
    {
        self.add_route(Method::PUT, path, handler)
    }

    /// Register a PATCH route on this registrar.
    pub fn patch<F, Fut, R>(self, path: &str, handler: F) -> Self
    where
        F: Fn(Request) -> Fut + Send + Sync + 'static,
        Fut: Future<Output = R> + Send + 'static,
        R: Responsable,
    {
        self.add_route(Method::PATCH, path, handler)
    }

    /// Register a DELETE route on this registrar.
    pub fn delete<F, Fut, R>(self, path: &str, handler: F) -> Self
    where
        F: Fn(Request) -> Fut + Send + Sync + 'static,
        Fut: Future<Output = R> + Send + 'static,
        R: Responsable,
    {
        self.add_route(Method::DELETE, path, handler)
    }

    /// Register a route that responds to all standard HTTP methods.
    pub fn any<F, Fut, R>(mut self, path: &str, handler: F) -> Self
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

        self.methods = methods;
        self.path = Some(path.to_string());
        self.handler = Some(h);
        self
    }

    /// Register a route with multiple specific HTTP methods.
    pub fn r#match<F, Fut, R>(mut self, methods: &[Method], path: &str, handler: F) -> Self
    where
        F: Fn(Request) -> Fut + Send + Sync + 'static,
        Fut: Future<Output = R> + Send + 'static,
        R: Responsable,
    {
        let h: HandlerFn = Arc::new(move |req| {
            let fut = handler(req);
            Box::pin(async move { fut.await.to_response() })
        });

        self.methods = methods.to_vec();
        self.path = Some(path.to_string());
        self.handler = Some(h);
        self
    }

    /// Attach middleware to this route or route group.
    ///
    /// Accepts alias names (`&str`, `String`), slices/arrays/Vecs of aliases,
    /// or `Arc<dyn Middleware>` instances.
    pub fn middleware(mut self, middleware: impl IntoMiddleware) -> Self {
        self.middleware.extend(middleware.into_middleware_items());
        self
    }

    /// Exclude specified middleware alias/group names from this route or route group's pipeline.
    pub fn without_middleware(mut self, middleware: impl IntoMiddleware) -> Self {
        for item in middleware.into_middleware_items() {
            if let MiddlewareItem::Alias(name) = item {
                self.excluded_middleware.push(name);
            }
        }
        self
    }

    /// Set the name (or group name prefix) for this route or route group.
    pub fn name(mut self, name: impl Into<String>) -> Self {
        let incoming = name.into();
        self.name = Some(match self.name.take() {
            Some(existing) => format!("{}{}", existing, incoming),
            None => incoming,
        });
        self
    }

    /// Set an additional path prefix for this route or route group.
    pub fn prefix(mut self, prefix: impl Into<String>) -> Self {
        let incoming = prefix.into();
        self.prefix = Some(match self.prefix.take() {
            Some(existing) => {
                let existing_clean = existing.trim_matches('/');
                let incoming_clean = incoming.trim_matches('/');
                if existing_clean.is_empty() {
                    incoming_clean.to_string()
                } else if incoming_clean.is_empty() {
                    existing_clean.to_string()
                } else {
                    format!("{}/{}", existing_clean, incoming_clean)
                }
            }
            None => incoming,
        });
        self
    }

    /// Set a domain restriction or metadata for this route or route group.
    pub fn domain(mut self, domain: impl Into<String>) -> Self {
        self.domain = Some(domain.into());
        self
    }

    /// Add a regular expression / pattern constraint on a route parameter.
    pub fn r#where(mut self, param: &str, pattern: &str) -> Self {
        self.wheres.insert(param.to_string(), pattern.to_string());
        self
    }

    /// Add a numeric constraint on a route parameter (e.g. `^[0-9]+$`).
    pub fn where_number(self, param: &str) -> Self {
        self.r#where(param, r"^[0-9]+$")
    }

    /// Add an alphabetic constraint on a route parameter (e.g. `^[a-zA-Z]+$`).
    pub fn where_alpha(self, param: &str) -> Self {
        self.r#where(param, r"^[a-zA-Z]+$")
    }

    /// Execute the callback registering all routes within a group sharing this registrar's attributes.
    ///
    /// Passes `&mut PendingRouteRegistrar` to `callback` so any routes or nested groups registered
    /// inside the closure inherit this group's attributes directly.
    pub fn group<F>(mut self, callback: F)
    where
        F: FnOnce(&mut Self),
    {
        self.committed = true;
        callback(&mut self);
    }

    /// Commit the route into the router immediately.
    pub fn commit(&mut self) {
        if self.committed {
            return;
        }
        self.committed = true;

        let Some(ref handler) = self.handler else {
            return;
        };

        let raw_path = self.path.as_deref().unwrap_or("/");
        let trimmed_path = raw_path.trim_matches('/');

        let final_path = match &self.prefix {
            Some(p) => {
                let trimmed_prefix = p.trim_matches('/');
                if trimmed_prefix.is_empty() {
                    if trimmed_path.is_empty() {
                        "/".to_string()
                    } else {
                        format!("/{}", trimmed_path)
                    }
                } else if trimmed_path.is_empty() {
                    format!("/{}", trimmed_prefix)
                } else {
                    format!("/{}/{}", trimmed_prefix, trimmed_path)
                }
            }
            None => {
                if trimmed_path.is_empty() {
                    "/".to_string()
                } else {
                    format!("/{}", trimmed_path)
                }
            }
        };

        // Build the route's middleware pipeline (global + group/route-specific, minus excluded)
        let middleware = self
            .router
            .compile_route_middlewares(&self.middleware, &self.excluded_middleware);

        let entry = RouteEntry {
            handler: Arc::clone(handler),
            middleware,
            name: self.name.clone(),
            domain: self.domain.clone(),
            wheres: self.wheres.clone(),
        };

        for method in &self.methods {
            self.router
                .insert_route(method.clone(), &final_path, entry.clone());
        }
    }
}

impl<'a> RouteRegistrar for PendingRouteRegistrar<'a> {
    fn get<F, Fut, R>(&mut self, path: &str, handler: F) -> PendingRouteRegistrar<'_>
    where
        F: Fn(Request) -> Fut + Send + Sync + 'static,
        Fut: Future<Output = R> + Send + 'static,
        R: Responsable,
    {
        self.inherit_from().get(path, handler)
    }

    fn post<F, Fut, R>(&mut self, path: &str, handler: F) -> PendingRouteRegistrar<'_>
    where
        F: Fn(Request) -> Fut + Send + Sync + 'static,
        Fut: Future<Output = R> + Send + 'static,
        R: Responsable,
    {
        self.inherit_from().post(path, handler)
    }

    fn put<F, Fut, R>(&mut self, path: &str, handler: F) -> PendingRouteRegistrar<'_>
    where
        F: Fn(Request) -> Fut + Send + Sync + 'static,
        Fut: Future<Output = R> + Send + 'static,
        R: Responsable,
    {
        self.inherit_from().put(path, handler)
    }

    fn patch<F, Fut, R>(&mut self, path: &str, handler: F) -> PendingRouteRegistrar<'_>
    where
        F: Fn(Request) -> Fut + Send + Sync + 'static,
        Fut: Future<Output = R> + Send + 'static,
        R: Responsable,
    {
        self.inherit_from().patch(path, handler)
    }

    fn delete<F, Fut, R>(&mut self, path: &str, handler: F) -> PendingRouteRegistrar<'_>
    where
        F: Fn(Request) -> Fut + Send + Sync + 'static,
        Fut: Future<Output = R> + Send + 'static,
        R: Responsable,
    {
        self.inherit_from().delete(path, handler)
    }

    fn any<F, Fut, R>(&mut self, path: &str, handler: F) -> PendingRouteRegistrar<'_>
    where
        F: Fn(Request) -> Fut + Send + Sync + 'static,
        Fut: Future<Output = R> + Send + 'static,
        R: Responsable,
    {
        self.inherit_from().any(path, handler)
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
        self.inherit_from().r#match(methods, path, handler)
    }

    fn domain(&mut self, domain: impl Into<String>) -> PendingRouteRegistrar<'_> {
        self.inherit_from().domain(domain)
    }

    fn prefix(&mut self, prefix: impl Into<String>) -> PendingRouteRegistrar<'_> {
        self.inherit_from().prefix(prefix)
    }

    fn name(&mut self, name: impl Into<String>) -> PendingRouteRegistrar<'_> {
        self.inherit_from().name(name)
    }

    fn middleware(&mut self, middleware: impl IntoMiddleware) -> PendingRouteRegistrar<'_> {
        self.inherit_from().middleware(middleware)
    }

    fn without_middleware(&mut self, middleware: impl IntoMiddleware) -> PendingRouteRegistrar<'_> {
        self.inherit_from().without_middleware(middleware)
    }
}

impl<'a> Drop for PendingRouteRegistrar<'a> {
    fn drop(&mut self) {
        self.commit();
    }
}
