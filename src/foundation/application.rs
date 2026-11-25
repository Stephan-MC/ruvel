use std::env;
use std::error::Error;
use std::future::poll_fn;
use std::sync::Arc;
use std::task::Poll;

use crate::configuration::ApplicationBuilder;
use crate::constants::RESPONSE_BUFFER_SIZE;
use crate::container::Container;
use crate::database::{DatabaseManager, DatabaseServiceProvider};
use crate::events::EventServiceProvider;
use crate::http::parser::h1;
use crate::http::{Kernel, Request, Response, Status};
use crate::routing::{Router, RoutingServiceProvider};
use crate::support::{LogServiceProvider, ServiceProvider};

use tokio::io::{AsyncRead, AsyncWrite, AsyncWriteExt};
use tokio::net::TcpListener;

const HTTP_100_CONTINUE: &[u8] = b"HTTP/1.1 100 Continue\r\n\r\n";

async fn flush_response_buffer<S: AsyncWrite + Unpin>(
    socket: &mut S,
    buf: &[u8],
    len: &mut usize,
) -> bool {
    let n = *len;
    if n == 0 {
        return true;
    }
    *len = 0;
    socket.write_all(&buf[..n]).await.is_ok()
}

async fn write_error_and_close<S: AsyncWrite + Unpin>(
    socket: &mut S,
    response_buffer: &[u8],
    resp_len: usize,
    status: Status,
) {
    let mut pending = resp_len;
    if !flush_response_buffer(socket, response_buffer, &mut pending).await {
        return;
    }

    let code = status.code();
    let code_bytes = [
        b'0' + ((code / 100) % 10) as u8,
        b'0' + ((code / 10) % 10) as u8,
        b'0' + (code % 10) as u8,
    ];
    let reason = status.message().as_bytes();

    let mut frame = [0u8; 192];
    let mut pos = 0usize;
    for part in [
        b"HTTP/1.1 " as &[u8],
        &code_bytes,
        b" ",
        reason,
        b"\r\nContent-Length: 0\r\nConnection: close\r\n",
    ] {
        frame[pos..pos + part.len()].copy_from_slice(part);
        pos += part.len();
    }
    crate::http::date::with_current_date_header(|line| {
        frame[pos..pos + line.len()].copy_from_slice(line);
        pos += line.len();
    });
    frame[pos..pos + 2].copy_from_slice(b"\r\n");
    pos += 2;

    let _ = socket.write_all(&frame[..pos]).await;
    let _ = socket.shutdown().await;
}

async fn write_response_buffered<S: AsyncWrite + Unpin>(
    socket: &mut S,
    response_buffer: &mut [u8],
    resp_len: &mut usize,
    response: &Response,
    is_head: bool,
) -> bool {
    if let Some(written) = response.write_to_slice_for(&mut response_buffer[*resp_len..], is_head) {
        *resp_len += written;
        return true;
    }
    if !flush_response_buffer(socket, response_buffer, resp_len).await {
        return false;
    }
    if let Some(written) = response.write_to_slice_for(response_buffer, is_head) {
        *resp_len = written;
        true
    } else {
        socket
            .write_all(&response.to_bytes_for(is_head))
            .await
            .is_ok()
    }
}

#[derive(Default)]
pub struct Scheduler {}

pub struct Application {
    pub version: &'static str,
    pub container: Container,
    pub scheduler: Scheduler,
    pub providers: Vec<Box<dyn ServiceProvider>>,
    booted: bool,
    /// Used to enaled or disable no delay mode in TCP Listener
    tcp_no_delay: bool,
}

impl Default for Application {
    fn default() -> Self {
        Self::new("")
    }
}

