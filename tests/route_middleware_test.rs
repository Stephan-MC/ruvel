use ruvel::foundation::Application;
use ruvel::http::{Kernel, Method, Middleware, Next, Request, Response};
use std::future::Future;
use std::pin::Pin;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

struct HeaderMiddleware {
    key: &'static str,
    val: &'static str,
}

impl Middleware for HeaderMiddleware {
    fn handle<'a>(
        &'a self,
        request: Request,
        next: Next,
    ) -> Pin<Box<dyn Future<Output = Response> + Send + 'a>> {
        Box::pin(async move {
            let res = next.run(request).await;
            res.header(self.key, self.val)
        })
    }
}

struct CounterMiddleware {
    count: Arc<AtomicUsize>,
}

impl Middleware for CounterMiddleware {
    fn handle<'a>(
        &'a self,
        request: Request,
        next: Next,
    ) -> Pin<Box<dyn Future<Output = Response> + Send + 'a>> {
        Box::pin(async move {
            self.count.fetch_add(1, Ordering::SeqCst);
            next.run(request).await
        })
    }
}

#[tokio::test]
async fn test_route_level_middleware_with_alias() {
    let app = Application::configure("")
        .with_middleware(|m| {
            m.alias(
                "auth",
                HeaderMiddleware {
                    key: "X-Auth",
                    val: "passed",
                },
            );
        })
        .with_routing(|router| {
            router
                .get("/dashboard", |_req| async { "secret data" })
                .middleware("auth");
        })
        .create();

    let kernel = app.make::<ruvel::http::Kernel>().unwrap();

    let req = Request::new(Method::GET, "/dashboard");
    let res = kernel.handle(req).await;

    assert_eq!(res.body, b"secret data");
    assert_eq!(res.headers.get("X-Auth"), Some("passed"));
}

#[tokio::test]
async fn test_route_level_middleware_with_direct_instance() {
    let app = Application::configure("")
        .with_routing(|router| {
            router
                .get("/profile", |_req| async { "profile data" })
                .middleware(Arc::new(HeaderMiddleware {
                    key: "X-Profile",
                    val: "loaded",
                }));
        })
        .create();

    let kernel = app.make::<ruvel::http::Kernel>().unwrap();

    let req = Request::new(Method::GET, "/profile");
    let res = kernel.handle(req).await;

    assert_eq!(res.body, b"profile data");
    assert_eq!(res.headers.get("X-Profile"), Some("loaded"));
}

#[tokio::test]
async fn test_route_level_middleware_slice_and_chaining() {
    let app = Application::configure("")
        .with_middleware(|m| {
            m.alias(
                "first",
                HeaderMiddleware {
                    key: "X-First",
                    val: "1",
                },
            );
            m.alias(
                "second",
                HeaderMiddleware {
                    key: "X-Second",
                    val: "2",
                },
            );
            m.alias(
                "third",
                HeaderMiddleware {
                    key: "X-Third",
                    val: "3",
                },
            );
        })
        .with_routing(|router| {
            router
                .get("/chained", |_req| async { "chain" })
                .middleware(["first", "second"])
                .middleware("third");
        })
        .create();

    let kernel = app.make::<ruvel::http::Kernel>().unwrap();

    let req = Request::new(Method::GET, "/chained");
    let res = kernel.handle(req).await;

    assert_eq!(res.body, b"chain");
    assert_eq!(res.headers.get("X-First"), Some("1"));
    assert_eq!(res.headers.get("X-Second"), Some("2"));
    assert_eq!(res.headers.get("X-Third"), Some("3"));
}

#[tokio::test]
async fn test_without_middleware() {
    let app = Application::configure("")
        .with_middleware(|m| {
            m.alias(
                "auth",
                HeaderMiddleware {
                    key: "X-Auth",
                    val: "passed",
                },
            );
            m.alias(
                "log",
                HeaderMiddleware {
                    key: "X-Log",
                    val: "logged",
                },
            );
        })
        .with_routing(|router| {
            router
                .middleware(["auth", "log"])
                .group(|router| {
                    router
                        .get("/excluded", |_req| async { "excluded" })
                        .without_middleware("log");
                });
        })
        .create();

    let kernel = app.make::<ruvel::http::Kernel>().unwrap();

    let req = Request::new(Method::GET, "/excluded");
    let res = kernel.handle(req).await;

    assert_eq!(res.body, b"excluded");
    assert_eq!(res.headers.get("X-Auth"), Some("passed"));
    assert_eq!(res.headers.get("X-Log"), None);
}

#[tokio::test]
async fn test_fluent_group_and_counter_middleware() {
    let counter = Arc::new(AtomicUsize::new(0));

    let app = Application::configure("")
        .with_routing({
            let counter = Arc::clone(&counter);
            move |router| {
                router
                    .middleware(Arc::new(CounterMiddleware {
                        count: Arc::clone(&counter),
                    }))
                    .prefix("api")
                    .group(|router| {
                        router.get("/counter", |_req| async { "ok" });
                    });
            }
        })
        .create();

    let kernel = app.make::<ruvel::http::Kernel>().unwrap();

    let req = Request::new(Method::GET, "/api/counter");
    let res = kernel.handle(req).await;

    assert_eq!(res.body, b"ok");
    assert_eq!(counter.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn test_route_prefix_and_group_prefix() {
    let app = Application::configure("")
        .with_routing(|router| {
            router.prefix("v1").group(|router| {
                router.get("/users", |_req| async { "users list" });
            });
        })
        .create();

    let kernel = app.make::<ruvel::http::Kernel>().unwrap();

    let req = Request::new(Method::GET, "/v1/users");
    let res = kernel.handle(req).await;

    assert_eq!(res.body, b"users list");
}

#[tokio::test]
async fn test_route_naming() {
    let app = Application::configure("")
        .with_routing(|router| {
            router
                .get("/named", |_req| async { "named route" })
                .name("test.named");
        })
        .create();

    let kernel = app.make::<ruvel::http::Kernel>().unwrap();

    let req = Request::new(Method::GET, "/named");
    let res = kernel.handle(req).await;

    assert_eq!(res.body, b"named route");
}

#[tokio::test]
async fn test_any_method() {
    let app = Application::configure("")
        .with_routing(|router| {
            router.any("/any", |_req| async { "any response" });
        })
        .create();

    let kernel = app.make::<ruvel::http::Kernel>().unwrap();

    for method in &[Method::GET, Method::POST, Method::PUT, Method::DELETE, Method::PATCH] {
        let req = Request::new(method.clone(), "/any");
        let res = kernel.handle(req).await;
        assert_eq!(res.body, b"any response");
    }
}
