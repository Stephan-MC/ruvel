use ruvel::configuration::ApplicationBuilder;
use ruvel::configuration::Middleware as ConfigMiddleware;
use ruvel::foundation::Application;
use ruvel::http::{BoxFuture, Method, Middleware, Next, Request, Response};
use std::sync::Arc;
use std::time::Instant;

struct DummyMiddleware(usize);

impl Middleware for DummyMiddleware {
    fn handle<'a>(&'a self, request: Request, next: Next<'a>) -> BoxFuture<'a, Response> {
        Box::pin(async move { next.run(request).await })
    }
}

macro_rules! define_middleware {
    ($name:ident) => {
        struct $name;
        impl Middleware for $name {
            fn handle<'a>(&'a self, request: Request, next: Next<'a>) -> BoxFuture<'a, Response> {
                Box::pin(async move { next.run(request).await })
            }
        }
    };
}

define_middleware!(M1);
define_middleware!(M2);
define_middleware!(M3);
define_middleware!(M4);
define_middleware!(M5);
define_middleware!(M6);
define_middleware!(M7);
define_middleware!(M8);
define_middleware!(M9);
define_middleware!(M10);
define_middleware!(Replacer);

#[tokio::main]
async fn main() {
    println!("============================================================");
    println!("      Ruvel Framework Middleware Performance Profiling      ");
    println!("============================================================\n");

    profile_prepend_vs_append();
    profile_with_middleware_resolution();
    profile_route_registration();
    profile_dispatch_throughput().await;
}

fn profile_prepend_vs_append() {
    println!("--- 1. Configuration: Append vs Prepend Scaling (O(1) vs O(N)) ---");
    let counts = [100, 500, 2000, 5000];

    for &count in &counts {
        let start = Instant::now();
        let mut mw = ConfigMiddleware::new();
        for i in 0..count {
            mw.append(DummyMiddleware(i));
        }
        let append_dur = start.elapsed();

        let start = Instant::now();
        let mut mw = ConfigMiddleware::new();
        for i in 0..count {
            mw.prepend(DummyMiddleware(i));
        }
        let prepend_dur = start.elapsed();

        println!(
            "Count: {:5} | append (Vec::push): {:10.2?} | prepend (Vec::insert(0)): {:10.2?} (slowdown: {:.1}x)",
            count,
            append_dur,
            prepend_dur,
            prepend_dur.as_nanos() as f64 / append_dur.as_nanos().max(1) as f64
        );
    }
    println!();
}

fn profile_with_middleware_resolution() {
    println!("--- 2. Bootstrapping: ApplicationBuilder::with_middleware & resolve() ---");
    let iterations = 20_000;

    let start = Instant::now();
    for _ in 0..iterations {
        let app = Application::new("/");
        let _builder = ApplicationBuilder::new(app).with_middleware(|mw| {
            mw.append(M1)
                .append(M2)
                .append(M3)
                .append(M4)
                .append(M5)
                .append(M6)
                .append(M7)
                .append(M8)
                .append(M9)
                .append(M10)
                .remove::<M3>()
                .remove::<M7>()
                .replace::<M5, Replacer>(Replacer)
                .group("web", vec![Arc::new(M1), Arc::new(M2)])
                .alias("auth", M3);
        });
    }
    let dur = start.elapsed();
    println!(
        "{} with_middleware() boot resolutions (10 layers, 2 removals, 1 replacement, 1 group, 1 alias):\n  Total: {:?} | Average: {:.2} µs/call",
        iterations,
        dur,
        (dur.as_nanos() as f64 / iterations as f64) / 1000.0
    );
    println!();
}

fn profile_route_registration() {
    println!("--- 3. Route Compilation: Pipeline Compilation at Route Registration ---");
    let iterations = 10_000;
    let app = Application::new("/");
    let builder = ApplicationBuilder::new(app).with_middleware(|mw| {
        mw.append(M1).append(M2).append(M3);
        mw.group("web", vec![Arc::new(M4), Arc::new(M5)]);
        mw.alias("auth", M6);
    });
    let mut app = builder.create();

    let start = Instant::now();
    for i in 0..iterations {
        let path = format!("/route/{}", i);
        app.router.group(&["web", "auth"], |r| {
            r.get(&path, |_req| async { "ok" });
        });
    }
    let dur = start.elapsed();
    println!(
        "{} route registrations with middleware groups & aliases:\n  Total: {:?} | Average: {:.2} µs/route",
        iterations,
        dur,
        (dur.as_nanos() as f64 / iterations as f64) / 1000.0
    );
    println!();
}

async fn profile_dispatch_throughput() {
    println!("--- 4. Per-Request Dispatch Latency (Runtime: Fast-path vs 5 Layers vs 10 Layers) ---");

    // Scenario A: 0 middleware (Fast-path bypass)
    let app_0 = Application::new("/");
    let mut app_0 = ApplicationBuilder::new(app_0).create();
    app_0.router.get("/test", |_req| async { "ok" });

    // Scenario B: 5 middleware layers
    let app_5 = Application::new("/");
    let builder_5 = ApplicationBuilder::new(app_5).with_middleware(|mw| {
        mw.append(M1).append(M2).append(M3).append(M4).append(M5);
    });
    let mut app_5 = builder_5.create();
    app_5.router.get("/test", |_req| async { "ok" });

    // Scenario C: 10 middleware layers
    let app_10 = Application::new("/");
    let builder_10 = ApplicationBuilder::new(app_10).with_middleware(|mw| {
        mw.append(M1)
            .append(M2)
            .append(M3)
            .append(M4)
            .append(M5)
            .append(M6)
            .append(M7)
            .append(M8)
            .append(M9)
            .append(M10);
    });
    let mut app_10 = builder_10.create();
    app_10.router.get("/test", |_req| async { "ok" });

    let iterations = 100_000;

    // Benchmark 0 layers
    let start = Instant::now();
    for _ in 0..iterations {
        let req = Request::new(Method::GET, "/test".to_string());
        let _res = app_0.router.dispatch(req).await;
    }
    let dur_0 = start.elapsed();

    // Benchmark 5 layers
    let start = Instant::now();
    for _ in 0..iterations {
        let req = Request::new(Method::GET, "/test".to_string());
        let _res = app_5.router.dispatch(req).await;
    }
    let dur_5 = start.elapsed();

    // Benchmark 10 layers
    let start = Instant::now();
    for _ in 0..iterations {
        let req = Request::new(Method::GET, "/test".to_string());
        let _res = app_10.router.dispatch(req).await;
    }
    let dur_10 = start.elapsed();

    println!(
        "0 layers  (Fast-path): {:10.2?} | {:.2} ns/req | ~{:.0} req/sec",
        dur_0,
        dur_0.as_nanos() as f64 / iterations as f64,
        iterations as f64 / dur_0.as_secs_f64()
    );
    println!(
        "5 layers  (BoxFuture): {:10.2?} | {:.2} ns/req | ~{:.0} req/sec",
        dur_5,
        dur_5.as_nanos() as f64 / iterations as f64,
        iterations as f64 / dur_5.as_secs_f64()
    );
    println!(
        "10 layers (BoxFuture): {:10.2?} | {:.2} ns/req | ~{:.0} req/sec",
        dur_10,
        dur_10.as_nanos() as f64 / iterations as f64,
        iterations as f64 / dur_10.as_secs_f64()
    );
    println!();
}
