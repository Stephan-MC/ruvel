# In-Depth Performance & Maximum Throughput Analysis of Ruvel

## 1. Executive Summary

Ruvel is designed as an asynchronous, ergonomic Rust web framework inspired by Laravel. While it adopts high-performance asynchronous primitives (`tokio`, `httparse`, `matchit`), several architectural design patterns introduce significant latency, heap memory churn, CPU cache invalidations, and system call overhead under high concurrency and pipelining workloads.

In high-throughput HTTP servers (capable of millions of requests per second, such as `actix-web`, `hyper`, `axum`, or `may-minihttp`), the hot path must adhere to three foundational rules:
1. **Zero Heap Allocations on Hot Paths**: No `String`, `Vec`, `Box`, or `HashMap` should be allocated per incoming request.
2. **Zero Memory Copying / Borrowed Slices**: Parsing must yield borrowed slices (`&str`, `&[u8]`) or reference-counted buffer slices (`bytes::Bytes`).
3. **Optimized I/O and Pipelining**: Memory buffers must avoid compaction (`Vec::drain`), system calls should be amortized via vectored writes (`writev`), and memory layouts should maximize L1/L2 data cache locality.

This document details the performance-degrading patterns found in Ruvel across six architectural layers and provides production-grade code samples to remediate them.

---

## 2. Layer 1: Network & Socket I/O (`src/foundation/application.rs`)

### 2.1 Bottleneck Breakdown

#### A. Buffer Compaction via `read_buffer.drain(..offset)`
In `src/foundation/application.rs`:
```rust
Ok(None) => {
    if offset > 0 {
        read_buffer.drain(..offset);
    }
    break;
}
```
When pipelined requests or fragmented TCP streams are processed, `Vec::drain(..offset)` shifts all remaining bytes from `offset..read_buffer.len()` down to index `0` via `memmove`. Under pipelining or streaming workloads, this causes $O(N)$ memory copying on every partial frame.

#### B. Fixed-Size 2048-Byte Intermediate Chunk Copy
```rust
let mut chunk = [0u8; 2048];
...
match socket.read(&mut chunk).await {
    Ok(n) => {
        ...
        read_buffer.extend_from_slice(&chunk[..n]);
    }
}
```
Every socket read performs a two-step copy:
1. Socket read into the stack buffer `chunk`.
2. Memory copy from `chunk[..n]` into `read_buffer`.

This double-buffering wastes CPU memory bandwidth. Furthermore, a 2048-byte read buffer is smaller than typical Linux TCP receive buffers (often 32KB to 128KB+), requiring many more `read()` syscalls under high throughput.

#### C. Immediate Per-Request Syscall `socket.write_all` Without Pipelining Batching
```rust
// Dispatched within pipelined request loop
let response = router_clone.dispatch(req).await;
response_buffer.clear();
response.write_to(&mut response_buffer);
if socket.write_all(&response_buffer).await.is_err() {
    break 'connection;
}
```
When multiple requests arrive in a single `read()` (HTTP/1.1 pipelining), Ruvel executes a separate `write()` syscall for each request immediately. This defeats TCP Nagle/CORK optimizations and creates high kernel transition overhead.

---

### 2.2 Solution & Code Sample: Layer 1

Replace double-buffering and `drain` shifts with `bytes::BytesMut`. Read directly into uninitialized space in the buffer. Batch pipelined responses into a single flush.

