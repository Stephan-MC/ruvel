use crate::container::Container;
use crate::support::ServiceProvider;

pub struct LogServiceProvider;

impl ServiceProvider for LogServiceProvider {
    fn register(&self, _container: &mut Container) {
        // Log service provider registration
    }
}
