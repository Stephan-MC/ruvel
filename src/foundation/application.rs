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
const READ_CHUNK_SIZE: usize = 8 * 1024; // 8 KiB read buffer to match standard MTU/TCP windows

#[derive(Default)]
pub struct Scheduler {}

#[derive(Default)]
pub struct Application {
    pub container: Container,
    pub router: Router,
    pub scheduler: Scheduler,
    pub providers: Vec<Box<dyn ServiceProvider>>,
    booted: bool,
}

impl Application {
    pub fn new(_base_path: &str) -> Self {
        Self {
            container: Container::new(),
            router: Router::new(),
            providers: Vec::new(),
            scheduler: Scheduler::default(),
            booted: false,
        }
    }

    pub fn configure(base_path: &str) -> ApplicationBuilder {
        ApplicationBuilder::new(Self::new(base_path))
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
        let _kernel = self.make::<Kernel>();

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
                let mut chunk = [0u8; READ_CHUNK_SIZE];
                let mut read_buffer = Vec::with_capacity(READ_CHUNK_SIZE);
                // Pre-allocated response buffer reused across pipelined requests
                let mut response_buffer = Vec::with_capacity(4096);

                'connection: loop {
                    match socket.read(&mut chunk).await {
                        Ok(0) => break, // Client gracefully closed connection
                        Ok(n) => {
                            if read_buffer.len().saturating_add(n) > MAX_REQUEST_BYTES {
                                break;
                            }
                            read_buffer.extend_from_slice(&chunk[..n]);

                            let mut offset = 0;
                            let mut responses_queued = 0;

                            loop {
                                if offset >= read_buffer.len() {
                                    read_buffer.clear();
                                    break;
                                }

                                let (req, consumed) = match Request::parse(&read_buffer[offset..]) {
                                    Ok(Some(parsed)) => parsed,
                                    Ok(None) => {
                                        if offset > 0 {
                                            read_buffer.drain(..offset);
                                        }
                                        break;
                                    }
                                    Err(()) => break 'connection,
                                };

                                let Some(end) = offset.checked_add(consumed) else {
                                    break 'connection;
                                };
                                if consumed == 0 || end > read_buffer.len() {
                                    break 'connection;
                                }

                                // Dispatch request
                                let response = router_clone.dispatch(req).await;

                                // Serialize response into the write buffer
                                response.write_to(&mut response_buffer);
                                responses_queued += 1;

                                offset = end;
                            }

                            // Batched flush: If any responses were queued from this read chunk (pipelining),
                            // flush them all in one single write_all syscall to the OS socket.
                            if responses_queued > 0 {
                                if socket.write_all(&response_buffer).await.is_err() {
                                    break 'connection;
                                }
                                response_buffer.clear();
                            }
                        }
                        Err(_) => break, // Network error
                    }
                }
            });
        }
    }
}
