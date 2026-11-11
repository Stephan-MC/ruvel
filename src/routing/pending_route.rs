use super::into_middleware::{IntoMiddleware, MiddlewareItem};
use super::router::{HandlerFn, RouteEntry, Router};
use crate::contracts::support::Responsable;
use crate::http::{Method, Request};
use std::collections::HashMap;
use std::future::Future;
use std::sync::Arc;

/// A fluent builder for configuring a pending route or route group before it is
/// committed to the [`Router`].
///
/// Returned by [`RouteRegistrar`](super::RouteRegistrar) methods on [`Router`].
/// When configuring a single route (where a handler is attached via `.get(...)`, `.post(...)`, etc.),
/// dropping `PendingRouteRegistrar` automatically commits the route into the underlying `Router`.
/// When configuring a route group, call `.group(|router| { ... })`.
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
    pub fn group<F>(mut self, callback: F)
    where
        F: FnOnce(&mut Router),
    {
        self.committed = true;

        // Save current router group properties
        let prev_prefix = self.router.group_prefix.clone();
        let prev_domain = self.router.group_domain.clone();
        let prev_name = self.router.group_name.clone();
        let prev_wheres = self.router.group_wheres.clone();
        let prev_middleware = self.router.group_middleware.clone();
        let prev_excluded = self.router.group_excluded_middleware.clone();

        // Push new group prefix onto the router
        if let Some(ref p) = self.prefix {
            self.router.group_prefix = Some(match &self.router.group_prefix {
                Some(parent) => {
                    let parent_clean = parent.trim_matches('/');
                    let p_clean = p.trim_matches('/');
                    if parent_clean.is_empty() {
                        format!("/{}", p_clean)
                    } else if p_clean.is_empty() {
                        format!("/{}", parent_clean)
                    } else {
                        format!("/{}/{}", parent_clean, p_clean)
                    }
                }
                None => {
                    let p_clean = p.trim_matches('/');
                    format!("/{}", p_clean)
                }
            });
        }

        if let Some(ref domain) = self.domain {
            self.router.group_domain = Some(domain.clone());
        }

        if let Some(ref name) = self.name {
            self.router.group_name = Some(match &self.router.group_name {
                Some(parent_name) => format!("{}{}", parent_name, name),
                None => name.clone(),
            });
        }

        if !self.wheres.is_empty() {
            self.router
                .group_wheres
                .extend(self.wheres.iter().map(|(k, v)| (k.clone(), v.clone())));
        }

        if !self.middleware.is_empty() {
            self.router
                .group_middleware
                .extend(self.middleware.iter().cloned());
        }

        if !self.excluded_middleware.is_empty() {
            self.router
                .group_excluded_middleware
                .extend(self.excluded_middleware.iter().cloned());
        }

        callback(self.router);

        // Restore previous router group properties
        self.router.group_prefix = prev_prefix;
        self.router.group_domain = prev_domain;
        self.router.group_name = prev_name;
        self.router.group_wheres = prev_wheres;
        self.router.group_middleware = prev_middleware;
        self.router.group_excluded_middleware = prev_excluded;
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

        // Build final path: prepend any route-specific prefix
        let mut final_path = raw_path.to_string();
        if let Some(ref p) = self.prefix {
            let trimmed_prefix = p.trim_matches('/');
            let trimmed_path = final_path.trim_matches('/');
            if trimmed_prefix.is_empty() {
                final_path = if trimmed_path.is_empty() {
                    "/".to_string()
                } else {
                    format!("/{}", trimmed_path)
                };
            } else if trimmed_path.is_empty() {
                final_path = format!("/{}", trimmed_prefix);
            } else {
                final_path = format!("/{}/{}", trimmed_prefix, trimmed_path);
            }
        }

        // Apply router group prefix if any
        final_path = self.router.combine_prefix(&final_path);

        // Combine group middleware + route middleware, and group excluded + route excluded
        let mut combined_middleware = self.router.group_middleware.clone();
        combined_middleware.extend(self.middleware.iter().cloned());

        let mut combined_excluded = self.router.group_excluded_middleware.clone();
        combined_excluded.extend(self.excluded_middleware.iter().cloned());

        // Build the route's middleware pipeline (global + group + route-specific, minus excluded)
        let middleware = self
            .router
            .compile_route_middlewares(&combined_middleware, &combined_excluded);

        let final_domain = self
            .domain
            .clone()
            .or_else(|| self.router.group_domain.clone());

        let final_name = match (&self.router.group_name, &self.name) {
            (Some(group_name), Some(route_name)) => Some(format!("{}{}", group_name, route_name)),
            (Some(group_name), None) => Some(group_name.clone()),
            (None, Some(route_name)) => Some(route_name.clone()),
            (None, None) => None,
        };

        let mut final_wheres = self.router.group_wheres.clone();
        final_wheres.extend(self.wheres.iter().map(|(k, v)| (k.clone(), v.clone())));

        let entry = RouteEntry {
            handler: Arc::clone(handler),
            middleware,
            name: final_name,
            domain: final_domain,
            wheres: final_wheres,
        };

        for method in &self.methods {
            self.router
                .insert_route(method.clone(), &final_path, entry.clone());
        }
    }
}

impl<'a> Drop for PendingRouteRegistrar<'a> {
    fn drop(&mut self) {
        self.commit();
    }
}
