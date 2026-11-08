use ruvel::console::Kernel as ConsoleKernel;
use ruvel::contracts::http::Kernel as HttpKernelContract;
use ruvel::foundation::Application;
use ruvel::http::{Kernel as HttpKernel, Method, Middleware, Next, Request, Response};
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;

struct GlobalHeaderMiddleware {
    key: &'static str,
    val: &'static str,
}

impl Middleware for GlobalHeaderMiddleware {
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

#[tokio::test]
async fn test_http_kernel_handles_global_middleware() {
    let app = Application::configure("")
        .with_middleware(|m| {
            m.append(GlobalHeaderMiddleware {
                key: "X-Global",
                val: "true",
            });
        })
        .with_routing(|router| {
            router.get("/hello", |_req| async { "world" });
        })
        .create();

    // Verify Kernel is registered in the container
    let kernel = app.make::<HttpKernel>().expect("Kernel must be in container");

    let req = Request::new(Method::GET, "/hello");
    let res = kernel.handle(req).await;

    assert_eq!(res.body, b"world");
    assert_eq!(res.headers.get("X-Global"), Some("true"));
}

#[tokio::test]
async fn test_http_kernel_handles_404_through_global_middleware() {
    let app = Application::configure("")
        .with_middleware(|m| {
            m.append(GlobalHeaderMiddleware {
                key: "X-Global-404",
                val: "applied",
            });
        })
        .create();

    let kernel = app.make::<HttpKernel>().unwrap();

    let req = Request::new(Method::GET, "/non-existent");
    let res = kernel.handle(req).await;

    assert_eq!(res.status.code(), 404);
    assert_eq!(res.headers.get("X-Global-404"), Some("applied"));
}

#[tokio::test]
async fn test_http_kernel_trait_object_dispatches() {
    let app = Application::configure("")
        .with_middleware(|m| {
            m.append(GlobalHeaderMiddleware {
                key: "X-Global",
                val: "true",
            });
        })
        .with_routing(|router| {
            router.get("/hello", |_req| async { "world" });
        })
        .create();

    // Same cast Application::listen uses for every accepted connection.
    let kernel: Arc<dyn HttpKernelContract> = app
        .make::<HttpKernel>()
        .expect("Kernel must be in container");

    let res = kernel.handle(Request::new(Method::GET, "/hello")).await;

    assert_eq!(res.body, b"world");
    assert_eq!(res.headers.get("X-Global"), Some("true"));
}

#[tokio::test]
async fn test_with_kernels_registers_both_kernels_in_container() {
    let app = Application::configure("")
        .with_kernels()
        .create();

    let console_kernel = app.make::<ConsoleKernel>();
    assert!(console_kernel.is_ok());

    let http_kernel = app.make::<HttpKernel>();
    assert!(http_kernel.is_ok());
}
