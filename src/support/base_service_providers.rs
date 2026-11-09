use crate::container::Container;
use crate::support::ServiceProvider;
use async_trait::async_trait;

pub struct LogServiceProvider;

#[async_trait]
impl ServiceProvider for LogServiceProvider {
    async fn register(&self, _container: &mut Container) {
        // Log service provider registration
    }
}
