pub(crate) use crate::foundation::Application;
use crate::{
    configuration::Middleware,
    console::Kernel as ConsoleKernel,
    database::DatabaseManager,
    http::Kernel as HttpKernel,
    routing::Router,
    support::ServiceProvider,
};
use std::sync::Arc;

#[derive(Default)]
pub struct Scheduler {}

pub struct ApplicationBuilder {
    app: Application,
    routing_callbacks: Vec<Box<dyn FnOnce(&mut Router)>>,
    database_callbacks: Vec<Box<dyn FnOnce(&mut DatabaseManager) + Send + 'static>>,
}

impl ApplicationBuilder {
    pub fn new(app: Application) -> Self {
        Self {
            app,
            routing_callbacks: Vec::new(),
            database_callbacks: Vec::new(),
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
        self.routing_callbacks.push(Box::new(callback));
        self
    }

    /// Configure database connections on the DatabaseManager
    pub fn with_database<F>(mut self, callback: F) -> Self
    where
        F: FnOnce(&mut DatabaseManager) + Send + 'static,
    {
        self.database_callbacks.push(Box::new(callback));
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
        self.app.container.singleton(|c| {
            let router = c.make::<Router>().unwrap_or_else(|_| Arc::new(Router::new()));
            HttpKernel::new(router)
        });
        self
    }

    /// Configure global middleware, groups, and aliases via HttpKernel after resolution.
    pub fn with_middleware<F>(self, callback: F) -> Self
    where
        F: FnOnce(&mut Middleware) + Send + Sync + 'static,
    {
        let callback_cell = std::sync::Mutex::new(Some(callback));
        self.app.container.after_resolving::<HttpKernel, _>(move |kernel, _| {
            if let Some(cb) = callback_cell.lock().unwrap().take() {
                kernel.configure_middleware(cb);
            }
        });
        self
    }

    pub fn with_scheduler<F>(self, _callback: F) -> Self
    where
        F: FnOnce(&mut Scheduler),
    {
        todo!("Not yet implemented");
    }

    pub fn create(mut self) -> Application {
        // 0. Initialize and configure DatabaseManager
        let mut db_manager = DatabaseManager::new();
        for callback in self.database_callbacks.drain(..) {
            callback(&mut db_manager);
        }
        let db_manager_arc = Arc::new(db_manager);
        DatabaseManager::register_global(Arc::clone(&db_manager_arc));
        self.app.container.instance((*db_manager_arc).clone());

        // 1. Instantiate the Router
        let mut router = Router::new();

        // 2. Resolve the HttpKernel from the container (which runs after_resolving to configure middleware)
        let kernel = self.app.make::<HttpKernel>().expect("HttpKernel must be registered");

        // 3. Inherit groups and aliases configured on the kernel
        let groups = kernel.middleware_groups.read().unwrap().clone();
        let aliases = kernel.middleware_aliases.read().unwrap().clone();
        router.install_middleware(groups, aliases);

        // 4. Execute all routing callbacks on the router
        for callback in self.routing_callbacks.drain(..) {
            callback(&mut router);
        }

        // 5. Update the kernel's router so dispatch routes match!
        let router_arc = Arc::new(router);
        kernel.set_router(Arc::clone(&router_arc));

        // 6. Bind the router as an instance in the container
        self.app.container.instance((*router_arc).clone());

        self.app
    }
}
