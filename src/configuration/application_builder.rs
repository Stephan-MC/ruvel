pub(crate) use crate::foundation::Application;
use crate::{configuration::Middleware, routing::Router, support::ServiceProvider};

#[derive(Default)]
pub struct Scheduler {}

#[derive(Default)]
pub struct ApplicationBuilder {
    app: Application,
}

impl ApplicationBuilder {
    pub fn new() -> Self {
        Self {
            app: Application::new(),
        }
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

    pub fn with_middleware<F>(self, callback: F) -> Self
    where
        F: FnOnce(&Middleware),
    {
        let mut middleware = Middleware::new();

        callback(&middleware);

        // TODO: Sync Middleware Configs with HttpKernel
        // 1. Set global Middleware
        // 2. Set global Middelwares group
        // 3. Set Middleware Aliases

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