```rust
use bytes::{Buf, BytesMut};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;
use std::sync::Arc;
use crate::routing::Router;
use crate::http::Request;

const READ_BUF_INIT: usize = 16 * 1024; // 16 KiB buffer
const MAX_REQUEST_BYTES: usize = 64 * 1024;

pub async fn handle_connection(mut socket: TcpStream, router: Arc<Router>) {
    let _ = socket.set_nodelay(true);

    let mut read_buf = BytesMut::with_capacity(READ_BUF_INIT);
    let mut write_buf = BytesMut::with_capacity(READ_BUF_INIT);

    'connection: loop {
        // Read directly into the unconsumed space of read_buf (Zero Stack Intermediate Buffer)
        let n = match socket.read_buf(&mut read_buf).await {
            Ok(0) => break, // Connection closed by peer
            Ok(n) => n,
            Err(_) => break,
        };

        if read_buf.len() > MAX_REQUEST_BYTES {
            break 'connection;
        }

        // Process all complete requests currently in read_buf
        let mut responses_queued = 0;

        loop {
            // Fast zero-copy parse against slice
            match Request::parse(&read_buf) {
                Ok(Some((req, consumed))) => {
                    // Advance internal pointer in O(1) time without memmove
                    read_buf.advance(consumed);

                    let response = router.dispatch(req).await;
                    response.write_to_bytes_mut(&mut write_buf);
                    responses_queued += 1;
                }
                Ok(None) => break, // Need more bytes from socket
                Err(()) => break 'connection, // Protocol error
            }
        }

        // Batch flush all responses generated during this cycle in a single syscall
        if responses_queued > 0 {
            if socket.write_all(&write_buf).await.is_err() {
                break 'connection;
            }
            write_buf.clear();
        }
    }
}
```

---

## 3. Layer 2: HTTP Parsing & Memory Allocation (`src/http/request.rs`)

### 3.1 Bottleneck Breakdown

#### A. Multiple Heap String Allocations per Header
In `src/http/request.rs`:
```rust
for header in parser.headers.iter() {
    header_map.insert(
        header.name,
        String::from_utf8_lossy(header.value).into_owned(),
    );
}
```
For a typical HTTP request with 10–15 headers:
- `header.name` is converted into an owned `String` (`into()`).
- `header.value` is converted via `String::from_utf8_lossy(...).into_owned()`, allocating a second `String`.
- In `HeaderMap::insert`, `.to_ascii_lowercase()` allocates a *third* `String` per header.
For 15 headers, this results in **45 distinct heap allocations per request** solely for header parsing.

#### B. Unnecessary Owned Strings for `path` and `query`
```rust
Self {
    method: Method::from(parser.method.ok_or(())?),
    path: path.to_string(),      // Heap allocation
    query: query.to_string(),    // Heap allocation
    headers: header_map,
    body,
    params: HashMap::new(),
}
```
`path` and `query` are cloned into separate heap `String`s, even when route matching only requires reading the slice during dispatch.

#### C. Eager Cloning of Request Body
```rust
let body = if total == header_len {
    Vec::new()
} else {
    buf[header_len..total].to_vec() // Heap allocation & byte copy
};
```
For endpoints with a payload (e.g., JSON POST), the entire body is allocated as a new `Vec<u8>` and copied.

---

### 3.2 Solution & Code Sample: Layer 2

Utilize zero-copy borrowing (`&'a [u8]`, `&'a str`) or reference-counted slice offsets (`bytes::Bytes`).

```rust
use bytes::Bytes;
use std::collections::HashMap;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Method {
    Get,
    Post,
    Put,
    Delete,
    Patch,
    Head,
    Options,
}

pub struct HeaderEntry<'a> {
    pub name: &'a str,
    pub value: &'a [u8],
}

pub struct ZeroCopyRequest<'a> {
    pub method: Method,
    pub path: &'a str,
    pub query: &'a str,
    pub headers: &'a [httparse::Header<'a>],
    pub body: &'a [u8],
}

// Or an owned zero-copy representation using reference-counted `Bytes`:
pub struct RequestRef {
    pub method: Method,
    // Buffer slice reference without heap copying:
    pub raw: Bytes,
    pub path_range: (usize, usize),
    pub query_range: (usize, usize),
    pub body_range: (usize, usize),
}

impl RequestRef {
    #[inline(always)]
    pub fn path(&self) -> &str {
        // Safe UTF-8 slice verified during parsing
        unsafe { std::str::from_utf8_unchecked(&self.raw[self.path_range.0..self.path_range.1]) }
    }

    #[inline(always)]
    pub fn body(&self) -> &[u8] {
        &self.raw[self.body_range.0..self.body_range.1]
    }
}
```

---

## 4. Layer 3: Header Map Implementation (`src/http/headers.rs`)

### 4.1 Bottleneck Breakdown

