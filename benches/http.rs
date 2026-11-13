//! HTTP request parsing, response serialization, and end-to-end dispatch benchmarks.
//!
//! Run: `cargo bench --bench http`

use criterion::{Criterion, black_box, criterion_group, criterion_main};
use ruvel::http::{Kernel, Request, Response};
use ruvel::routing::Router;
use std::future::Future;
use std::sync::Arc;
use std::task::{Context, Poll, Waker};

const GET_10_HEADERS: &[u8] = b"\
GET /api/v1/users/42?page=2&limit=20 HTTP/1.1\r\n\
Host: api.example.com\r\n\
User-Agent: Mozilla/5.0 (X11; Linux x86_64) Chrome/120.0.0.0\r\n\
Accept: application/json, text/plain, */*\r\n\
Accept-Encoding: gzip, deflate, br\r\n\
Accept-Language: en-US,en;q=0.9\r\n\
Connection: keep-alive\r\n\
Cache-Control: no-cache\r\n\
Authorization: Bearer eyJhbGciOiJIUzI1NiIsInR5cCI6IkpXVCJ9\r\n\
Cookie: session_id=abc123def456; theme=dark\r\n\
X-Request-Id: 550e8400-e29b-41d4-a716-446655440000\r\n\
\r\n";

const GET_ROOT_MINIMAL: &[u8] = b"\
GET / HTTP/1.1\r\n\
Host: localhost:7883\r\n\
User-Agent: wrk\r\n\
Accept: */*\r\n\
\r\n";

const POST_JSON: &[u8] = b"\
POST /api/v1/users HTTP/1.1\r\n\
Host: api.example.com\r\n\
User-Agent: curl/8.5.0\r\n\
Accept: application/json\r\n\
Content-Type: application/json\r\n\
Content-Length: 43\r\n\
Connection: keep-alive\r\n\
\r\n\
{\"name\":\"Alice\",\"email\":\"alice@example.com\"}";

fn poll_ready<F: Future<Output = Response>>(fut: F) -> Response {
    let mut fut = std::pin::pin!(fut);
    let mut cx = Context::from_waker(Waker::noop());
    match fut.as_mut().poll(&mut cx) {
        Poll::Ready(res) => res,
        Poll::Pending => panic!("benchmark handler unexpectedly returned Pending"),
    }
}

fn bench_http(c: &mut Criterion) {
    let mut parse_group = c.benchmark_group("request_parse");
    parse_group.bench_function("get_10_headers", |b| {
        b.iter(|| Request::parse(black_box(GET_10_HEADERS)).unwrap().unwrap())
    });
    parse_group.bench_function("get_root_minimal", |b| {
        b.iter(|| {
            Request::parse(black_box(GET_ROOT_MINIMAL))
                .unwrap()
                .unwrap()
        })
    });
    parse_group.bench_function("post_json", |b| {
        b.iter(|| Request::parse(black_box(POST_JSON)).unwrap().unwrap())
    });

    let mut pipelined = Vec::with_capacity(GET_10_HEADERS.len() * 8);
    for _ in 0..8 {
        pipelined.extend_from_slice(GET_10_HEADERS);
    }
    parse_group.bench_function("pipelined_8_requests", |b| {
        b.iter(|| {
            let buf = black_box(pipelined.as_slice());
            let mut offset = 0;
            let mut count = 0usize;
            while offset < buf.len() {
                let (req, consumed) = Request::parse(&buf[offset..]).unwrap().unwrap();
                black_box(req);
                offset += consumed;
                count += 1;
            }
            count
        })
    });
    parse_group.finish();

    let mut write_group = c.benchmark_group("response_write");
    let mut out = Vec::with_capacity(4096);
    write_group.bench_function("ok_text", |b| {
        b.iter(|| {
            out.clear();
            let res = Response::ok().text(black_box("Hello, Ruvel!"));
            res.write_to(&mut out);
            out.len()
        })
    });
    write_group.bench_function("ok_json", |b| {
        b.iter(|| {
            out.clear();
            let res = Response::ok().json(black_box(r#"{"status":"ok","id":42}"#));
            res.write_to(&mut out);
            out.len()
        })
    });
    write_group.finish();

    let mut router = Router::new();
    router.get("/", |_req| async { "ok" });
    router.get("/api/v1/users/{id}", |req| async move {
        let id = req.param("id").unwrap_or("0");
        Response::ok().text(id)
    });
    let kernel = Kernel::new(Arc::new(router));

    let mut roundtrip_group = c.benchmark_group("http_roundtrip");
    roundtrip_group.bench_function("static_root_minimal", |b| {
        b.iter(|| {
            out.clear();
            let (req, _) = Request::parse(black_box(GET_ROOT_MINIMAL))
                .unwrap()
                .unwrap();
            let res = poll_ready(kernel.handle(req));
            res.write_to(&mut out);
            out.len()
        })
    });
    roundtrip_group.bench_function("param_10_headers", |b| {
        b.iter(|| {
            out.clear();
            let (req, _) = Request::parse(black_box(GET_10_HEADERS)).unwrap().unwrap();
            let res = poll_ready(kernel.handle(req));
            res.write_to(&mut out);
            out.len()
        })
    });
    roundtrip_group.finish();
}

criterion_group!(benches, bench_http);
criterion_main!(benches);
