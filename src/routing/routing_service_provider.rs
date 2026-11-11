use crate::container::Container;
use crate::routing::{Redirector, Router};
use crate::support::ServiceProvider;

pub struct RoutingServiceProvider;

impl RoutingServiceProvider {
    pub fn register_router(&self, container: &Container) {
        container.singleton(|| Router::new());
    }

    pub fn register_redirector(&self, container: &Container) {
        container.singleton(|| Redirector {});
    }
}

impl ServiceProvider for RoutingServiceProvider {
    fn register(&self, _container: &mut Container) {
        self.register_router(_container);
        // TODO:
        // self.register_url_generator()
        self.register_redirector();
    }
}