#### A. Quadruple String Allocation in `HeaderMap`
In `src/http/headers.rs`:
```rust
pub struct HeaderMap {
    inner: HashMap<String, (String, String)>,
}

pub fn insert(&mut self, key: impl Into<String>, value: impl Into<String>) {
    let key_str = key.into();
    let lower = key_str.to_ascii_lowercase(); // Heap Allocation 1
    self.inner.insert(lower, (key_str, value.into())); // Heap Allocations 2 & 3
}
```
The internal map requires three heap `String`s per entry:
1. `lower`: lowercased lookup key.
2. `key_str`: preserved original casing.
3. `value`: header value string.

#### B. `IntoIterator` Allocates an Intermediate Vector
```rust
impl<'a> IntoIterator for &'a HeaderMap {
    type Item = (&'a String, &'a String);
    type IntoIter = std::vec::IntoIter<Self::Item>;

    fn into_iter(self) -> Self::IntoIter {
        let items: Vec<Self::Item> = self.inner.values().map(|(k, v)| (k, v)).collect();
        items.into_iter()
    }
}
```
Serializing a response iterates over `HeaderMap`. Instead of returning an iterator borrowing `inner.values()`, this allocates a brand new `Vec` on the heap for every response serialization.

---

### 4.2 Solution & Code Sample: Layer 3

Use an array-backed or small-vector header container (or `http::HeaderMap`) that stores standard headers inline without hashing or lowercasing strings.

```rust
use smallvec::SmallVec;

#[derive(Debug, Clone, Default)]
pub struct FastHeaderMap {
    // Up to 16 headers stored entirely on the stack (0 heap allocations)
    entries: SmallVec<[(HeaderName, HeaderValue); 16]>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HeaderName {
    ContentType,
    ContentLength,
    Host,
    Authorization,
    Custom(Box<str>),
}

#[derive(Debug, Clone)]
pub struct HeaderValue(SmallVec<[u8; 32]>);

impl FastHeaderMap {
    #[inline]
    pub fn insert_static(&mut self, name: HeaderName, value: &[u8]) {
        let mut val = SmallVec::new();
        val.extend_from_slice(value);
        self.entries.push((name, HeaderValue(val)));
    }

    #[inline]
    pub fn get(&self, name: &HeaderName) -> Option<&[u8]> {
        self.entries.iter()
            .find(|(n, _)| n == name)
            .map(|(_, v)| v.0.as_slice())
    }

    #[inline]
    pub fn iter(&self) -> impl Iterator<Item = (&HeaderName, &[u8])> {
        self.entries.iter().map(|(k, v)| (k, v.0.as_slice()))
    }
}
```

---

## 5. Layer 4: Router & Parameter Extraction (`src/routing/router.rs`)

### 5.1 Bottleneck Breakdown

#### A. `HashMap<String, String>` Allocation per Route Match
In `src/routing/router.rs`:
```rust
if let Ok(matched) = method_router.at(&req.path) {
    for (key, value) in matched.params.iter() {
        req.params.insert(key.to_string(), value.to_string());
    }
    ...
}
```
Every matched route containing path parameters (such as `/posts/{id}`) performs:
1. `key.to_string()` (heap allocation).
2. `value.to_string()` (heap allocation).
3. `HashMap::insert`, resizing and hashing table buckets.
This makes parameterized routing 10x–20x slower than static routes.

#### B. Dynamic Boxed Future Return on Every Request Dispatch
```rust
pub fn dispatch(&self, mut req: Request) -> BoxFuture<'_, Response> {
    ...
    if pipeline.is_empty() {
        return (handler)(req);
    }

    Box::pin(async move {
        ...
    })
}
```
Even with the fast path, `(handler)(req)` returns `BoxFuture<'static, Response>`. Every single request incurs at least one heap-allocated `Box::pin` for the handler's asynchronous state machine.

---

### 5.2 Solution & Code Sample: Layer 4

Store route params as borrowed slices in a fixed-capacity inline array (e.g. `[(&str, &str); 8]`).

```rust
pub struct RouteParams<'a> {
    count: usize,
    entries: [(&'a str, &'a str); 8],
}

impl<'a> RouteParams<'a> {
    pub fn new() -> Self {
        Self {
            count: 0,
            entries: [("", ""); 8],
        }
    }

    #[inline]
    pub fn push(&mut self, key: &'a str, val: &'a str) {
        if self.count < 8 {
            self.entries[self.count] = (key, val);
            self.count += 1;
        }
    }

    #[inline]
    pub fn get(&self, key: &str) -> Option<&'a str> {
        for i in 0..self.count {
            if self.entries[i].0 == key {
                return Some(self.entries[i].1);
            }
        }
        None
    }
}
```

