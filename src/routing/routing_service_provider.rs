use crate::container::Container;
use crate::support::ServiceProvider;
use async_trait::async_trait;

pub struct RoutingServiceProvider;

#[async_trait]
impl ServiceProvider for RoutingServiceProvider {
    async fn register(&self, _container: &mut Container) {
        // Routing service provider registration
    }
}
