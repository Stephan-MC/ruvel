use std::env;
use std::error::Error;
use std::sync::Arc;

use crate::configuration::ApplicationBuilder;
use crate::container::Container;
use crate::http::{Kernel, Request};
use crate::routing::Router;
use crate::support::ServiceProvider;

use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;

const MAX_REQUEST_BYTES: usize = 64 * 1024;

#[derive(Default)]
pub struct Scheduler {}

#[derive(Default)]
pub struct Application {
    base_path: String,
    pub container: Container,
    pub router: Router,
    pub scheduler: Scheduler,
    pub providers: Vec<Box<dyn ServiceProvider>>,
    booted: bool,
}

impl Application {
    pub fn new(base_path: &str) -> Self {
        Self {
            base_path: base_path.to_string(),
            container: Container::new(),
            router: Router::new(),
            providers: Vec::new(),
            scheduler: Scheduler::default(),
            booted: false,
        }
    }

    pub fn configure(base_path: &str) -> ApplicationBuilder {
        ApplicationBuilder::new(Self::new(base_path))
        // .with_kernels()
        // .with_events()
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

    /// Resolve an instance of type `T` from the application container.
    /// Equivalent to Laravel's `$app->make()`.
    pub fn make<T: Send + Sync + 'static>(&self) -> Result<Arc<T>, String> {
        self.container.make()
    }

    pub async fn handle_requests(self) -> Result<(), Box<dyn Error>> {
        let kernel = self.make::<Kernel>();

        self.listen(
            env::var("HOST").unwrap_or_else(|_| "0.0.0.0".to_string()),
            env::var("PORT")
                .unwrap_or_else(|_| "7883".to_string())
                .parse::<u16>()
                .unwrap(),
        )
        .await
    }

    /// Start the TCP Listener (analogous to public/index.php `$kernel->handle()`)
    async fn listen(mut self, host: String, port: u16) -> Result<(), Box<dyn std::error::Error>> {
        // 1. Ensure all providers are booted before accepting traffic
        self.boot().await;

        let addr = format!("{}:{}", host, port);
        let listener = TcpListener::bind(&addr).await?;
        println!("🚀 Ruvel native server running on http://{}", addr);

        let router = Arc::new(self.router);

        loop {
            let (mut socket, _) = listener.accept().await?;
            let _ = socket.set_nodelay(true);
            let router_clone = Arc::clone(&router);

            tokio::spawn(async move {
                // 1. Fixed-size chunk for reading from the OS network stack
                let mut chunk = [0u8; 2048];
                // 2. Buffer for assembling fragmented packets and handling pipelining
                let mut read_buffer = Vec::with_capacity(2048);
                // 3. Pre-allocated response buffer (used repeatedly for keep-alive)
                let mut response_buffer = Vec::with_capacity(1024);

                'connection: loop {
                    // Read directly from the raw TCP socket
                    match socket.read(&mut chunk).await {
                        Ok(0) => break, // Client gracefully closed connection
                        Ok(n) => {
                            // Security: Prevent buffer overflow / memory exhaustion
                            if read_buffer.len().saturating_add(n) > MAX_REQUEST_BYTES {
                                break;
                            }
                            read_buffer.extend_from_slice(&chunk[..n]);

                            let mut offset = 0;
                            loop {
                                if offset >= read_buffer.len() {
                                    // Fully consumed. `clear` keeps the allocation for the next read.
                                    read_buffer.clear();
                                    break;
                                }

                                // Attempt to parse a complete request from the unconsumed tail.
                                let (req, consumed) = match Request::parse(&read_buffer[offset..]) {
                                    Ok(Some(parsed)) => parsed,
                                    Ok(None) => {
                                        // One shift per read, and only when a previous request
                                        // in this read left a partial one behind.
                                        if offset > 0 {
                                            read_buffer.drain(..offset);
                                        }
                                        break;
                                    }
                                    Err(()) => break 'connection, // Malformed payload, drop connection
                                };

                                let Some(end) = offset.checked_add(consumed) else {
                                    break 'connection;
                                };
                                if consumed == 0 || end > read_buffer.len() {
                                    break 'connection;
                                }

                                // Dispatch to the Router
                                let response = router_clone.dispatch(req).await;

                                // Reset the response buffer (keeps capacity)
                                response_buffer.clear();

                                // Format your response into the contiguous Vec<u8>
                                response.write_to(&mut response_buffer);

                                // Performance: A single write_all syscall directly to the kernel,
                                // completely bypassing the overhead of BufWriter.
                                if socket.write_all(&response_buffer).await.is_err() {
                                    break 'connection;
                                }

                                offset = end;
                            }
                        }
                        Err(_) => break, // Network error
                    }
                }
            });
        }
    }
}
