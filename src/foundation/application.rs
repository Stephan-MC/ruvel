use crate::container::Container;
use crate::http::Request;
use crate::routing::Router;
use crate::support::ServiceProvider;

use std::sync::Arc;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;

#[derive(Default)]
pub struct Application {
    pub container: Container,
    pub router: Router,
    // Store registered service providers as dynamic trait objects
    providers: Vec<Arc<dyn ServiceProvider>>,
    booted: bool,
}

impl Application {
    pub fn new() -> Self {
        Self {
            container: Container::new(),
            router: Router::new(),
            providers: Vec::new(),
            booted: false,
        }
    }

    /// Register a service provider with the application.
    /// This immediately calls the provider's `register` method.
    pub async fn register_provider<P: ServiceProvider + 'static>(mut self, provider: P) -> Self {
        // 1. Run the register method immediately upon binding
        provider.register(&mut self.container).await;

        let provider_arc = Arc::new(provider);
        self.providers.push(provider_arc.clone());

        // 2. If the application is already booted, boot this provider immediately
        if self.booted {
            provider_arc.boot(&self.container, &mut self.router).await;
        }

        self
    }

    /// Boot the application's service providers.
    /// This loops through all registered providers and calls their `boot` methods.
    pub async fn boot(&mut self) {
        if self.booted {
            return;
        }

        for provider in &self.providers {
            provider.boot(&self.container, &mut self.router).await;
        }

        self.booted = true;
    }

    /// Start the TCP Listener (analogous to public/index.php `$kernel->handle()`)
    pub async fn listen(mut self, port: u16) -> Result<(), Box<dyn std::error::Error>> {
        // 1. Ensure all providers are booted before accepting traffic
        self.boot().await;

        let addr = format!("0.0.0.0:{}", port);
        let listener = TcpListener::bind(&addr).await?;
        println!("🚀 Ruvel native server running on http://{}", addr);

        let router = Arc::new(self.router);

        loop {
            let (mut socket, _) = listener.accept().await?;
            let router_clone = Arc::clone(&router);

            tokio::spawn(async move {
                let mut buffer = [0u8; 1024];

                if let Ok(bytes_read) = socket.read(&mut buffer).await {
                    if bytes_read == 0 {
                        return;
                    }

                    if let Some(req) = Request::parse(&buffer[..bytes_read]) {
                        let response = router_clone.dispatch(req).await;
                        let _ = socket.write_all(&response.to_bytes()).await;
                    }
                }
            });
        }
    }
}
