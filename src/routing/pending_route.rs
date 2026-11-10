use super::into_middleware::{IntoMiddleware, MiddlewareItem};
use super::router::{HandlerFn, RouteEntry, Router};
use crate::http::Method;
use std::collections::HashMap;
use std::sync::Arc;

/// A builder for configuring a pending route before it is committed to the [`Router`].
///
/// Returned by `router.get(...)`, `router.post(...)`, `router.any(...)`, etc.
/// When dropped, it inserts the configured route into the underlying `Router`.
pub struct PendingRouteRegistrar<'a> {
    router: &'a mut Router,
    methods: Vec<Method>,
    path: String,
    prefix: Option<String>,
    name: Option<String>,
    domain: Option<String>,
    wheres: HashMap<String, String>,
    handler: HandlerFn,
    middleware: Vec<MiddlewareItem>,
    excluded_middleware: Vec<String>,
    committed: bool,
}

impl<'a> PendingRouteRegistrar<'a> {
    pub(crate) fn new(
        router: &'a mut Router,
        methods: Vec<Method>,
        path: &str,
        handler: HandlerFn,
    ) -> Self {
        Self {
            router,
            methods,
            path: path.to_string(),
            prefix: None,
            name: None,
            domain: None,
            wheres: HashMap::new(),
            handler,
            middleware: Vec::new(),
            excluded_middleware: Vec::new(),
            committed: false,
        }
    }

    /// Attach middleware to this route.
    ///
    /// Accepts alias names (`&str`, `String`), slices/arrays/Vecs of aliases,
    /// or `Arc<dyn Middleware>` instances.
    pub fn middleware(mut self, middleware: impl IntoMiddleware) -> Self {
        self.middleware.extend(middleware.into_middleware_items());
        self
    }

    /// Exclude specified middleware alias/group names from this route's pipeline.
    pub fn without_middleware(mut self, middleware: impl IntoMiddleware) -> Self {
        for item in middleware.into_middleware_items() {
            if let MiddlewareItem::Alias(name) = item {
                self.excluded_middleware.push(name);
            }
        }
        self
    }

    /// Set the name for this route.
    pub fn name(mut self, name: impl Into<String>) -> Self {
        self.name = Some(name.into());
        self
    }

    /// Set an additional path prefix for this route.
    pub fn prefix(mut self, prefix: impl Into<String>) -> Self {
        self.prefix = Some(prefix.into());
        self
    }

    /// Set a domain restriction or metadata for this route.
    pub fn domain(mut self, domain: impl Into<String>) -> Self {
        self.domain = Some(domain.into());
        self
    }

    /// Add a regular expression / pattern constraint on a route parameter.
    pub fn r#where(mut self, param: &str, pattern: &str) -> Self {
        self.wheres.insert(param.to_string(), pattern.to_string());
        self
    }

    /// Add a numeric constraint on a route parameter (e.g. [0-9]+).
    pub fn where_number(self, param: &str) -> Self {
        self.r#where(param, r"^[0-9]+$")
    }

    /// Add an alphabetic constraint on a route parameter (e.g. [a-zA-Z]+).
    pub fn where_alpha(self, param: &str) -> Self {
        self.r#where(param, r"^[a-zA-Z]+$")
    }

    /// Commit the route into the router immediately.
    pub fn commit(&mut self) {
        if self.committed {
            return;
        }
        self.committed = true;

        // Build final path: prepend any route-specific prefix
        let mut final_path = self.path.clone();
        if let Some(ref p) = self.prefix {
            let trimmed_prefix = p.trim_matches('/');
            let trimmed_path = final_path.trim_start_matches('/');
            if trimmed_prefix.is_empty() {
                final_path = format!("/{}", trimmed_path);
            } else if trimmed_path.is_empty() {
                final_path = format!("/{}", trimmed_prefix);
            } else {
                final_path = format!("/{}/{}", trimmed_prefix, trimmed_path);
            }
        }

        // Apply router prefix if any
        final_path = self.router.combine_prefix(&final_path);

        // Build the route's middleware pipeline (global + group + route-specific, minus excluded)
        let middleware = self
            .router
            .compile_route_middlewares(&self.middleware, &self.excluded_middleware);

        let final_domain = self
            .domain
            .clone()
            .or_else(|| self.router.group_domain.clone());

        let entry = RouteEntry {
            handler: Arc::clone(&self.handler),
            middleware,
            name: self.name.clone(),
            domain: final_domain,
            wheres: self.wheres.clone(),
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

/// A builder for configuring a group of routes with shared attributes (prefix, domain).
pub struct RouteGroupRegistrar<'a> {
    router: &'a mut Router,
    prefix: Option<String>,
    domain: Option<String>,
}

impl<'a> RouteGroupRegistrar<'a> {
    pub(crate) fn new(router: &'a mut Router) -> Self {
        Self {
            router,
            prefix: None,
            domain: None,
        }
    }

    /// Add a path prefix for all routes in this group.
    pub fn prefix(mut self, prefix: impl Into<String>) -> Self {
        self.prefix = Some(prefix.into());
        self
    }

    /// Set domain for all routes in this group.
    pub fn domain(mut self, domain: impl Into<String>) -> Self {
        self.domain = Some(domain.into());
        self
    }

    /// Execute the callback registering all routes within this group.
    pub fn group<F>(self, callback: F)
    where
        F: FnOnce(&mut Router),
    {
        // Save current router group properties
        let prev_prefix = self.router.group_prefix.clone();
        let prev_domain = self.router.group_domain.clone();

        // Push new group properties onto the router
        if let Some(p) = self.prefix {
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

        if self.domain.is_some() {
            self.router.group_domain = self.domain;
        }

        callback(self.router);

        // Restore previous router group properties
        self.router.group_prefix = prev_prefix;
        self.router.group_domain = prev_domain;
    }
}
