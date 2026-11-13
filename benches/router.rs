//! Router dispatch micro-benchmarks.
//!
//! Handlers complete immediately, so each future is polled once with a no-op waker.
//! This measures routing + middleware + handler overhead without async runtime noise.
//!
//! Run: `cargo bench --bench router`

use criterion::{Criterion, black_box, criterion_group, criterion_main};
use ruvel::http::middleware::{BoxFuture, Middleware, Next};
use ruvel::http::{Kernel, Method, Request, Response};
use ruvel::routing::Router;
use std::future::Future;
use std::sync::Arc;
use std::task::{Context, Poll, Waker};

struct PassThrough;

impl Middleware for PassThrough {
    fn handle<'a>(&'a self, request: Request, next: Next) -> BoxFuture<'a, Response> {
        Box::pin(async move { next.run(request).await })
    }
}

fn poll_ready<F: Future<Output = Response>>(fut: F) -> Response {
    let mut fut = std::pin::pin!(fut);
    let mut cx = Context::from_waker(Waker::noop());
    match fut.as_mut().poll(&mut cx) {
        Poll::Ready(res) => res,
        Poll::Pending => panic!("benchmark handler unexpectedly returned Pending"),
    }
}

/// ~50 routes: static, single-param, nested-param, and middleware-wrapped.
fn build_router() -> Router {
    let mut router = Router::new();

    for i in 0..20 {
        let path = format!("/static/page{i}");
        router.get(&path, |_req| async { "static" });
    }
    for resource in ["users", "posts", "comments", "tags", "teams"] {
        router.get(&format!("/api/v1/{resource}"), |_req| async { "index" });
        router.get(&format!("/api/v1/{resource}/{{id}}"), |_req| async {
            "show"
        });
        router.post(&format!("/api/v1/{resource}"), |_req| async { "store" });
        router.get(
            &format!("/api/v1/{resource}/{{id}}/children/{{child}}"),
            |_req| async { "nested" },
        );
    }
    router.get("/api/v1/health", |_req| async { "ok" });

    let mw: Vec<Arc<dyn Middleware>> = vec![Arc::new(PassThrough), Arc::new(PassThrough)];
    router
        .middleware(mw)
        .get("/mw/users/{id}", |_req| async { "mw" });

    router
}

fn bench_router(c: &mut Criterion) {
    let router = build_router();

    let mut group = c.benchmark_group("router_dispatch");
    group.bench_function("static_hit", |b| {
        b.iter(|| {
            poll_ready(router.dispatch(Request::new(Method::GET, black_box("/api/v1/health"))))
        })
    });
    group.bench_function("param_hit", |b| {
        b.iter(|| {
            poll_ready(router.dispatch(Request::new(Method::GET, black_box("/api/v1/teams/42"))))
        })
    });
    group.bench_function("nested_param_hit", |b| {
        b.iter(|| {
            poll_ready(router.dispatch(Request::new(
                Method::GET,
                black_box("/api/v1/teams/42/children/7"),
            )))
        })
    });
    group.bench_function("param_hit_2_middleware", |b| {
        b.iter(|| poll_ready(router.dispatch(Request::new(Method::GET, black_box("/mw/users/42")))))
    });
    group.bench_function("not_found", |b| {
        b.iter(|| {
            poll_ready(router.dispatch(Request::new(Method::GET, black_box("/nope/nothing"))))
        })
    });
    group.finish();

    let kernel = Kernel::new(Arc::new(build_router()));
    c.bench_function("kernel_handle/param_hit", |b| {
        b.iter(|| {
            poll_ready(kernel.handle(Request::new(Method::GET, black_box("/api/v1/teams/42"))))
        })
    });
}

criterion_group!(benches, bench_router);
criterion_main!(benches);
