use crate::container::Container;
use crate::foundation::Application;
use crate::routing::Router;
use async_trait::async_trait;

/// Equivalent to Illuminate\Support\ServiceProvider
#[async_trait]
pub trait ServiceProvider: Send + Sync {
    /// Register any application services into the container.
    /// This is called first, before any other providers are booted.
    ///
    /// Note: You receive a mutable reference to the Container here so you can
    /// insert bindings. You should NOT access other services here, as they
    /// may not be registered yet.
    async fn register(&self, container: &mut Container);

    /// Bootstrap any application services.
    /// This is called after ALL providers have been registered.
    ///
    /// Note: You receive an immutable reference to the Container, and a mutable
    /// reference to the Router to register routes. You can safely resolve
    /// dependencies from the container here.
    async fn boot(&self, _container: &Container, _router: &mut Router) {
        // Default implementation does nothing.
        // Users can override this if they need to boot services.
    }
}
