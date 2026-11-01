use crate::contracts::Responsable;
use crate::http::{BoxFuture, Pipeline};
use crate::http::{Method, Middleware, Request, Response};
use matchit::Router as MatchitRouter; // Alias to avoid naming conflicts
use std::collections::HashMap;
use std::future::Future;
use std::sync::Arc;

pub type HandlerFn = Arc<dyn Fn(Request) -> BoxFuture + Send + Sync + 'static>;

#[derive(Clone)]
struct RouteEntry {
    handler: HandlerFn,
    /// Group and alias names to resolve when the route is dispatched.
    middleware: Vec<String>,
}

#[derive(Default)]
pub struct Router {
    // Map each HTTP Method to its own Radix Tree of routes
    routes: HashMap<Method, MatchitRouter<RouteEntry>>,
    global: Vec<Arc<dyn Middleware>>,
    groups: HashMap<String, Vec<Arc<dyn Middleware>>>,
    aliases: HashMap<String, Arc<dyn Middleware>>,
    /// Names applied to routes registered inside [`Router::group`].
    stack: Vec<String>,
}

impl Router {
    pub fn new() -> Self {
        Self {
            routes: HashMap::new(),
            global: Vec::new(),
            groups: HashMap::new(),
            aliases: HashMap::new(),
            stack: Vec::new(),
        }
    }

    /// Install global middleware, groups, and aliases from application configuration.
    pub(crate) fn install_middleware(
        &mut self,
        global: Vec<Arc<dyn Middleware>>,
        groups: HashMap<String, Vec<Arc<dyn Middleware>>>,
        aliases: HashMap<String, Arc<dyn Middleware>>,
    ) {
        self.global = global;
        self.groups = groups;
        self.aliases = aliases;
    }

    /// Register `configure`'s routes with these group or alias names.
    ///
    /// Names are resolved on dispatch. A group name expands to its layers.
    /// Otherwise the name is an alias. Nested groups append to the outer ones.
    pub fn group(&mut self, middleware: &[&str], configure: impl FnOnce(&mut Self)) {
        let start = self.stack.len();
        self.stack
            .extend(middleware.iter().copied().map(str::to_string));
        configure(self);
        self.stack.truncate(start);
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

        let entry = RouteEntry {
            handler: h,
            middleware: self.stack.clone(),
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

    pub fn dispatch(&self, mut req: Request) -> BoxFuture {
        let (handler, names) = if let Some(method_router) = self.routes.get(&req.method) {
            if let Ok(matched) = method_router.at(&req.path) {
                for (key, value) in matched.params.iter() {
                    req.params.insert(key.to_string(), value.to_string());
                }

                (
                    Arc::clone(&matched.value.handler),
                    matched.value.middleware.clone(),
                )
            } else {
                (self.not_found(), Vec::new())
            }
        } else {
            (self.not_found(), Vec::new())
        };

        // Global layers are outermost. Route groups and aliases run inside them.
        // A missing route still passes through global middleware.
        let mut layers = self.global.clone();
        layers.extend(self.resolve_names(&names));

        Pipeline::new(layers).run(req, handler)
    }

    fn not_found(&self) -> HandlerFn {
        Arc::new(|_req| Box::pin(async { Response::not_found().text("404 Not Found") }))
    }

    fn resolve_names(&self, names: &[String]) -> Vec<Arc<dyn Middleware>> {
        let mut layers = Vec::new();

        for name in names {
            if let Some(group) = self.groups.get(name) {
                layers.extend(group.iter().cloned());
            } else if let Some(alias) = self.aliases.get(name) {
                layers.push(Arc::clone(alias));
            } else {
                panic!("Unknown middleware [{name}]");
            }
        }

        layers
    }
}