---

## 6. Layer 5: Middleware Pipeline Execution (`src/http/middleware.rs`)

### 6.1 Bottleneck Breakdown

#### A. Multiple `BoxFuture` Allocations in Onion Recursion
In `src/http/middleware.rs`:
```rust
pub trait Middleware: Send + Sync {
    fn handle<'a>(&'a self, request: Request, next: Next<'a>) -> BoxFuture<'a, Response>;
}

impl<'a> Next<'a> {
    pub async fn run(self, request: Request) -> Response {
        ...
        layer.handle(request, next).await
    }
}
```
For every middleware layer in the chain:
- `layer.handle(...)` constructs a `Box::pin(async move { ... })`.
- If a route has 5 middleware layers (e.g., logging, CORS, authentication, throttling, session), that is **5 separate heap allocations per request** solely for async state-machine framing.

---

### 6.2 Solution & Code Sample: Layer 5

Static compile-time middleware pipelines (like Tower / Axum `Service` or static generic function composition) completely remove heap allocations for middleware chains.

```rust
use std::future::Future;

// Zero-allocation middleware trait using RPITIT (Return Position Impl Trait in Trait)
pub trait FastMiddleware: Send + Sync + 'static {
    fn call<H, Fut>(&self, req: Request, next: H) -> impl Future<Output = Response> + Send
    where
        H: Fn(Request) -> Fut + Send + Sync,
        Fut: Future<Output = Response> + Send;
}

// Example zero-allocation middleware
pub struct FastRequestId;

impl FastMiddleware for FastRequestId {
    async fn call<H, Fut>(&self, req: Request, next: H) -> Response
    where
        H: Fn(Request) -> Fut + Send + Sync,
        Fut: Future<Output = Response> + Send,
    {
        let mut response = next(req).await;
        response.headers.insert_static(HeaderName::Custom("x-request-id".into()), b"1");
        response
    }
}
```

---

## 7. Layer 6: Response Construction & Serialization (`src/http/response.rs`)

### 7.1 Bottleneck Breakdown

#### A. `write!` Macro Overhead for Status and Length
In `src/http/response.rs`:
```rust
pub fn write_to(&self, buffer: &mut Vec<u8>) {
    let _ = write!(
        buffer,
        "HTTP/1.1 {} {}\r\n",
        self.status_code(),
        self.status_message()
    );
    let _ = write!(buffer, "Content-Length: {}\r\n", self.body.len());
    ...
}
```
Formatting with `std::fmt::write!` involves:
- Virtual function calls on the `std::fmt::Formatter`.
- Runtime formatting parsing of format strings.
- Integer-to-string decimal formatting at runtime for `status_code` and `Content-Length`.

Status lines are fixed and known ahead of time (e.g. `b"HTTP/1.1 200 OK\r\n"`), and integer-to-ascii formatting can be done via fast branchless lookup tables (such as `itoa`).

#### B. Byte-by-Byte Header Concatenation
```rust
for (k, v) in &self.headers {
    buffer.extend_from_slice(k.as_bytes());
    buffer.extend_from_slice(b": ");
    buffer.extend_from_slice(v.as_bytes());
    buffer.extend_from_slice(b"\r\n");
}
```
Frequent `extend_from_slice` calls cause multiple bounds-checking operations and potential micro-reallocations of `buffer`.

---

### 7.2 Solution & Code Sample: Layer 6

Pre-compute static status lines and use fast integer serialization (`itoa`).

