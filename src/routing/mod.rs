mod into_middleware;
pub mod pending_resource_registrar;
mod pending_route_registrar;
pub mod pipeline;
mod redirector;
pub mod route;
pub mod route_registrar;
mod router;

pub use into_middleware::{IntoMiddleware, MiddlewareItem};
pub use pending_resource_registrar::{
    IntoResourceAction, PendingResourceRegistration, ResourceAction,
};
pub use pending_route_registrar::PendingRouteRegistrar;
pub use pipeline::Pipeline;
pub use redirector::Redirector;
pub use route::Route;
pub use route_registrar::RouteRegistrar;
pub use router::{HandlerFn, Router};
pub mod routing_service_provider;
pub use routing_service_provider::RoutingServiceProvider;
