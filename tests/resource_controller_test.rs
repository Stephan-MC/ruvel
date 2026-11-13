use ruvel::contracts::support::Responsable;
use ruvel::foundation::Application;
use ruvel::http::middleware::{Middleware, Next};
use ruvel::http::{Controller, Kernel, Method, Request, Response, Status};
use std::future::Future;
use std::pin::Pin;

struct TestPostController;

impl Controller for TestPostController {
    async fn index(&self, _req: Request) -> impl Responsable {
        "posts.index"
    }

    async fn create(&self, _req: Request) -> impl Responsable {
        "posts.create"
    }

    async fn store(&self, _req: Request) -> impl Responsable {
        (Status::Created, "posts.store")
    }

    async fn show(&self, req: Request) -> impl Responsable {
        format!("posts.show:{}", req.param("id").unwrap_or("none"))
    }

    async fn edit(&self, req: Request) -> impl Responsable {
        format!("posts.edit:{}", req.param("id").unwrap_or("none"))
    }

    async fn update(&self, req: Request) -> impl Responsable {
        format!("posts.update:{}", req.param("id").unwrap_or("none"))
    }

    async fn destroy(&self, req: Request) -> impl Responsable {
        format!("posts.destroy:{}", req.param("id").unwrap_or("none"))
    }
}

#[tokio::test]
async fn test_resource_controller_all_7_actions() {
    let app = Application::configure("")
        .with_routing(|routing| {
            routing.web(|router| {
                router.resource("posts", TestPostController);
            });
        })
        .create();

    let kernel = app.make::<Kernel>().unwrap();

    // 1. index: GET /posts
    let res = kernel.handle(Request::new(Method::GET, "/posts")).await;
    assert_eq!(res.status.code(), 200);
    assert_eq!(res.body, b"posts.index");

    // 2. create: GET /posts/create
    let res = kernel
        .handle(Request::new(Method::GET, "/posts/create"))
        .await;
    assert_eq!(res.status.code(), 200);
    assert_eq!(res.body, b"posts.create");

    // 3. store: POST /posts
    let res = kernel.handle(Request::new(Method::POST, "/posts")).await;
    assert_eq!(res.status.code(), 201);
    assert_eq!(res.body, b"posts.store");

    // 4. show: GET /posts/123
    let res = kernel.handle(Request::new(Method::GET, "/posts/123")).await;
    assert_eq!(res.status.code(), 200);
    assert_eq!(res.body, b"posts.show:123");

    // 5. edit: GET /posts/123/edit
    let res = kernel
        .handle(Request::new(Method::GET, "/posts/123/edit"))
        .await;
    assert_eq!(res.status.code(), 200);
    assert_eq!(res.body, b"posts.edit:123");

    // 6. update: PUT and PATCH /posts/123
    let res = kernel.handle(Request::new(Method::PUT, "/posts/123")).await;
    assert_eq!(res.status.code(), 200);
    assert_eq!(res.body, b"posts.update:123");

    let res = kernel
        .handle(Request::new(Method::PATCH, "/posts/123"))
        .await;
    assert_eq!(res.status.code(), 200);
    assert_eq!(res.body, b"posts.update:123");

    // 7. destroy: DELETE /posts/123
    let res = kernel
        .handle(Request::new(Method::DELETE, "/posts/123"))
        .await;
    assert_eq!(res.status.code(), 200);
    assert_eq!(res.body, b"posts.destroy:123");
}

#[tokio::test]
async fn test_resource_controller_only_filtering() {
    let app = Application::configure("")
        .with_routing(|routing| {
            routing.web(|router| {
                router
                    .resource("posts", TestPostController)
                    .only(&["index", "show"]);
            });
        })
        .create();

    let kernel = app.make::<Kernel>().unwrap();

    // Allowed actions
    let res = kernel.handle(Request::new(Method::GET, "/posts")).await;
    assert_eq!(res.status.code(), 200);
    assert_eq!(res.body, b"posts.index");

    let res = kernel.handle(Request::new(Method::GET, "/posts/42")).await;
    assert_eq!(res.status.code(), 200);
    assert_eq!(res.body, b"posts.show:42");

    // Filtered out actions
    let res = kernel.handle(Request::new(Method::POST, "/posts")).await;
    assert_eq!(res.status.code(), 404);

    let res = kernel
        .handle(Request::new(Method::DELETE, "/posts/42"))
        .await;
    assert_eq!(res.status.code(), 404);
}

#[tokio::test]
async fn test_resource_controller_except_filtering() {
    let app = Application::configure("")
        .with_routing(|routing| {
            routing.web(|router| {
                router
                    .resource("posts", TestPostController)
                    .except(&["destroy", "edit"]);
            });
        })
        .create();

    let kernel = app.make::<Kernel>().unwrap();

    // Excluded actions
    let res = kernel
        .handle(Request::new(Method::DELETE, "/posts/42"))
        .await;
    assert_eq!(res.status.code(), 404);

    let res = kernel
        .handle(Request::new(Method::GET, "/posts/42/edit"))
        .await;
    assert_eq!(res.status.code(), 404);

    // Remaining actions still work
    let res = kernel.handle(Request::new(Method::GET, "/posts/42")).await;
    assert_eq!(res.status.code(), 200);
    assert_eq!(res.body, b"posts.show:42");
}

