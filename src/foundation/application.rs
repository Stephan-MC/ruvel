use std::env;
use std::error::Error;
use std::sync::Arc;

use crate::configuration::ApplicationBuilder;
use crate::constants::RESPONSE_BUFFER_SIZE;
use crate::container::Container;
use crate::database::{DatabaseManager, DatabaseServiceProvider};
use crate::events::EventServiceProvider;
use crate::http::parser::{self, h1};
use crate::http::{Kernel, Request};
use crate::routing::{Router, RoutingServiceProvider};
use crate::support::{LogServiceProvider, ServiceProvider};

use tokio::io::AsyncWriteExt;
use tokio::net::{TcpListener, TcpStream};

const HTTP_400_RESPONSE: &[u8] =
    b"HTTP/1.1 400 Bad Request\r\nContent-Length: 0\r\nConnection: close\r\n\r\n";
const HTTP_413_RESPONSE: &[u8] =
    b"HTTP/1.1 413 Payload Too Large\r\nContent-Length: 0\r\nConnection: close\r\n\r\n";
const HTTP_417_RESPONSE: &[u8] =
    b"HTTP/1.1 417 Expectation Failed\r\nContent-Length: 0\r\nConnection: close\r\n\r\n";
const HTTP_431_RESPONSE: &[u8] =
    b"HTTP/1.1 431 Request Header Fields Too Large\r\nContent-Length: 0\r\nConnection: close\r\n\r\n";
const HTTP_501_RESPONSE: &[u8] =
    b"HTTP/1.1 501 Not Implemented\r\nContent-Length: 0\r\nConnection: close\r\n\r\n";
const HTTP_505_RESPONSE: &[u8] =
    b"HTTP/1.1 505 HTTP Version Not Supported\r\nContent-Length: 0\r\nConnection: close\r\n\r\n";
const HTTP_100_CONTINUE: &[u8] = b"HTTP/1.1 100 Continue\r\n\r\n";

#[inline]
fn parser_error_response(err: &parser::Error) -> &'static [u8] {
    match err.status() {
        Some(413) => HTTP_413_RESPONSE,
        Some(417) => HTTP_417_RESPONSE,
        Some(431) => HTTP_431_RESPONSE,
        Some(501) => HTTP_501_RESPONSE,
        Some(505) => HTTP_505_RESPONSE,
        _ => HTTP_400_RESPONSE,
    }
}

async fn write_error_and_close(
    socket: &mut TcpStream,
    response_buffer: &[u8],
    resp_len: usize,
    error_bytes: &[u8],
) {
    if resp_len > 0
        && socket
            .write_all(&response_buffer[..resp_len])
            .await
            .is_err()
    {
        return;
    }
    let _ = socket.write_all(error_bytes).await;
    let _ = socket.shutdown().await;
}

const HTTP_400_RESPONSE: &[u8] =
    b"HTTP/1.1 400 Bad Request\r\nContent-Length: 0\r\nConnection: close\r\n\r\n";
const HTTP_413_RESPONSE: &[u8] =
    b"HTTP/1.1 413 Payload Too Large\r\nContent-Length: 0\r\nConnection: close\r\n\r\n";
const HTTP_417_RESPONSE: &[u8] =
    b"HTTP/1.1 417 Expectation Failed\r\nContent-Length: 0\r\nConnection: close\r\n\r\n";
const HTTP_431_RESPONSE: &[u8] =
    b"HTTP/1.1 431 Request Header Fields Too Large\r\nContent-Length: 0\r\nConnection: close\r\n\r\n";
const HTTP_501_RESPONSE: &[u8] =
    b"HTTP/1.1 501 Not Implemented\r\nContent-Length: 0\r\nConnection: close\r\n\r\n";
const HTTP_505_RESPONSE: &[u8] =
    b"HTTP/1.1 505 HTTP Version Not Supported\r\nContent-Length: 0\r\nConnection: close\r\n\r\n";
const HTTP_100_CONTINUE: &[u8] = b"HTTP/1.1 100 Continue\r\n\r\n";

fn resolve_env_usize(name: &str, compile_default: usize) -> usize {
    match env::var(name) {
        Ok(val) => match val.trim().parse::<usize>() {
            Ok(v) if v > 0 => v,
            _ => compile_default,
        },
        Err(_) => compile_default,
    }
}

#[inline]
fn parser_error_response(err: &parser::Error) -> &'static [u8] {
    match err.status() {
        Some(413) => HTTP_413_RESPONSE,
        Some(417) => HTTP_417_RESPONSE,
        Some(431) => HTTP_431_RESPONSE,
        Some(501) => HTTP_501_RESPONSE,
        Some(505) => HTTP_505_RESPONSE,
        _ => HTTP_400_RESPONSE,
    }
}

