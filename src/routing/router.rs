use crate::contracts::Responsable;
use crate::http::{Method, Request, Response};
use matchit::Router as MatchitRouter; // Alias to avoid naming conflicts
use std::collections::HashMap;
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;

pub type BoxFuture = Pin<Box<dyn Future<Output = Response> + Send + 'static>>;
pub type HandlerFn = Arc<dyn Fn(Request) -> BoxFuture + Send + Sync + 'static>;

#[derive(Default)]
pub struct Router {
    // Map each HTTP Method to its own Radix Tree of routes
    routes: HashMap<Method, MatchitRouter<HandlerFn>>,
}

impl Router {
    pub fn new() -> Self {
        Self {
            routes: HashMap::new(),
        }
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

        // Get the radix tree for this HTTP Method, or create one if it doesn't exist
        let method_router = self.routes.entry(method).or_insert_with(MatchitRouter::new);

        // Insert the route. Using unwrap() because a syntax error in route
        // definitions (e.g., overlapping params) should panic at startup.
        method_router.insert(path, h).unwrap();
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
        // 1. Find the Radix Tree for the incoming HTTP method
        if let Some(method_router) = self.routes.get(&req.method) {
            // 2. Traverse the tree matching the URL path
            if let Ok(matched) = method_router.at(&req.path) {
                // 3. Extract dynamic parameters (e.g., {id}) and inject them into our Request
                for (key, value) in matched.params.iter() {
                    req.params.insert(key.to_string(), value.to_string());
                }

                // 4. Trigger the matched handler
                let handler = matched.value;
                return handler(req);
            }
        }

        Box::pin(async { Response::not_found().text("404 Not Found") })
    }
}