impl Application {
    pub fn new(_base_path: &str) -> Self {
        let mut app = Self {
            version: "0.15.0", // x-release-please-version
            container: Container::new(),
            providers: Vec::new(),
            scheduler: Scheduler::default(),
            booted: false,
            tcp_no_delay: true,
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

    pub fn set_tcp_no_delay(&mut self, delay: bool) -> &mut Self {
        self.tcp_no_delay = delay;

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
    ///   immediately-ready serialized responses into `response_buffer` while flushing prior
    ///   buffered responses on `Poll::Pending` (no reverse head-of-line blocking) or buffer
    ///   saturation (`RESPONSE_BUFFER_SIZE`).
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

        let _date_ticker = crate::http::date::start_server_ticker();

        // Initiate the listener's loop that runs indefinitely until stopped
        loop {
            let (socket, _) = listener.accept().await?;
            let _ = socket.set_nodelay(self.tcp_no_delay);
            let kernel_clone = Arc::clone(&kernel);

            tokio::spawn(serve_connection(socket, kernel_clone));
        }
    }
}

async fn serve_connection<S: AsyncRead + AsyncWrite + Unpin>(mut socket: S, kernel: Arc<Kernel>) {
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
                Err(status) => {
                    write_error_and_close(&mut socket, &response_buffer, resp_len, status).await;
                    break 'connection;
                }
            };

            let (req, is_head, keep_alive) = {
                let raw = conn.request(frame);
                let is_head = raw.method() == h1::Method::Head;
                let keep_alive = raw.keep_alive();
                match Request::from_h1(raw) {
                    Ok(req) => (req, is_head, keep_alive),
                    Err(status) => {
                        write_error_and_close(&mut socket, &response_buffer, resp_len, status)
                            .await;
                        break 'connection;
                    }
                }
            };
            conn.consume(frame);

            let req_wants_close = !keep_alive;

            // Hybrid flush-on-Pending dispatch:
            // If prior pipelined responses are buffered, probe the future once with the real
            // task waker. Keep batching on Poll::Ready; flush buffered bytes immediately on
            // Poll::Pending so a slow request cannot stall already-finished responses.
            let mut fut = kernel.handle(req);
            let mut response = if resp_len > 0 {
                match poll_fn(|cx| Poll::Ready(fut.as_mut().poll(cx))).await {
                    Poll::Ready(resp) => resp,
                    Poll::Pending => {
                        if !flush_response_buffer(&mut socket, &response_buffer, &mut resp_len)
                            .await
                        {
                            break 'connection;
                        }
                        fut.await
                    }
                }
            } else {
                fut.await
            };

            let res_wants_close =
                should_close_response_connection(response.headers.get_all("Connection"));
            if req_wants_close && !res_wants_close {
                response.headers.set("Connection", "close");
            }

            if !write_response_buffered(
                &mut socket,
                &mut response_buffer,
                &mut resp_len,
                &response,
                is_head,
            )
            .await
            {
                break 'connection;
            }

            if req_wants_close || res_wants_close {
                close_connection = true;
                break;
            }
        }

        // Batched flush: flush any queued responses before awaiting more socket reads.
        if !flush_response_buffer(&mut socket, &response_buffer, &mut resp_len).await {
            break 'connection;
        }
        if close_connection {
            let _ = socket.shutdown().await;
            break 'connection;
        }

