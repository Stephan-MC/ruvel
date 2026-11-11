use super::into_middleware::IntoMiddleware;
use super::pending_route::PendingRouteRegistrar;
use crate::contracts::support::Responsable;
use crate::http::{Method, Request};
use std::future::Future;

/// Trait defining the fluent route registration and attribute configuration surface
/// for [`Router`](super::Router).
pub trait RouteRegistrar {
    /// Register a new GET route with the router.
    fn get<F, Fut, R>(&mut self, path: &str, handler: F) -> PendingRouteRegistrar<'_>
    where
        F: Fn(Request) -> Fut + Send + Sync + 'static,
        Fut: Future<Output = R> + Send + 'static,
        R: Responsable;

    /// Register a new POST route with the router.
    fn post<F, Fut, R>(&mut self, path: &str, handler: F) -> PendingRouteRegistrar<'_>
    where
        F: Fn(Request) -> Fut + Send + Sync + 'static,
        Fut: Future<Output = R> + Send + 'static,
        R: Responsable;

    /// Register a new PUT route with the router.
    fn put<F, Fut, R>(&mut self, path: &str, handler: F) -> PendingRouteRegistrar<'_>
    where
        F: Fn(Request) -> Fut + Send + Sync + 'static,
        Fut: Future<Output = R> + Send + 'static,
        R: Responsable;

    /// Register a new PATCH route with the router.
    fn patch<F, Fut, R>(&mut self, path: &str, handler: F) -> PendingRouteRegistrar<'_>
    where
        F: Fn(Request) -> Fut + Send + Sync + 'static,
        Fut: Future<Output = R> + Send + 'static,
        R: Responsable;

    /// Register a new DELETE route with the router.
    fn delete<F, Fut, R>(&mut self, path: &str, handler: F) -> PendingRouteRegistrar<'_>
    where
        F: Fn(Request) -> Fut + Send + Sync + 'static,
        Fut: Future<Output = R> + Send + 'static,
        R: Responsable;

    /// Register a new route that responds to all standard HTTP methods.
    fn any<F, Fut, R>(&mut self, path: &str, handler: F) -> PendingRouteRegistrar<'_>
    where
        F: Fn(Request) -> Fut + Send + Sync + 'static,
        Fut: Future<Output = R> + Send + 'static,
        R: Responsable;

    /// Register a new route that responds to the given HTTP methods.
    fn r#match<F, Fut, R>(
        &mut self,
        methods: &[Method],
        path: &str,
        handler: F,
    ) -> PendingRouteRegistrar<'_>
    where
        F: Fn(Request) -> Fut + Send + Sync + 'static,
        Fut: Future<Output = R> + Send + 'static,
        R: Responsable;

    /// Begin configuring a route or group with a domain constraint.
    fn domain(&mut self, domain: impl Into<String>) -> PendingRouteRegistrar<'_>;

    /// Begin configuring a route or group with a path prefix.
    fn prefix(&mut self, prefix: impl Into<String>) -> PendingRouteRegistrar<'_>;

    /// Begin configuring a route or group with a route name or name prefix.
    fn name(&mut self, name: impl Into<String>) -> PendingRouteRegistrar<'_>;

    /// Begin configuring a route or group with middleware.
    fn middleware(&mut self, middleware: impl IntoMiddleware) -> PendingRouteRegistrar<'_>;

    /// Begin configuring a route or group with excluded middleware aliases/groups.
    fn without_middleware(&mut self, middleware: impl IntoMiddleware) -> PendingRouteRegistrar<'_>;
}
