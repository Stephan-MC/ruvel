use std::env;
use std::error::Error;
use std::sync::Arc;

use crate::configuration::ApplicationBuilder;
use crate::container::Container;
use crate::database::{DatabaseManager, DatabaseServiceProvider};
use crate::events::EventServiceProvider;
use crate::http::parser::{self, HttpVersion, Status as ParseStatus, h1::H1Cursor};
use crate::http::{Kernel, Method, Request};
use crate::routing::{Router, RoutingServiceProvider};
use crate::support::{LogServiceProvider, ServiceProvider};

use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};

const RV_REQUEST_BUFFER_SIZE: usize = 8 * 1024;
const RV_RESPONSE_BUFFER_SIZE: usize = 4 * 1024;
const RV_MAX_REQUEST_SIZE: usize = 2 * 1024 * 1024;

const COMPILE_REQUEST_BUFFER_SIZE: usize = match option_env!("RV_REQUEST_BUFFER_SIZE") {
    Some(s) => match usize::from_str_radix(s, 10) {
        Ok(v) if v > 0 => v,
        _ => RV_REQUEST_BUFFER_SIZE,
    },
    None => RV_REQUEST_BUFFER_SIZE,
};

const COMPILE_RESPONSE_BUFFER_SIZE: usize = match option_env!("RV_RESPONSE_BUFFER_SIZE") {
    Some(s) => match usize::from_str_radix(s, 10) {
        Ok(v) if v > 0 => v,
        _ => RV_RESPONSE_BUFFER_SIZE,
    },
    None => RV_RESPONSE_BUFFER_SIZE,
};

const COMPILE_MAX_REQUEST_SIZE: usize = match option_env!("RV_MAX_REQUEST_SIZE") {
    Some(s) => match usize::from_str_radix(s, 10) {
        Ok(v) if v > 0 => v,
        _ => RV_MAX_REQUEST_SIZE,
    },
    None => RV_MAX_REQUEST_SIZE,
};

const HTTP_400_RESPONSE: &[u8] =
    b"HTTP/1.1 400 Bad Request\r\nContent-Length: 0\r\nConnection: close\r\n\r\n";
const HTTP_413_RESPONSE: &[u8] =
    b"HTTP/1.1 413 Payload Too Large\r\nContent-Length: 0\r\nConnection: close\r\n\r\n";
const HTTP_417_RESPONSE: &[u8] =
    b"HTTP/1.1 417 Expectation Failed\r\nContent-Length: 0\r\nConnection: close\r\n\r\n";
