use crate::container::Container;
use crate::support::ServiceProvider;
use async_trait::async_trait;

pub struct EventServiceProvider;

#[async_trait]
impl ServiceProvider for EventServiceProvider {
    async fn register(&self, _container: &mut Container) {
        // Event service provider registration (reserved for event dispatcher)
    }
}

pub struct RoutingServiceProvider;

#[async_trait]
impl ServiceProvider for RoutingServiceProvider {
    async fn register(&self, _container: &mut Container) {
        // Routing service provider registration
    }
}

pub struct LogServiceProvider;

#[async_trait]
impl ServiceProvider for LogServiceProvider {
    async fn register(&self, _container: &mut Container) {
        // Log service provider registration
    }
}
