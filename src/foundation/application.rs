use std::env;
use std::error::Error;
use std::sync::Arc;

use crate::configuration::ApplicationBuilder;
use crate::container::Container;
use crate::database::{DatabaseManager, DatabaseServiceProvider};
use crate::events::EventServiceProvider;
use crate::http::{Kernel, Request};
use crate::routing::{Router, RoutingServiceProvider};
use crate::support::{LogServiceProvider, ServiceProvider};

use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;

const REQUEST_BUFFER_SIZE: usize = match option_env!("RV_REQUEST_BUFFER_SIZE") {
    Some(s) => match usize::from_str_radix(s, 10) {
        Ok(v) if v > 0 => v,
        _ => 8 * 1024,
    },
    None => 8 * 1024,
};

const RESPONSE_BUFFER_SIZE: usize = match option_env!("RV_RESPONSE_BUFFER_SIZE") {
    Some(s) => match usize::from_str_radix(s, 10) {
        Ok(v) if v > 0 => v,
        _ => 4096,
    },
    None => 4096,
};

#[derive(Default)]
pub struct Scheduler {}

pub struct Application {
    pub version: &'static str,
    pub container: Container,
    pub scheduler: Scheduler,
    pub providers: Vec<Box<dyn ServiceProvider>>,
    booted: bool,
}

impl Default for Application {
    fn default() -> Self {
        Self::new("")
    }
}

impl Application {
    pub fn new(_base_path: &str) -> Self {
        let mut app = Self {
            version: "0.9.0", // x-release-please-version
            container: Container::new(),
            providers: Vec::new(),
            scheduler: Scheduler::default(),
            booted: false,
        };

        app.register_base_bindings();
        app.register_base_service_providers();
        app.register_core_container_aliases();

        app
    }

    /// Register the basic bindings into the container.
    pub fn register_base_bindings(&mut self) {
        // Register static container instance
        self.container.instance(Container::new());
    }

    /// Register the base service providers for the application.
    pub fn register_base_service_providers(&mut self) {
        self.register_provider(EventServiceProvider);
        self.register_provider(LogServiceProvider);
        self.register_provider(RoutingServiceProvider);

        let db_provider = Box::new(DatabaseServiceProvider);
        // Synchronously initialize the database service provider bindings into container
        let db_manager = Arc::new(DatabaseManager::new());
        DatabaseManager::register_global(Arc::clone(&db_manager));
        self.container.instance((*db_manager).clone());
        self.providers.push(db_provider);
    }

    /// Register the core class aliases in the container.
    pub fn register_core_container_aliases(&mut self) {
        self.container.alias::<Container>("app");
        self.container.alias::<DatabaseManager>("db");
        self.container.alias::<Router>("router");
        self.container.alias::<Kernel>("kernel");
        self.container.alias::<EventServiceProvider>("events");
        self.container.alias::<LogServiceProvider>("log");
    }

    /// Configure the application and register default kernels.
    pub fn configure(base_path: &str) -> ApplicationBuilder {
        ApplicationBuilder::new(Self::new(base_path)).with_kernels()
    }

    /// Register a service provider with the application.
    /// This immediately calls the provider's `register` method.
    pub fn register_provider<P: ServiceProvider + 'static>(&mut self, provider: P) -> &mut Self {
        let provider = Box::new(provider);

        // 1. Run the register method immediately upon binding
        provider.register(&mut self.container);

        // 2. If the application is already booted, boot this provider immediately
        if self.booted {
            let kernel = self
                .make::<Kernel>()
                .expect("Kernel not found in container");
            let mut router_clone = (*kernel.get_router()).clone();
            provider.boot(&self.container, &mut router_clone);
            kernel.set_router(Arc::new(router_clone));
        }

        self.providers.push(provider);

