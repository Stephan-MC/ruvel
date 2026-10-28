use crate::contracts::Responsable;
use crate::http::{Method, Request, Response};
use std::collections::HashMap;
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;

pub type BoxFuture = Pin<Box<dyn Future<Output = Response> + Send + 'static>>;
pub type HandlerFn = Arc<dyn Fn(Request) -> BoxFuture + Send + Sync + 'static>;

#[derive(Default)]
pub struct Router {
    routes: HashMap<(Method, String), HandlerFn>,
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
        self.routes.insert((method, path.to_string()), h);
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

    pub fn dispatch(&self, req: Request) -> BoxFuture {
        if let Some(handler) = self.routes.get(&(req.method.clone(), req.path.clone())) {
            handler(req)
        } else {
            Box::pin(async { Response::not_found().text("404 Not Found") })
        }
    }
}
