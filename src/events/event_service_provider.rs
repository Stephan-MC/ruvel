use crate::container::Container;
use crate::support::ServiceProvider;
use async_trait::async_trait;

pub struct EventServiceProvider;

#[async_trait]
impl ServiceProvider for EventServiceProvider {
    async fn register(&self, _container: &mut Container) {
        // Event service provider registration
    }
}
