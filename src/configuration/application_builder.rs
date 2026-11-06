pub(crate) use crate::foundation::Application;
use crate::{
    configuration::Middleware,
    console::Kernel as ConsoleKernel,
    http::Kernel as HttpKernel,
    routing::Router,
    support::ServiceProvider,
};
use std::sync::Arc;

#[derive(Default)]
pub struct Scheduler {}

pub struct ApplicationBuilder {
    app: Application,
    middleware: Option<Middleware>,
    routing_callback: Option<Box<dyn FnOnce(&mut Router)>>,
}

impl ApplicationBuilder {
    pub fn new(app: Application) -> Self {
        Self {
            app,
            middleware: None,
            routing_callback: None,
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
    pub fn with_routing<F>(mut self, callback: F) -> Self
    where
        F: FnOnce(&mut Router) + 'static,
    {
        if let Some(prev) = self.routing_callback.take() {
            self.routing_callback = Some(Box::new(move |router| {
                prev(router);
                callback(router);
            }));
        } else {
            self.routing_callback = Some(Box::new(callback));
        }

        self
    }

    pub fn with_events(self) -> Self {
        todo!("Not implemented");
    }

    pub fn with_commands(self) -> Self {
        todo!("Not implemented");
    }

    /// Register HTTP and Console kernels in the container and application.
    pub fn with_kernels(self) -> Self {
        self.app.container.singleton(|_| ConsoleKernel::default());
        self.app.container.singleton(|_| HttpKernel::default());
        self
    }

    /// Configure global middleware, groups, and aliases.
    pub fn with_middleware<F>(mut self, callback: F) -> Self
    where
        F: FnOnce(&mut Middleware),
    {
        let mut middleware = self.middleware.unwrap_or_default();
        callback(&mut middleware);
        self.middleware = Some(middleware);

        self
    }

    pub fn with_scheduler<F>(self, _callback: F) -> Self
    where
        F: FnOnce(&mut Scheduler),
    {
        todo!("Not yet implemented");
    }

    pub fn create(mut self) -> Application {
        // Automatically register kernels in the container
        self.app.container.singleton(|_| ConsoleKernel::default());

        let resolved = self.middleware.unwrap_or_default().resolve();

        // 1. Install route groups and aliases on the router first
        self.app
            .router
            .install_middleware(resolved.groups, resolved.aliases);

        // 2. Execute routing callbacks so routes can resolve aliases/groups
        if let Some(callback) = self.routing_callback.take() {
            callback(&mut self.app.router);
        }

        // 3. Build the HTTP Kernel with global middleware and Router
        let router_arc = Arc::new(std::mem::take(&mut self.app.router));
        let http_kernel = HttpKernel::new(Arc::clone(&router_arc), resolved.global);

        // 4. Register HTTP Kernel in the container (Option B: instance takes T and wraps in Arc)
        self.app.container.instance(http_kernel);

        // 5. Restore router reference to application for backward compatibility
        self.app.router = (*router_arc).clone();

        self.app
    }
}