const HTTP_431_RESPONSE: &[u8] =
    b"HTTP/1.1 431 Request Header Fields Too Large\r\nContent-Length: 0\r\nConnection: close\r\n\r\n";
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
fn parser_error_response(err: &parser::Error<'_>) -> &'static [u8] {
    match err {
        parser::Error::UnsupportedProtocol => HTTP_505_RESPONSE,
        parser::Error::TooManyHeaders | parser::Error::HeadersTooLarge => HTTP_431_RESPONSE,
        parser::Error::ExpectationFailed => HTTP_417_RESPONSE,
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

    /// Serve HTTP/1.x connections on an already-bound [`TcpListener`].
    ///
    /// Each accepted connection maintains a dynamically bounded streaming read buffer
    /// (`read_buffer`, starting at `RV_REQUEST_BUFFER_SIZE` and growing up to
    /// `RV_MAX_REQUEST_SIZE`) and batch cursor (`read_pos`) to handle keep-alive, HTTP/1.1
    /// pipelining, `Expect: 100-continue`, and arbitrary TCP fragmentation:
    /// - When [`Request::capture_incremental`] returns `Ok(ParseStatus::Incomplete)`, any unparsed
    ///   tail bytes are compacted to the front of `read_buffer` so the next bounded `read` call
    ///   can append trailing TCP fragments while preserving `H1Cursor` progress. If an announced
    ///   `Content-Length` or incoming payload exceeds `RV_MAX_REQUEST_SIZE`, `413 Payload Too Large`
    ///   is flushed and the connection closes.
    /// - When [`Request::capture_incremental`] returns `Ok(ParseStatus::Complete(req, consumed))`,
    ///   `read_pos` advances by `consumed` and any remaining bytes in `read_buffer` are immediately
    ///   parsed as pipelined requests before flushing `response_buffer` in a single `write_all` call.
    /// - When [`Request::capture_incremental`] returns `Err(err)`, any already-queued pipelined
    ///   responses plus an appropriate error response (`400`, `417`, `431`, or `505` with
    ///   `Connection: close`) are flushed and the socket closes.
    pub async fn serve(mut self, listener: TcpListener) -> Result<(), Box<dyn Error>> {
        // 1. Ensure all providers are booted before accepting traffic
        self.boot();

        // Resolve the HTTP Kernel directly from the container
        let kernel: Arc<Kernel> = match self.make::<Kernel>() {
            Ok(k) => k,
            Err(e) => panic!("Failed to resolve Kernel from container: {}", e),
        };

        let max_request_size = resolve_env_usize("RV_MAX_REQUEST_SIZE", COMPILE_MAX_REQUEST_SIZE);
        let initial_read_buf_size =
            resolve_env_usize("RV_REQUEST_BUFFER_SIZE", COMPILE_REQUEST_BUFFER_SIZE)
                .min(max_request_size);
        let response_buf_size =
            resolve_env_usize("RV_RESPONSE_BUFFER_SIZE", COMPILE_RESPONSE_BUFFER_SIZE);

        loop {
            let (mut socket, _) = listener.accept().await?;
            let _ = socket.set_nodelay(true);
            let kernel_clone = Arc::clone(&kernel);

            tokio::spawn(async move {
                let mut read_buffer = vec![0u8; initial_read_buf_size];
                let mut read_len = 0usize;
                let mut response_buffer = vec![0u8; response_buf_size];
                let mut resp_len = 0usize;
                let mut cursor = H1Cursor::new();

                'connection: loop {
                    if read_len >= read_buffer.len() {
                        if read_buffer.len() >= max_request_size {
                            write_error_and_close(
                                &mut socket,
                                &response_buffer,
                                resp_len,
                                HTTP_413_RESPONSE,
                            )
                            .await;
                            break;
                        }
                        let new_len = read_buffer.len().saturating_mul(2).min(max_request_size);
                        read_buffer.resize(new_len, 0);
                    }

                    match socket.read(&mut read_buffer[read_len..]).await {
                        Ok(0) => break, // Client gracefully closed connection
                        Ok(n) => {
                            read_len += n;
                            let mut read_pos = 0usize;
                            let mut close_connection = false;

                            while read_pos < read_len {
                                let (req, consumed) = match Request::capture_incremental(
                                    &read_buffer[read_pos..read_len],
                                    &mut cursor,
                                ) {
                                    Ok(ParseStatus::Complete(req, consumed)) => {
                                        cursor.reset();
                                        (req, consumed)
                                    }
                                    // Partial frame (split headers, Content-Length body, or chunked
                                    // payload): preserve `&read_buffer[read_pos..read_len]` for the next read.
                                    Ok(ParseStatus::Incomplete) => {
                                        if cursor
                                            .expected_total_len()
                                            .is_some_and(|total| total > max_request_size)
                                        {
                                            write_error_and_close(
                                                &mut socket,
                                                &response_buffer,
                                                resp_len,
                                                HTTP_413_RESPONSE,
                                            )
                                            .await;
                                            break 'connection;
                                        }
                                        if cursor.take_expect_100_continue() {
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
                                            if socket.write_all(HTTP_100_CONTINUE).await.is_err() {
                                                break 'connection;
                                            }
                                        }
                                        break;
                                    }
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

                                let Some(end) = read_pos.checked_add(consumed) else {
                                    break 'connection;
                                };
                                if consumed == 0 || end > read_len {
                                    break 'connection;
                                };

                                let is_head = req.method == Method::Head;
                                let req_wants_close = should_close_request_connection(
                                    req.version,
                                    req.headers.get_all("Connection"),
                                );

                                // Dispatch request through HTTP Kernel
                                let mut response = kernel_clone.handle(req).await;
                                let res_wants_close = should_close_response_connection(
                                    response.headers.get_all("Connection"),
                                );
                                if req_wants_close && !res_wants_close {
                                    response.headers.set("Connection", "close");
                                }

                                // Serialize response into the write buffer, flushing early if needed.
                                if let Some(written) = response
                                    .write_to_slice_for(&mut response_buffer[resp_len..], is_head)
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
                                if read_buffer.len() > initial_read_buf_size {
                                    read_buffer = vec![0u8; initial_read_buf_size];
                                }
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

/// Returns `(has_close, has_keep_alive)` across comma-separated `Connection` header values.
fn connection_token_flags<'a>(values: impl Iterator<Item = &'a str>) -> (bool, bool) {
    let mut has_close = false;
    let mut has_keep_alive = false;

    for value in values {
        for token in value.split(',') {
            let trimmed = token.trim();
            if trimmed.eq_ignore_ascii_case("close") {
                has_close = true;
            } else if trimmed.eq_ignore_ascii_case("keep-alive") {
                has_keep_alive = true;
            }
        }
    }

    (has_close, has_keep_alive)
}

fn should_close_request_connection<'a>(
    version: HttpVersion,
    values: impl Iterator<Item = &'a str>,
) -> bool {
    let (has_close, has_keep_alive) = connection_token_flags(values);
    if has_close {
        return true;
    }
    version == HttpVersion::Http1_0 && !has_keep_alive
}

fn should_close_response_connection<'a>(values: impl Iterator<Item = &'a str>) -> bool {
    connection_token_flags(values).0
}
