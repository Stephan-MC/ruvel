use crate::container::Container;
use crate::support::ServiceProvider;

pub struct RoutingServiceProvider;

impl ServiceProvider for RoutingServiceProvider {
    fn register(&self, _container: &mut Container) {
        // Routing service provider registration
    }
}
