use crate::container::Container;
use crate::support::ServiceProvider;

pub struct EventServiceProvider;

impl ServiceProvider for EventServiceProvider {
    fn register(&self, _container: &mut Container) {
        // Event service provider registration
    }
}
