use crate::routing::{PendingRouteRegistrar, Router};

pub(crate) type RouterCallback = Box<dyn FnOnce(&mut Router) + 'static>;
pub(crate) type PendingRouteCallback = Box<dyn FnOnce(&mut PendingRouteRegistrar) + 'static>;

/// Laravel-style routing configuration builder.
///
/// Passed as a mutable reference to the `with_routing` closure on [`ApplicationBuilder`](crate::configuration::ApplicationBuilder).
#[derive(Default)]
pub struct Routing {
    using: Vec<RouterCallback>,
    web: Vec<RouterCallback>,
    api: Vec<PendingRouteCallback>,
    commands: Vec<RouterCallback>,
    channels: Vec<RouterCallback>,
    health: Option<String>,
    api_prefix: String,
    then: Vec<RouterCallback>,
}

impl Routing {
    /// Create a new, empty `Routing` configuration with default `api_prefix = "api"`.
    pub fn new() -> Self {
        Self {
            using: Vec::new(),
            web: Vec::new(),
            api: Vec::new(),
            commands: Vec::new(),
            channels: Vec::new(),
            health: None,
            api_prefix: "api".to_string(),
            then: Vec::new(),
        }
    }

    /// Override the default route loading entirely.
    ///
    /// When specified, `web`, `api`, `health`, `commands`, and `then` are skipped,
    /// giving full manual control to the provided callbacks.
    pub fn using<F>(&mut self, callback: F) -> &mut Self
    where
        F: FnOnce(&mut Router) + 'static,
    {
        self.using.push(Box::new(callback));
        self
    }

    /// Register web routes on the root router.
    pub fn web<F>(&mut self, callback: F) -> &mut Self
    where
        F: FnOnce(&mut Router) + 'static,
    {
        self.web.push(Box::new(callback));
        self
    }

    /// Register API routes scoped under the configured `api_prefix` (default `"api"`).
    ///
    /// If an `"api"` middleware group exists on the router, it is automatically attached.
    pub fn api<F>(&mut self, callback: F) -> &mut Self
    where
        F: FnOnce(&mut PendingRouteRegistrar) + 'static,
    {
        self.api.push(Box::new(callback));
        self
    }

    /// Register console command routes.
    pub fn commands<F>(&mut self, callback: F) -> &mut Self
    where
        F: FnOnce(&mut Router) + 'static,
    {
        self.commands.push(Box::new(callback));
        self
    }

    /// Register broadcast channel routes.
    pub fn channels<F>(&mut self, callback: F) -> &mut Self
    where
        F: FnOnce(&mut Router) + 'static,
    {
        self.channels.push(Box::new(callback));
        self
    }

    /// Register an application health check GET route returning `"OK"` (HTTP 200).
    ///
    /// If `path` is empty, no health check route is registered.
    pub fn health(&mut self, path: impl Into<String>) -> &mut Self {
        self.health = Some(path.into());
        self
    }

    /// Set the path prefix used for API routes (default: `"api"`).
    pub fn api_prefix(&mut self, prefix: impl Into<String>) -> &mut Self {
        self.api_prefix = prefix.into();
        self
    }

    /// Register a callback to execute after standard `web` and `api` routes have been registered.
    pub fn then<F>(&mut self, callback: F) -> &mut Self
    where
        F: FnOnce(&mut Router) + 'static,
    {
        self.then.push(Box::new(callback));
        self
    }

    /// Commit this routing configuration to the given [`Router`].
    pub(crate) fn register(mut self, router: &mut Router) {
        if !self.using.is_empty() {
            for using in self.using.drain(..) {
                using(router);
            }
            return;
        }

        if let Some(health_path) = self.health {
            let path = health_path.trim();
            if !path.is_empty() {
                let formatted = if path.starts_with('/') {
                    path.to_string()
                } else {
                    format!("/{}", path)
                };
                router.get(&formatted, |_req| async { "OK" });
            }
        }

        for web in self.web.drain(..) {
            web(router);
        }

        if !self.api.is_empty() {
            let prefix = self.api_prefix.trim_matches('/').to_string();
            let mut pending = router.prefix(prefix);
            if pending.has_middleware_group("api") {
                pending = pending.middleware("api");
            }
            let api_callbacks = std::mem::take(&mut self.api);
            pending.group(|r| {
                for api in api_callbacks {
                    api(r);
                }
            });
        }

        for commands in self.commands.drain(..) {
            commands(router);
        }

        for channels in self.channels.drain(..) {
            channels(router);
        }

        for then in self.then.drain(..) {
            then(router);
        }
    }
}
