use crate::container::Container;
use crate::foundation::ApplicationBuilder;
use crate::http::Request;
use crate::routing::Router;
use crate::support::ServiceProvider;

use std::sync::Arc;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;

#[derive(Default)]
pub struct MiddlewarePipeline {}

#[derive(Default)]
pub struct Scheduler {}

#[derive(Default)]
pub struct Application {
    pub container: Container,
    pub router: Router,
    pub middleware: MiddlewarePipeline,
    pub scheduler: Scheduler,
    pub providers: Vec<Box<dyn ServiceProvider>>,
    booted: bool,
}

impl Application {
    pub fn new() -> Self {
        Self {
            container: Container::new(),
            router: Router::new(),
            providers: Vec::new(),
            middleware: MiddlewarePipeline::default(),
            scheduler: Scheduler::default(),
            booted: false,
        }
    }

    pub fn configure() -> ApplicationBuilder {
        ApplicationBuilder::new()
    }

    /// Register a service provider with the application.
    /// This immediately calls the provider's `register` method.
    pub async fn register_provider<P: ServiceProvider + 'static>(mut self, provider: P) -> Self {
        let provider = Box::new(provider);

        // 1. Run the register method immediately upon binding
        provider.register(&mut self.container).await;

        // 2. If the application is already booted, boot this provider immediately
        if self.booted {
            provider.boot(&self.container, &mut self.router).await;
        }

        self.providers.push(provider);

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
    pub async fn listen(
        mut self,
        host: String,
        port: u16,
    ) -> Result<(), Box<dyn std::error::Error>> {
        // 1. Ensure all providers are booted before accepting traffic
        self.boot().await;

        let addr = format!("{}:{}", host, port);
        let listener = TcpListener::bind(&addr).await?;
        println!("🚀 Ruvel native server running on http://{}", addr);

        let router = Arc::new(self.router);

        loop {
            let (mut socket, _) = listener.accept().await?;
            let router_clone = Arc::clone(&router);

            tokio::spawn(async move {
                let mut buffer = [0u8; 2048]; // Slightly larger buffer for headers

                // Loop over the same socket connection for Keep-Alive
                loop {
                    match socket.read(&mut buffer).await {
                        Ok(0) => break, // Client closed connection
                        Ok(bytes_read) => {
                            if let Some(req) = Request::parse(&buffer[..bytes_read]) {
                                let response = router_clone.dispatch(req).await;
                                if socket.write_all(&response.to_bytes()).await.is_err() {
                                    break;
                                }
                            } else {
                                break;
                            }
                        }
                        Err(_) => break,
                    }
                }
            });
        }
    }
}
