use crate::container::Container;
use crate::foundation::ApplicationBuilder;
use crate::http::Request;
use crate::routing::Router;
use crate::support::ServiceProvider;

use std::sync::Arc;
use tokio::io::{AsyncReadExt, AsyncWriteExt, BufWriter};
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

    // Make sure to add this import at the top of application.rs:
    // use tokio::io::{AsyncReadExt, AsyncWriteExt, BufWriter};

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
            let _ = socket.set_nodelay(true);
            let router_clone = Arc::clone(&router);

            tokio::spawn(async move {
                // 1. Split the socket to buffer the write half
                let (mut read_half, write_half) = socket.into_split();
                let mut writer = BufWriter::new(write_half);

                let mut read_buffer = [0u8; 2048];

                // 2. Pre-allocate the response buffer EXACTLY ONCE per connection
                let mut response_buffer = Vec::with_capacity(1024);

                loop {
                    match read_half.read(&mut read_buffer).await {
                        Ok(0) => break, // Client closed connection
                        Ok(bytes_read) => {
                            if let Some(req) = Request::parse(&read_buffer[..bytes_read]) {
                                let response = router_clone.dispatch(req).await;

                                // 3. Reset buffer length to 0 (keeps capacity, avoids allocation)
                                response_buffer.clear();

                                // 4. Write directly into the existing buffer
                                response.write_to(&mut response_buffer);

                                // 5. Write to BufWriter and flush to the kernel
                                if writer.write_all(&response_buffer).await.is_err() {
                                    break;
                                }
                                if writer.flush().await.is_err() {
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