        // Emit interim `100 Continue` if the in-progress request requested it.
        if conn.take_expect_100_continue() && socket.write_all(HTTP_100_CONTINUE).await.is_err() {
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::pin::Pin;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::task::Context;
    use std::time::Duration;
    use tokio::io::{AsyncReadExt, DuplexStream, ReadBuf};

    struct CountingStream {
        inner: DuplexStream,
        write_calls: Arc<AtomicUsize>,
    }

    impl AsyncRead for CountingStream {
        fn poll_read(
            mut self: Pin<&mut Self>,
            cx: &mut Context<'_>,
            buf: &mut ReadBuf<'_>,
        ) -> Poll<std::io::Result<()>> {
            Pin::new(&mut self.inner).poll_read(cx, buf)
        }
    }

    impl AsyncWrite for CountingStream {
        fn poll_write(
            mut self: Pin<&mut Self>,
            cx: &mut Context<'_>,
            buf: &[u8],
        ) -> Poll<std::io::Result<usize>> {
            let res = Pin::new(&mut self.inner).poll_write(cx, buf);
            if matches!(res, Poll::Ready(Ok(n)) if n > 0) {
                self.write_calls.fetch_add(1, Ordering::SeqCst);
            }
            res
        }

        fn poll_flush(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<std::io::Result<()>> {
            Pin::new(&mut self.inner).poll_flush(cx)
        }

        fn poll_shutdown(
            mut self: Pin<&mut Self>,
            cx: &mut Context<'_>,
        ) -> Poll<std::io::Result<()>> {
            Pin::new(&mut self.inner).poll_shutdown(cx)
        }
    }

    async fn run_pipelined_with_write_count(
        kernel: Arc<Kernel>,
        wire_bytes: &[u8],
    ) -> (usize, String) {
        let (mut client, server_io) = tokio::io::duplex(64 * 1024);
        client.write_all(wire_bytes).await.unwrap();
        client.shutdown().await.unwrap();

        let write_calls = Arc::new(AtomicUsize::new(0));
        let counting = CountingStream {
            inner: server_io,
            write_calls: Arc::clone(&write_calls),
        };
        let task = tokio::spawn(serve_connection(counting, kernel));

        let mut received = Vec::new();
        client.read_to_end(&mut received).await.unwrap();
        task.await.unwrap();

        (
            write_calls.load(Ordering::SeqCst),
            String::from_utf8(received).unwrap(),
        )
    }

    #[tokio::test]
    async fn hybrid_write_strategy_coalesces_bursts_and_flushes_on_pending_or_cap() {
        let mut router = Router::new();
        router.get("/fast", |_req| async { Response::ok().text("ok") });
        router.get("/medium", |_req| async {
            Response::ok().text("x".repeat(512))
        });
        router.get("/slow", |_req| async {
            tokio::time::sleep(Duration::from_millis(50)).await;
            Response::ok().text("slow")
        });
        let kernel = Arc::new(Kernel::new(Arc::new(router)));

        // 1. Burst of 16 fast pipelined requests (< RESPONSE_BUFFER_SIZE) -> 1 write call.
        let burst = "GET /fast HTTP/1.1\r\nHost: localhost\r\n\r\n".repeat(16);
        let (writes, text) =
            run_pipelined_with_write_count(Arc::clone(&kernel), burst.as_bytes()).await;
        assert_eq!(text.matches("HTTP/1.1 200 OK\r\n").count(), 16);
        assert_eq!(
            writes, 1,
            "16 fast pipelined responses should coalesce into a single write call"
        );

        // 2. [fast, fast, slow, fast] -> 2 write calls: [fast, fast] flushed on slow's
        //    Poll::Pending, then [slow, fast] coalesced together.
        let mixed = b"GET /fast HTTP/1.1\r\nHost: localhost\r\n\r\n\
                      GET /fast HTTP/1.1\r\nHost: localhost\r\n\r\n\
                      GET /slow HTTP/1.1\r\nHost: localhost\r\n\r\n\
                      GET /fast HTTP/1.1\r\nHost: localhost\r\n\r\n";
        let (writes, text) = run_pipelined_with_write_count(Arc::clone(&kernel), mixed).await;
        assert_eq!(text.matches("HTTP/1.1 200 OK\r\n").count(), 4);
        assert_eq!(
            writes, 2,
            "expected 2 writes: [fast, fast] flushed on slow Pending, then [slow, fast]"
        );

        // 3. Burst exceeding RESPONSE_BUFFER_SIZE triggers intermediate high-water flushes
        //    while still coalescing within each chunk.
        let medium_burst = "GET /medium HTTP/1.1\r\nHost: localhost\r\n\r\n".repeat(16);
        let (writes, text) = run_pipelined_with_write_count(kernel, medium_burst.as_bytes()).await;
        assert_eq!(text.matches("HTTP/1.1 200 OK\r\n").count(), 16);
        assert!(
            (2..16).contains(&writes),
            "expected batched high-water flushes in 2..16 writes, got {writes}"
        );
    }
}
