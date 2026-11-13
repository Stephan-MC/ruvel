use ruvel::foundation::Application;
use ruvel::http::{Method, Middleware, Next, Request, Response};
use ruvel::routing::RouteRegistrar;
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

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
        .with_routing(|routing| {
            routing.web(|router| {
                router
                    .get("/dashboard", |_req| async { "secret data" })
                    .middleware("auth");
            });
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
        .with_routing(|routing| {
            routing.web(|router| {
                router
                    .get("/profile", |_req| async { "profile data" })
                    .middleware(Arc::new(HeaderMiddleware {
                        key: "X-Profile",
                        val: "loaded",
                    }));
            });
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
        .with_routing(|routing| {
            routing.web(|router| {
                router
                    .get("/chained", |_req| async { "chain" })
                    .middleware(["first", "second"])
                    .middleware("third");
            });
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
        .with_routing(|routing| {
            routing.web(|router| {
                router
                    .get("/excluded", |_req| async { "excluded" })
                    .middleware(["auth", "log"])
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
            move |routing| {
                routing.web({
                    let counter = Arc::clone(&counter);
                    move |router| {
                        router.prefix("api").group(|router| {
                            router
                                .get("/counter", |_req| async { "ok" })
                                .middleware(Arc::new(CounterMiddleware {
                                    count: Arc::clone(&counter),
                                }));
                        });
                    }
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
        .with_routing(|routing| {
            routing.web(|router| {
                router.prefix("v1").group(|router| {
                    router.get("/users", |_req| async { "users list" });
                });
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
        .with_routing(|routing| {
            routing.web(|router| {
                router
                    .get("/named", |_req| async { "named route" })
                    .name("test.named");
            });
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
        .with_routing(|routing| {
            routing.web(|router| {
                router.any("/any", |_req| async { "any response" });
            });
        })
        .create();

    let kernel = app.make::<ruvel::http::Kernel>().unwrap();

    for method in &[
        Method::GET,
        Method::HEAD,
        Method::POST,
        Method::PUT,
        Method::PATCH,
        Method::DELETE,
        Method::OPTIONS,
    ] {
        let req = Request::new(method.clone(), "/any");
        let res = kernel.handle(req).await;
        assert_eq!(res.status.code(), 200);
        assert_eq!(res.body, b"any response");
    }

    for method in &[Method::TRACE, Method::CONNECT] {
        let req = Request::new(method.clone(), "/any");
        let res = kernel.handle(req).await;
        assert_eq!(res.status.code(), 404);
    }
}

#[tokio::test]
async fn test_route_registrar_attribute_first_and_group_middleware() {
    let app = Application::configure("")
        .with_middleware(|m| {
            m.alias(
                "auth",
                HeaderMiddleware {
                    key: "X-Auth",
                    val: "ok",
                },
            );
            m.alias(
                "audit",
                HeaderMiddleware {
                    key: "X-Audit",
                    val: "logged",
                },
            );
        })
        .with_routing(|routing| {
            routing.web(|router| {
                router
                    .middleware(["auth", "audit"])
                    .prefix("admin")
                    .domain("admin.example.com")
                    .name("admin.")
                    .where_number("id")
                    .group(|router| {
                        router
                            .without_middleware("audit")
                            .get("/users/{id}", |req| async move {
                                format!("user:{}", req.param("id").unwrap_or("0"))
                            })
                            .name("users.show");

                        router.prefix("v1").group(|router| {
                            router
                                .post("/posts/{slug}", |_req| async { "created" })
                                .where_alpha("slug");
                        });
                    });
            });
        })
        .create();

    let kernel = app.make::<ruvel::http::Kernel>().unwrap();

    let res = kernel
        .handle(Request::new(Method::GET, "/admin/users/42"))
        .await;
    assert_eq!(res.body, b"user:42");
    assert_eq!(res.headers.get("X-Auth"), Some("ok"));
    assert_eq!(res.headers.get("X-Audit"), None);

    let res_post = kernel
        .handle(Request::new(Method::POST, "/admin/v1/posts/hello"))
        .await;
    assert_eq!(res_post.body, b"created");
    assert_eq!(res_post.headers.get("X-Auth"), Some("ok"));
    assert_eq!(res_post.headers.get("X-Audit"), Some("logged"));
}

#[tokio::test]
async fn test_trailing_slash_normalization() {
    let app = Application::configure("")
        .with_routing(|routing| {
            routing.web(|router| {
                router.get("/", |_req| async { "root" });
                router.get("/users", |_req| async { "users" });
                router.get("/posts/", |_req| async { "posts" });
                router.prefix("api/").group(|router| {
                    router.get("/items/{id}/", |req| async move {
                        format!("item:{}", req.param("id").unwrap_or("0"))
                    });
                });
            });
        })
        .create();

    let kernel = app.make::<ruvel::http::Kernel>().unwrap();

    // Root route
    let res_root = kernel.handle(Request::new(Method::GET, "/")).await;
    assert_eq!(res_root.body, b"root");

    // Registered without trailing slash, requested with trailing slash
    let res_users_slash = kernel.handle(Request::new(Method::GET, "/users/")).await;
    assert_eq!(res_users_slash.body, b"users");

    // Registered with trailing slash, requested both ways
    let res_posts = kernel.handle(Request::new(Method::GET, "/posts")).await;
    assert_eq!(res_posts.body, b"posts");
    let res_posts_slash = kernel.handle(Request::new(Method::GET, "/posts/")).await;
    assert_eq!(res_posts_slash.body, b"posts");

    // Parameterized route inside a group requested with trailing slash
    let res_item = kernel
        .handle(Request::new(Method::GET, "/api/items/99/"))
        .await;
    assert_eq!(res_item.body, b"item:99");
}
