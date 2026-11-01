pub(crate) use crate::foundation::Application;
use crate::{configuration::Middleware, routing::Router, support::ServiceProvider};

#[derive(Default)]
pub struct Scheduler {}

#[derive(Default)]
pub struct ApplicationBuilder {
    app: Application,
}

impl ApplicationBuilder {
    pub fn new(app: Application) -> Self {
        Self { app }
    }

    pub async fn with_provider<P: ServiceProvider + 'static>(mut self, provider: P) -> Self {
        self.app = self.app.register_provider(provider).await;

        self
    }

    pub fn with_providers(mut self, mut providers: Vec<Box<dyn ServiceProvider>>) -> Self {
        self.app.providers.append(&mut providers);
        self
    }

    /// Register a route with a specified middleware group
    ///
    ///
    pub fn with_routing<F>(mut self, callback: F) -> Self
    where
        F: FnOnce(&mut Router),
    {
        callback(&mut self.app.router);

        self
    }

    pub fn with_events(self) -> Self {
        todo!("Not implemented");
    }

    pub fn with_commands(self) -> Self {
        todo!("Not implemented");
    }

    pub fn with_kernels(self) -> Self {
        todo!("Not implemented");
    }

    /// Configure global middleware, groups, and aliases, then install them on the router.
    pub fn with_middleware<F>(mut self, callback: F) -> Self
    where
        F: FnOnce(&mut Middleware),
    {
        let mut middleware = Middleware::new();
        callback(&mut middleware);

        let resolved = middleware.resolve();
        self.app.router.install_middleware(
            resolved.global,
            resolved.groups,
            resolved.aliases,
        );

        self
    }

    pub fn with_scheduler<F>(self, _callback: F) -> Self
    where
        F: FnOnce(&mut Scheduler),
    {
        todo!("Not yet implemented");
    }

    pub fn create(self) -> Application {
        self.app
    }
}
