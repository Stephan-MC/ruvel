use crate::container::{self, Container};
use crate::routing::{Redirector, Router};
use crate::support::ServiceProvider;

pub struct RoutingServiceProvider;

impl RoutingServiceProvider {
    pub fn register_router(&self, container: &Container) {
        container.singleton(|_| Router::new());
    }

    pub fn register_redirector(&self, container: &Container) {
        container.singleton(|_| Redirector {});
    }
}

impl ServiceProvider for RoutingServiceProvider {
    fn register(&self, container: &mut Container) {
        self.register_router(container);
        // TODO:
        // self.register_url_generator()
        self.register_redirector(container);
    }
}