#[tokio::test]
async fn test_api_resource_skips_create_and_edit() {
    let app = Application::configure("")
        .with_routing(|routing| {
            routing.web(|router| {
                router.api_resource("posts", TestPostController);
            });
        })
        .create();

    let kernel = app.make::<Kernel>().unwrap();

    // API actions work
    assert_eq!(
        kernel
            .handle(Request::new(Method::GET, "/posts"))
            .await
            .status
            .code(),
        200
    );
    assert_eq!(
        kernel
            .handle(Request::new(Method::POST, "/posts"))
            .await
            .status
            .code(),
        201
    );
    assert_eq!(
        kernel
            .handle(Request::new(Method::GET, "/posts/99"))
            .await
            .status
            .code(),
        200
    );
    assert_eq!(
        kernel
            .handle(Request::new(Method::PUT, "/posts/99"))
            .await
            .status
            .code(),
        200
    );
    assert_eq!(
        kernel
            .handle(Request::new(Method::DELETE, "/posts/99"))
            .await
            .status
            .code(),
        200
    );

    // Form routes are skipped in API resources: `/posts/{id}/edit` has no route (404),
    // while `/posts/create` falls through to `show` with id = "create" (Laravel behavior
    // when `{id}` is unconstrained).
    let res = kernel
        .handle(Request::new(Method::GET, "/posts/create"))
        .await;
    assert_eq!(res.status.code(), 200);
    assert_eq!(res.body, b"posts.show:create");
    assert_eq!(
        kernel
            .handle(Request::new(Method::GET, "/posts/99/edit"))
            .await
            .status
            .code(),
        404
    );
}

struct CustomParamController;

impl Controller for CustomParamController {
    async fn show(&self, req: Request) -> impl Responsable {
        format!("slug:{}", req.param("slug").unwrap_or("none"))
    }
}

#[tokio::test]
async fn test_resource_controller_custom_parameter() {
    let app = Application::configure("")
        .with_routing(|routing| {
            routing.web(|router| {
                router
                    .resource("articles", CustomParamController)
                    .parameter("slug");
            });
        })
        .create();

    let kernel = app.make::<Kernel>().unwrap();

    let res = kernel
        .handle(Request::new(Method::GET, "/articles/rust-edition-2024"))
        .await;
    assert_eq!(res.status.code(), 200);
    assert_eq!(res.body, b"slug:rust-edition-2024");
}

struct AuthMiddleware;

impl Middleware for AuthMiddleware {
    fn handle<'a>(
        &'a self,
        req: Request,
        next: Next,
    ) -> Pin<Box<dyn Future<Output = Response> + Send + 'a>> {
        Box::pin(async move {
            let res = next.run(req).await;
            res.header("X-Resource-Auth", "verified")
        })
    }
}

#[tokio::test]
async fn test_resource_controller_with_middleware() {
    let app = Application::configure("")
        .with_middleware(|m| {
            m.alias("auth", AuthMiddleware);
        })
        .with_routing(|routing| {
            routing.web(|router| {
                router
                    .resource("posts", TestPostController)
                    .middleware("auth");
            });
        })
        .create();

    let kernel = app.make::<Kernel>().unwrap();

    let res = kernel.handle(Request::new(Method::GET, "/posts")).await;
    assert_eq!(res.headers.get("X-Resource-Auth"), Some("verified"));

    let res = kernel.handle(Request::new(Method::GET, "/posts/10")).await;
    assert_eq!(res.headers.get("X-Resource-Auth"), Some("verified"));
}

#[tokio::test]
async fn test_resource_controller_in_group_with_prefix() {
    let app = Application::configure("")
        .with_routing(|routing| {
            routing.web(|router| {
                router.prefix("api/v1").group(|router| {
                    router.api_resource("posts", TestPostController);
                });
            });
        })
        .create();

    let kernel = app.make::<Kernel>().unwrap();

    let res = kernel
        .handle(Request::new(Method::GET, "/api/v1/posts"))
        .await;
    assert_eq!(res.status.code(), 200);
    assert_eq!(res.body, b"posts.index");

    let res = kernel
        .handle(Request::new(Method::GET, "/api/v1/posts/77"))
        .await;
    assert_eq!(res.status.code(), 200);
    assert_eq!(res.body, b"posts.show:77");
}

struct DefaultOnlyController;
impl Controller for DefaultOnlyController {}

#[tokio::test]
async fn test_default_controller_methods_return_501() {
    let app = Application::configure("")
        .with_routing(|routing| {
            routing.web(|router| {
                router.resource("items", DefaultOnlyController);
            });
        })
        .create();

    let kernel = app.make::<Kernel>().unwrap();

    let res = kernel.handle(Request::new(Method::GET, "/items")).await;
    assert_eq!(res.status.code(), 501);
    assert_eq!(res.body, b"501 Not Implemented: index");

    let res = kernel.handle(Request::new(Method::GET, "/items/1")).await;
    assert_eq!(res.status.code(), 501);
    assert_eq!(res.body, b"501 Not Implemented: show");
}