        self
    }

    /// Boot the application's service providers.
    /// This loops through all registered providers and calls their `boot` methods.
    pub fn boot(&mut self) {
        if self.booted {
            return;
        }

        let kernel = self
            .make::<Kernel>()
            .expect("Kernel not found in container");
        let mut router_clone = (*kernel.get_router()).clone();

        for provider in &self.providers {
            provider.boot(&self.container, &mut router_clone);
        }

        let final_router = Arc::new(router_clone);
        kernel.set_router(Arc::clone(&final_router));
        self.container.instance((*final_router).clone());

        self.booted = true;
    }

    /// Resolve an instance of type `T` from the application container.
    /// Equivalent to Laravel's `$app->make()`.
    pub fn make<T: Send + Sync + 'static>(&self) -> Result<Arc<T>, String> {
        self.container.make()
    }

    pub async fn handle_requests(self) -> Result<(), Box<dyn Error>> {
        self.listen(
            env::var("HOST").unwrap_or_else(|_| "0.0.0.0".to_string()),
            env::var("PORT")
                .unwrap_or_else(|_| "7883".to_string())
                .parse::<u16>()
                .unwrap(),
        )
        .await
    }

    /// Start the TCP listener (analogous to `public/index.php` `$kernel->handle()`).
    async fn listen(self, host: String, port: u16) -> Result<(), Box<dyn Error>> {
        let addr = format!("{}:{}", host, port);
        let listener = TcpListener::bind(&addr).await?;
        println!("🚀 Ruvel native server running on http://{}", addr);

        self.serve(listener).await
    }

    /// Serve HTTP/1.1 connections on an already-bound [`TcpListener`].
    ///
    /// Each accepted connection maintains a streaming read buffer (`read_buffer`) and batch cursor
    /// (`read_pos`) to handle keep-alive, HTTP/1.1 pipelining, and arbitrary TCP fragmentation:
    /// - When [`Request::parse`] returns `Ok(None)`, any unparsed tail bytes are compacted to the
    ///   front of `read_buffer` so the next bounded `read_buf` call can append trailing TCP fragments.
    /// - When [`Request::parse`] returns `Ok(Some((req, consumed)))`, `read_pos` advances by
    ///   `consumed` and any remaining bytes in `read_buffer` are immediately parsed as pipelined
    ///   requests before flushing `response_buffer` in a single `write_all` call.
    /// - When [`Request::parse`] returns `Err(())`, any already-queued pipelined responses plus an
    ///   `HTTP/1.1 400 Bad Request` (`Connection: close`) response are flushed and the socket closes.
    pub async fn serve(mut self, listener: TcpListener) -> Result<(), Box<dyn Error>> {
        // 1. Ensure all providers are booted before accepting traffic
        self.boot();

        // Resolve the HTTP Kernel directly from the container
        let kernel: Arc<Kernel> = match self.make::<Kernel>() {
            Ok(k) => k,
            Err(e) => panic!("Failed to resolve Kernel from container: {}", e),
        };

        loop {
            let (mut socket, _) = listener.accept().await?;
            let _ = socket.set_nodelay(true);
            let kernel_clone = Arc::clone(&kernel);

            tokio::spawn(async move {
                let mut read_buffer = [0u8; REQUEST_BUFFER_SIZE];
                let mut read_len = 0usize;
                let mut response_buffer = [0u8; RESPONSE_BUFFER_SIZE];
                let mut resp_len = 0usize;

                'connection: loop {
                    if read_len >= REQUEST_BUFFER_SIZE {
                        // TODO: Throw an error warning the about request size to large
                        break;
                    }

                    match socket.read(&mut read_buffer[read_len..]).await {
                        Ok(0) => break, // Client gracefully closed connection
                        Ok(n) => {
                            read_len += n;
                            let mut read_pos = 0usize;
                            let mut close_connection = false;

                            while read_pos < read_len {
                                let (req, consumed) = match Request::parse(
                                    &read_buffer[read_pos..read_len],
                                ) {
                                    Ok(Some(parsed)) => parsed,
                                    // Partial frame (split headers, Content-Length body, or chunked
                                    // payload): preserve `&read_buffer[read_pos..read_len]` for the next read.
                                    Ok(None) => break,
                                    Err(()) => {
                                        if resp_len > 0
                                            && socket
                                                .write_all(&response_buffer[..resp_len])
                                                .await
                                                .is_err()
                                        {
                                            break 'connection;
                                        }
                                        let _ = socket
                                                .write_all(b"HTTP/1.1 400 Bad Request\r\nContent-Length: 0\r\nConnection: close\r\n\r\n")
                                                .await;
                                        let _ = socket.shutdown().await;
                                        break 'connection;
                                    }
                                };

                                let Some(end) = read_pos.checked_add(consumed) else {
                                    break 'connection;
                                };
                                if consumed == 0 || end > read_len {
                                    break 'connection;
                                };

                                let req_wants_close =
                                    should_close_connection(req.header("Connection"));

                                // Dispatch request through HTTP Kernel
                                let response = kernel_clone.handle(req).await;
                                let res_wants_close =
                                    should_close_connection(response.headers.get("Connection"));

                                // Serialize response into the fixed write buffer, flushing early if needed.
                                if let Some(written) =
                                    response.write_to_slice(&mut response_buffer[resp_len..])
                                {
                                    resp_len += written;
                                } else {
                                    if resp_len > 0 {
                                        if socket
                                            .write_all(&response_buffer[..resp_len])
                                            .await
                                            .is_err()
                                        {
                                            break 'connection;
                                        }
                                        resp_len = 0;
                                    }
                                    if let Some(written) =
                                        response.write_to_slice(&mut response_buffer)
                                    {
                                        resp_len = written;
                                    } else if socket.write_all(&response.to_bytes()).await.is_err()
                                    {
                                        break 'connection;
                                    }
                                }

                                read_pos = end;

                                if req_wants_close || res_wants_close {
                                    close_connection = true;
                                    break;
                                }
                            }

                            // Compact any consumed prefix so `read_buffer[..read_len]` starts the
                            // next read iteration with only the unparsed in-flight request fragment.
                            if read_pos >= read_len {
                                read_len = 0;
                            } else if read_pos > 0 {
                                let remaining = read_len - read_pos;
                                read_buffer.copy_within(read_pos..read_len, 0);
                                read_len = remaining;
                            }

                            // Batched flush: If any responses are queued in `response_buffer`,
                            // flush them all in one single write_all syscall to the OS socket.
                            if resp_len > 0 {
                                if socket
                                    .write_all(&response_buffer[..resp_len])
                                    .await
                                    .is_err()
                                {
                                    break 'connection;
                                }
                                resp_len = 0;
                            }
                            if close_connection {
                                let _ = socket.shutdown().await;
                                break 'connection;
                            }
                        }
                        Err(_) => break, // Network error
                    }
                }
            });
        }
    }
}

/// Returns `true` when a `Connection` header value contains the `"close"` token
/// (case-insensitive, comma-separated per RFC 9110 §7.6.1).
fn should_close_connection(value: Option<&str>) -> bool {
    let Some(value) = value else {
        return false;
    };
    value
        .split(',')
        .any(|token| token.trim().eq_ignore_ascii_case("close"))
}
