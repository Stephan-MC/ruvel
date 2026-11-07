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
    router: Router,
    middleware_callback: Option<Box<dyn FnOnce(&mut Middleware)>>,
    routing_callback: Option<Box<dyn FnOnce(&mut Router)>>,
}

impl ApplicationBuilder {
    pub fn new(app: Application) -> Self {
        Self {
            app,
            router: Router::new(),
            middleware_callback: None,
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

    /// Register routes on the router
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

    /// Configure middleware callback for the HTTP Kernel.
    pub fn with_middleware<F>(mut self, callback: F) -> Self
    where
        F: FnOnce(&mut Middleware) + 'static,
    {
        if let Some(prev) = self.middleware_callback.take() {
            self.middleware_callback = Some(Box::new(move |m| {
                prev(m);
                callback(m);
            }));
        } else {
            self.middleware_callback = Some(Box::new(callback));
        }

        self
    }

    pub fn with_scheduler<F>(self, _callback: F) -> Self
    where
        F: FnOnce(&mut Scheduler),
    {
        todo!("Not yet implemented");
    }

    pub fn create(mut self) -> Application {
        // Construct the HTTP Kernel with &app and Arc<Router>
        let mut router = self.router;

        // If there's a middleware callback, resolve it through Kernel
        let mut kernel = HttpKernel::new(&self.app, Arc::new(Router::new()));
        if let Some(callback) = self.middleware_callback.take() {
            kernel.configure_middleware(callback);
            let groups = kernel.middleware_groups.read().unwrap().clone();
            let aliases = kernel.middleware_aliases.read().unwrap().clone();
            router.install_middleware(groups, aliases);
        }

        // Execute routing callbacks
        if let Some(callback) = self.routing_callback.take() {
            callback(&mut router);
        }

        // Attach final router to kernel
        kernel.router = Arc::new(router);
        kernel.sync_router_handler();

        // Register HttpKernel in the container
        self.app.container.instance(kernel);

        self.app
    }
}
