use ruvel::foundation::Application;
use ruvel::http::{Method, Middleware, Next, Request, Response};
use std::future::Future;
use std::pin::Pin;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

struct HeaderMiddleware {
    name: &'static str,
    val: &'static str,
}

impl Middleware for HeaderMiddleware {
    fn handle<'a>(
        &'a self,
        request: Request,
        next: Next<'a>,
    ) -> Pin<Box<dyn Future<Output = Response> + Send + 'a>> {
        Box::pin(async move {
            let mut res = next.run(request).await;
            res = res.header(self.name, self.val);
            res
        })
    }
}

struct CounterMiddleware {
    counter: Arc<AtomicUsize>,
}

impl Middleware for CounterMiddleware {
    fn handle<'a>(
        &'a self,
        request: Request,
        next: Next<'a>,
    ) -> Pin<Box<dyn Future<Output = Response> + Send + 'a>> {
        Box::pin(async move {
            self.counter.fetch_add(1, Ordering::SeqCst);
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
                    name: "X-Auth",
                    val: "passed",
                },
            );
        })
        .with_routing(|router| {
            router
                .get("/dashboard", |_req| async { "welcome" })
                .middleware("auth");

            router.get("/public", |_req| async { "public-data" });
        })
        .create();

    // /dashboard should have X-Auth header
    let req = Request::new(Method::GET, "/dashboard");
    let res = app.router.dispatch(req).await;
    assert_eq!(res.headers.get("X-Auth"), Some("passed"));
    assert_eq!(res.body, b"welcome");

    // /public should NOT have X-Auth header
    let req = Request::new(Method::GET, "/public");
    let res = app.router.dispatch(req).await;
    assert_eq!(res.headers.get("X-Auth"), None);
    assert_eq!(res.body, b"public-data");
}

#[tokio::test]
async fn test_route_level_middleware_with_direct_instance() {
    let direct_mw = Arc::new(HeaderMiddleware {
        name: "X-Direct",
        val: "yes",
    });

    let app = Application::configure("")
        .with_routing(|router| {
            router
                .get("/direct", |_req| async { "ok" })
                .middleware(direct_mw);
        })
        .create();

    let req = Request::new(Method::GET, "/direct");
    let res = app.router.dispatch(req).await;
    assert_eq!(res.headers.get("X-Direct"), Some("yes"));
}

#[tokio::test]
async fn test_route_level_middleware_slice_and_chaining() {
    let app = Application::configure("")
        .with_middleware(|m| {
            m.alias(
                "m1",
                HeaderMiddleware {
                    name: "X-M1",
                    val: "1",
                },
            );
            m.alias(
                "m2",
                HeaderMiddleware {
                    name: "X-M2",
                    val: "2",
                },
            );
            m.alias(
                "m3",
                HeaderMiddleware {
                    name: "X-M3",
                    val: "3",
                },
            );
        })
        .with_routing(|router| {
            router
                .post("/submit", |_req| async { "done" })
                .middleware(&["m1", "m2"])
                .middleware("m3");
        })
        .create();

    let req = Request::new(Method::POST, "/submit");
    let res = app.router.dispatch(req).await;
    assert_eq!(res.headers.get("X-M1"), Some("1"));
    assert_eq!(res.headers.get("X-M2"), Some("2"));
    assert_eq!(res.headers.get("X-M3"), Some("3"));
}

#[tokio::test]
async fn test_route_naming_and_lookup() {
    let app = Application::configure("")
        .with_routing(|router| {
            router
                .get("/users/profile", |_req| async { "profile" })
                .name("users.profile");
        })
        .create();

    assert_eq!(app.router.route("users.profile"), Some("/users/profile"));
    assert_eq!(app.router.route("unknown"), None);
}

#[tokio::test]
async fn test_route_prefix_and_group_prefix() {
    let app = Application::configure("")
        .with_routing(|router| {
            // Group prefix
            router.prefix("/api/v1").group(|router| {
                // Route with its own prefix addition
                router
                    .get("/users", |_req| async { "users-list" })
                    .prefix("/admin");

                router.get("/posts", |_req| async { "posts-list" });
            });
        })
        .create();

    let req = Request::new(Method::GET, "/api/v1/admin/users");
    let res = app.router.dispatch(req).await;
    assert_eq!(res.body, b"users-list");

    let req = Request::new(Method::GET, "/api/v1/posts");
    let res = app.router.dispatch(req).await;
    assert_eq!(res.body, b"posts-list");
}

#[tokio::test]
async fn test_without_middleware() {
    let app = Application::configure("")
        .with_middleware(|m| {
            m.alias(
                "auth",
                HeaderMiddleware {
                    name: "X-Auth",
                    val: "checked",
                },
            );
        })
        .with_routing(|router| {
            router.middleware("auth").group(|router| {
                router.get("/protected", |_req| async { "protected" });

                router
                    .get("/bypass", |_req| async { "bypassed" })
                    .without_middleware("auth");
            });
        })
        .create();

    let req = Request::new(Method::GET, "/protected");
    let res = app.router.dispatch(req).await;
    assert_eq!(res.headers.get("X-Auth"), Some("checked"));

    let req = Request::new(Method::GET, "/bypass");
    let res = app.router.dispatch(req).await;
    assert_eq!(res.headers.get("X-Auth"), None);
}

#[tokio::test]
async fn test_any_method() {
    let app = Application::configure("")
        .with_routing(|router| {
            router.any("/webhook", |_req| async { "handled" });
        })
        .create();

    for method in [
        Method::GET,
        Method::POST,
        Method::PUT,
        Method::DELETE,
        Method::PATCH,
    ] {
        let req = Request::new(method, "/webhook");
        let res = app.router.dispatch(req).await;
        assert_eq!(res.body, b"handled");
    }
}

#[tokio::test]
async fn test_fluent_group_and_counter_middleware() {
    let counter = Arc::new(AtomicUsize::new(0));
    let count_mw = Arc::new(CounterMiddleware {
        counter: Arc::clone(&counter),
    });

    let app = Application::configure("")
        .with_routing(|router| {
            router
                .prefix("/counter")
                .middleware(count_mw)
                .group(|router| {
                    router.get("/inc", |_req| async { "incremented" });
                });
        })
        .create();

    let req = Request::new(Method::GET, "/counter/inc");
    let res = app.router.dispatch(req).await;
    assert_eq!(res.body, b"incremented");
    assert_eq!(counter.load(Ordering::SeqCst), 1);
}
