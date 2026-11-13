use ruvel::foundation::Application;
use ruvel::http::{Kernel, Method, Middleware, Next, Request, Response};
use ruvel::routing::Router;
use std::future::Future;
use std::pin::Pin;
use std::sync::{Arc, Mutex};

struct OrderMarkerMiddleware {
    tag: &'static str,
    log: Arc<Mutex<Vec<String>>>,
}

impl Middleware for OrderMarkerMiddleware {
    fn handle<'a>(
        &'a self,
        request: Request,
        next: Next,
    ) -> Pin<Box<dyn Future<Output = Response> + Send + 'a>> {
        Box::pin(async move {
            self.log
                .lock()
                .unwrap()
                .push(format!("before:{}", self.tag));
            let res = next.run(request).await;
            self.log.lock().unwrap().push(format!("after:{}", self.tag));
            res
        })
    }
}

struct GlobalTagMiddleware {
    val: &'static str,
}

impl Middleware for GlobalTagMiddleware {
    fn handle<'a>(
        &'a self,
        request: Request,
        next: Next,
    ) -> Pin<Box<dyn Future<Output = Response> + Send + 'a>> {
        Box::pin(async move {
            let res = next.run(request).await;
            res.header("X-Global-Tag", self.val)
        })
    }
}

#[tokio::test]
async fn test_static_beats_param_and_method_miss_falls_through() {
    let mut router = Router::new();
    router.get("/posts/create", |_req| async { "static:create" });
    router.get("/posts/{id}", |req| async move {
        format!("get:{}", req.param("id").unwrap_or(""))
    });
    router.post("/posts/{id}", |req| async move {
        format!("post:{}", req.param("id").unwrap_or(""))
    });

    // Exact static route beats `{id}` on GET
    let res = router
        .dispatch(Request::new(Method::GET, "/posts/create"))
        .await;
    assert_eq!(res.status.code(), 200);
    assert_eq!(res.body, b"static:create");

    // Static path exists for GET only; POST must fall through to the trie and match `POST /posts/{id}`
    let res = router
        .dispatch(Request::new(Method::POST, "/posts/create"))
        .await;
    assert_eq!(res.status.code(), 200);
    assert_eq!(res.body, b"post:create");

    // Numeric parameter hits dynamic GET
    let res = router
        .dispatch(Request::new(Method::GET, "/posts/42"))
        .await;
    assert_eq!(res.status.code(), 200);
    assert_eq!(res.body, b"get:42");
}

#[tokio::test]
async fn test_regex_constraints_and_backtracking() {
    let mut router = Router::new();

    // Register unconstrained route first to verify constrained params take precedence
    router.get("/users/{slug}", |req| async move {
        format!("slug:{}", req.param("slug").unwrap_or(""))
    });
    // Laravel-style unanchored pattern
    router
        .get("/users/{id}", |req| async move {
            format!("num:{}", req.param("id").unwrap_or(""))
        })
        .r#where("id", "[0-9]+");

    // Custom regex pattern (8 hex chars)
    router
        .get("/items/{hex}", |req| async move {
            format!("hex:{}", req.param("hex").unwrap_or(""))
        })
        .r#where("hex", "[0-9a-f]{8}");

    // Deep backtracking: `{id}` matches segment 1, fails segment 2 (`comments`), pops, tries `{slug}`
    router.get("/teams/{id}/posts", |req| async move {
        format!("team_posts:{}", req.param("id").unwrap_or(""))
    });
    router.get("/teams/{slug}/comments", |req| async move {
        format!("team_comments:{}", req.param("slug").unwrap_or(""))
    });

    let res = router
        .dispatch(Request::new(Method::GET, "/users/123"))
        .await;
    assert_eq!(res.body, b"num:123");

    // Unanchored `[0-9]+` must not match mixed alphanumeric segments
    let res = router
        .dispatch(Request::new(Method::GET, "/users/123abc"))
        .await;
    assert_eq!(res.body, b"slug:123abc");

    let res = router
        .dispatch(Request::new(Method::GET, "/items/deadbeef"))
        .await;
    assert_eq!(res.status.code(), 200);
    assert_eq!(res.body, b"hex:deadbeef");

    let res = router
        .dispatch(Request::new(Method::GET, "/items/deadbeef1"))
        .await;
    assert_eq!(res.status.code(), 404);

    let res = router
        .dispatch(Request::new(Method::GET, "/teams/alpha/comments"))
        .await;
    assert_eq!(res.status.code(), 200);
    assert_eq!(res.body, b"team_comments:alpha");
}