async fn write_error_and_close(
    socket: &mut TcpStream,
    response_buffer: &[u8],
    resp_len: usize,
    error_bytes: &[u8],
) {
    if resp_len > 0
        && socket
            .write_all(&response_buffer[..resp_len])
            .await
            .is_err()
    {
        return;
    }
    let _ = socket.write_all(error_bytes).await;
    let _ = socket.shutdown().await;
}

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
            version: "0.12.1", // x-release-please-version
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

    /// Serve HTTP/1.x connections on an already-bound [`TcpListener`].
    ///
    /// Each accepted connection owns an [`h1::Conn`] configured from the compile-time limits in
    /// [`crate::constants`] (`RV_REQUEST_BUFFER_SIZE`, `RV_REQUEST_HEADER_SIZE`,
    /// `RV_REQUEST_BODY_SIZE`, `RV_MAX_REQUEST_BODY_SIZE`) to handle keep-alive,
    /// HTTP/1.1 pipelining, in-place chunked decoding, `Expect: 100-continue`, and arbitrary TCP
    /// fragmentation:
    /// - [`h1::Conn::try_next_request`] drains all complete buffered requests (pipelining) without
    ///   blocking on socket I/O, materializing each request via [`Request::from_h1`] and batching
    ///   serialized responses into `response_buffer` before flushing in a single `write_all` call.
    /// - When more bytes are required, [`h1::Conn::take_expect_100_continue`] emits an interim
    ///   `100 Continue` response if requested, then [`h1::Conn::prepare_read`] and
    ///   [`h1::Conn::fill_async`] compact/grow the connection buffer and read from the socket.
    /// - Any parser or limit error flushes already-queued pipelined responses plus the appropriate
    ///   error status (`400`, `413`, `417`, `431`, `501`, or `505` with `Connection: close`) and
    ///   closes the connection.
    pub async fn serve(mut self, listener: TcpListener) -> Result<(), Box<dyn Error>> {
        // 1. Ensure all providers are booted before accepting traffic
        self.boot();

        // Resolve the HTTP Kernel directly from the container
        let kernel: Arc<Kernel> = match self.make::<Kernel>() {
            Ok(k) => k,
            Err(e) => panic!("Failed to resolve Kernel from container: {}", e),
        };

        // Initiate the listener's loop that runs indefinitely until stopped
        loop {
            let (mut socket, _) = listener.accept().await?;
            let _ = socket.set_nodelay(true);
            let kernel_clone = Arc::clone(&kernel);

            tokio::spawn(async move {
                let mut conn = h1::Conn::new();
                let mut response_buffer = vec![0u8; RESPONSE_BUFFER_SIZE];
                let mut resp_len = 0usize;

                'connection: loop {
                    let mut close_connection = false;

                    // Drain every complete request already buffered in `conn` (pipelining).
                    loop {
                        let frame = match conn.try_next_request() {
                            Ok(Some(frame)) => frame,
                            Ok(None) => break,
                            Err(ref err) => {
                                write_error_and_close(
                                    &mut socket,
                                    &response_buffer,
                                    resp_len,
                                    parser_error_response(err),
                                )
                                .await;
                                break 'connection;
                            }
                        };

                        let (req, is_head, keep_alive) = {
                            let raw = conn.request(frame);
                            let is_head = raw.method() == h1::Method::Head;
                            let keep_alive = raw.keep_alive();
                            match Request::from_h1(raw) {
                                Ok(req) => (req, is_head, keep_alive),
                                Err(ref err) => {
                                    write_error_and_close(
                                        &mut socket,
                                        &response_buffer,
                                        resp_len,
                                        parser_error_response(err),
                                    )
                                    .await;
                                    break 'connection;
                                }
                            }
                        };
                        conn.consume(frame);

                        let req_wants_close = !keep_alive;

                        // Dispatch request through HTTP Kernel
                        let mut response = kernel_clone.handle(req).await;
                        let res_wants_close = should_close_response_connection(
                            response.headers.get_all("Connection"),
                        );
                        if req_wants_close && !res_wants_close {
                            response.headers.set("Connection", "close");
                        }

                        // Serialize response into the write buffer, flushing early if needed.
                        if let Some(written) =
                            response.write_to_slice_for(&mut response_buffer[resp_len..], is_head)
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
                                response.write_to_slice_for(&mut response_buffer, is_head)
                            {
                                resp_len = written;
                            } else if socket
                                .write_all(&response.to_bytes_for(is_head))
                                .await
                                .is_err()
                            {
                                break 'connection;
                            }
                        }

                        if req_wants_close || res_wants_close {
                            close_connection = true;
                            break;
                        }
                    }

                    // Batched flush: flush any queued responses before awaiting more socket reads.
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

                    // Emit interim `100 Continue` if the in-progress request requested it.
                    if conn.take_expect_100_continue()
                        && socket.write_all(HTTP_100_CONTINUE).await.is_err()
                    {
                        break 'connection;
                    }

                    // Prepare buffer capacity for current phase and read from socket.
                    conn.prepare_read();
                    match conn.fill_async(&mut socket).await {
                        Ok(0) => break 'connection, // Client closed connection
                        Ok(_) => {}
                        Err(_) => break 'connection, // Network error
                    }
                }
            });
        }
    }
}

fn should_close_response_connection<'a>(values: impl Iterator<Item = &'a str>) -> bool {
    for value in values {
        for token in value.split(',') {
            if token.trim().eq_ignore_ascii_case("close") {
                return true;
            }
        }
    }
    false
}