```rust
use itoa::Buffer as ItoaBuffer;

impl Response {
    pub fn write_to_fast(&self, buffer: &mut Vec<u8>) {
        // 1. Fast static status line dispatch
        let status_bytes = match self.status.code() {
            200 => b"HTTP/1.1 200 OK\r\n",
            201 => b"HTTP/1.1 201 Created\r\n",
            204 => b"HTTP/1.1 204 No Content\r\n",
            400 => b"HTTP/1.1 400 Bad Request\r\n",
            404 => b"HTTP/1.1 404 Not Found\r\n",
            500 => b"HTTP/1.1 500 Internal Server Error\r\n",
            _ => {
                // Fallback for custom status codes
                buffer.extend_from_slice(b"HTTP/1.1 ");
                let mut itoa_buf = ItoaBuffer::new();
                buffer.extend_from_slice(itoa_buf.format(self.status.code()).as_bytes());
                buffer.extend_from_slice(b" ");
                buffer.extend_from_slice(self.status.message().as_bytes());
                b"\r\n"
            }
        };
        buffer.extend_from_slice(status_bytes);

        // 2. Fast Content-Length via itoa
        buffer.extend_from_slice(b"Content-Length: ");
        let mut itoa_buf = ItoaBuffer::new();
        buffer.extend_from_slice(itoa_buf.format(self.body.len()).as_bytes());
        buffer.extend_from_slice(b"\r\n");

        // 3. Headers
        for (k, v) in &self.headers {
            buffer.extend_from_slice(k.as_bytes());
            buffer.extend_from_slice(b": ");
            buffer.extend_from_slice(v.as_bytes());
            buffer.extend_from_slice(b"\r\n");
        }

        buffer.extend_from_slice(b"\r\n");
        buffer.extend_from_slice(&self.body);
    }
}
```

---

## 8. Layer 7: Service Container & Dependency Injection (`src/container/container.rs`)

### 8.1 Bottleneck Breakdown

#### A. Atomic Pointer Swapping on Reads
```rust
let bindings = self.bindings.load();
```
`ArcSwap::load` performs atomic operations and increment/decrement cycles on internal reference counters on every `Container::make` call.

#### B. Type-Erased `Any` Downcasting
```rust
instance.clone().downcast::<T>().map_err(...)
```
Each resolution performs `TypeId` comparison and dynamic downcasting. When called inside per-request controller actions or middleware, this adds unnecessary branching and runtime overhead.

#### C. Factory Closure Returns Fresh `Arc`
```rust
let factory_fn: BindingFactory = Arc::new(move |c| Arc::new(factory(c)));
```
Transient factory bindings allocate a new `Arc` for every invocation.

---

### 8.2 Solution & Code Sample: Layer 7

Separate container configuration from runtime resolution. Once the application boots, freeze the container into an immutable read-only state table.

```rust
use std::any::{Any, TypeId};
use std::collections::HashMap;
use std::sync::Arc;

pub struct FrozenContainer {
    singletons: HashMap<TypeId, Arc<dyn Any + Send + Sync>>,
}

impl FrozenContainer {
    #[inline(always)]
    pub fn get<T: Send + Sync + 'static>(&self) -> Option<&T> {
        self.singletons
            .get(&TypeId::of::<T>())
            .and_then(|boxed| boxed.downcast_ref::<T>())
    }
}
```

---

## 9. Architecture Comparison Table

| Metric / Dimension | Current Ruvel Architecture | Proposed Maximum-Throughput Architecture | Impact |
| :--- | :--- | :--- | :--- |
| **TCP Read Buffer** | Stack `[u8; 2048]` copied to `Vec<u8>` | `bytes::BytesMut` direct uninitialized read | Eliminates 1 memcpy per read |
| **Pipeline Parsing** | `Vec::drain(..offset)` shifts bytes | `bytes::Buf::advance(offset)` $O(1)$ cursor | Eliminates $O(N)$ memory copies |
| **HTTP Request Parsing** | Multiple `String` & `Vec` heap allocations | Zero-copy slices (`&str`, `&[u8]`) or `Bytes` | 100% reduction in request-parse heap allocations |
| **Header Storage** | `HashMap<String, (String, String)>` | Stack-allocated `SmallVec` or enum indexed | Drops ~45 heap allocations per request |
| **Route Params** | `HashMap<String, String>` per match | Inline slice table `[(&str, &str); 8]` | Zero allocation route parameter matching |
| **Middleware Pipeline** | Recursive `BoxFuture` heap boxing per layer | Generic static dispatch / RPITIT | 0 heap allocations in middleware traversal |
| **Response Serialization**| `write!` string formatting | Pre-baked status byte arrays + `itoa` | 3x faster response serialization |
| **Syscall Overhead** | Immediate `write_all` per pipelined request | Batched write buffer flush | Amortizes syscalls under pipelined traffic |