#[tokio::test]
async fn test_precomposed_middleware_order() {
    let log = Arc::new(Mutex::new(Vec::new()));
    let mut router = Router::new();

    let mw: Vec<Arc<dyn Middleware>> = vec![
        Arc::new(OrderMarkerMiddleware {
            tag: "first",
            log: Arc::clone(&log),
        }),
        Arc::new(OrderMarkerMiddleware {
            tag: "second",
            log: Arc::clone(&log),
        }),
    ];

    {
        let log_h = Arc::clone(&log);
        router.middleware(mw).get("/orders/{id}", move |req| {
            let log_h = Arc::clone(&log_h);
            async move {
                log_h
                    .lock()
                    .unwrap()
                    .push(format!("handler:{}", req.param("id").unwrap_or("")));
                "ok"
            }
        });
    }

    let res = router
        .dispatch(Request::new(Method::GET, "/orders/99"))
        .await;
    assert_eq!(res.body, b"ok");

    let recorded = log.lock().unwrap().clone();
    assert_eq!(
        recorded,
        vec![
            "before:first",
            "before:second",
            "handler:99",
            "after:second",
            "after:first",
        ]
    );
}

#[tokio::test]
async fn test_404_runs_global_middleware_installed_after_routes() {
    let mut router = Router::new();
    router.get("/present", |_req| async { "ok" });

    // Install global middleware after routes are registered
    router.set_middlewares(vec![Arc::new(GlobalTagMiddleware { val: "late" })]);

    let res = router.dispatch(Request::new(Method::GET, "/missing")).await;
    assert_eq!(res.status.code(), 404);
    assert_eq!(res.headers.get("X-Global-Tag"), Some("late"));
}

#[tokio::test]
async fn test_root_and_trailing_slash_variants() {
    let mut router = Router::new();
    router.get("/", |_req| async { "root" });
    router.get("/ping/", |_req| async { "pong" });

    assert_eq!(
        router.dispatch(Request::new(Method::GET, "/")).await.body,
        b"root"
    );
    assert_eq!(
        router.dispatch(Request::new(Method::GET, "")).await.body,
        b"root"
    );
    assert_eq!(
        router
            .dispatch(Request::new(Method::GET, "/ping"))
            .await
            .body,
        b"pong"
    );
    assert_eq!(
        router
            .dispatch(Request::new(Method::GET, "/ping/"))
            .await
            .body,
        b"pong"
    );
    // Double internal slash must not match
    assert_eq!(
        router
            .dispatch(Request::new(Method::GET, "/ping//extra"))
            .await
            .status
            .code(),
        404
    );
}

#[test]
#[should_panic(expected = "exceeds the maximum")]
fn test_too_many_route_params_panics_at_registration() {
    let mut router = Router::new();
    router.get(
        "/{p1}/{p2}/{p3}/{p4}/{p5}/{p6}/{p7}/{p8}/{p9}",
        |_req| async { "too many" },
    );
}

#[tokio::test]
async fn test_api_resource_accepts_non_numeric_ids() {
    struct SlugPostController;
    impl ruvel::http::Controller for SlugPostController {
        async fn show(&self, req: Request) -> impl ruvel::contracts::support::Responsable {
            format!("post:{}", req.param("id").unwrap_or(""))
        }
    }

    let app = Application::configure("")
        .with_routing(|routing| {
            routing.web(|router| {
                router.api_resource("posts", SlugPostController);
            });
        })
        .create();

    let kernel = app.make::<Kernel>().unwrap();
    let res = kernel
        .handle(Request::new(
            Method::GET,
            "/posts/550e8400-e29b-41d4-a716-446655440000",
        ))
        .await;
    assert_eq!(res.status.code(), 200);
    assert_eq!(res.body, b"post:550e8400-e29b-41d4-a716-446655440000");
}

#[tokio::test]
async fn test_head_options_trace_and_connect_routes() {
    let mut router = Router::new();
    router.get("/only-get", |_req| async { "get" });
    router.head("/status", |_req| async { "head:static" });
    router.options("/cors/{resource}", |req| async move {
        format!("options:{}", req.param("resource").unwrap_or(""))
    });
    router.trace("/echo", |_req| async { "trace:static" });
    router.connect("example.com:443", |_req| async { "connect:tunnel" });

    // HEAD does not implicitly fall back to GET
    let head_miss = router
        .dispatch(Request::new(Method::HEAD, "/only-get"))
        .await;
    assert_eq!(head_miss.status.code(), 404);

    let head_hit = router.dispatch(Request::new(Method::HEAD, "/status")).await;
    assert_eq!(head_hit.status.code(), 200);
    assert_eq!(head_hit.body, b"head:static");

    let options_hit = router
        .dispatch(Request::new(Method::OPTIONS, "/cors/users"))
        .await;
    assert_eq!(options_hit.status.code(), 200);
    assert_eq!(options_hit.body, b"options:users");

    let trace_hit = router.dispatch(Request::new(Method::TRACE, "/echo")).await;
    assert_eq!(trace_hit.status.code(), 200);
    assert_eq!(trace_hit.body, b"trace:static");

    let connect_hit = router
        .dispatch(Request::new(Method::CONNECT, "example.com:443"))
        .await;
    assert_eq!(connect_hit.status.code(), 200);
    assert_eq!(connect_hit.body, b"connect:tunnel");
}
