pub mod config;
pub mod container;
pub mod contracts;
pub mod database;
pub mod foundation;
pub mod http;
pub mod routing;
pub mod support;

pub fn add(left: u64, right: u64) -> u64 {
    left + right
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::http::{Controller, HttpStatus, Method, Request, Response};
    use crate::routing::Router;
    use async_trait::async_trait;
    use std::collections::HashMap;
    use std::sync::Arc;

    #[test]
    fn it_works() {
        let result = add(2, 2);
        assert_eq!(result, 4);
    }

    struct DefaultResourceController;

    #[async_trait]
    impl Controller for DefaultResourceController {}

    #[tokio::test]
    async fn test_default_controller_methods_return_not_implemented() {
        let controller = DefaultResourceController;
        let req = Request {
            method: Method::GET,
            path: "/test".to_string(),
            headers: HashMap::new(),
            body: Vec::new(),
        };

        let res = controller.index(req).await;
        assert_eq!(res.status, HttpStatus::NotImplemented);
        assert_eq!(res.status.code(), 501);
        assert_eq!(res.status.message(), "Not Implemented");
    }

    struct PostController {
        app_name: String,
    }

    #[async_trait]
    impl Controller for PostController {
        async fn index(&self, _req: Request) -> Response {
            Response::ok().json(format!(r#"{{"app": "{}"}}"#, self.app_name))
        }

        async fn store(&self, req: Request) -> Response {
            let body_str = String::from_utf8_lossy(&req.body).to_string();
            Response::created().json(format!(r#"{{"created": "{}"}}"#, body_str))
        }

        async fn show(&self, _req: Request) -> Response {
            Response::ok().text("Post #1")
        }
    }

    #[tokio::test]
    async fn test_overridden_controller_actions() {
        let controller = PostController {
            app_name: "RuvelApp".to_string(),
        };

        // Test overridden index
        let req = Request {
            method: Method::GET,
            path: "/posts".to_string(),
            headers: HashMap::new(),
            body: Vec::new(),
        };
        let res = controller.index(req).await;
        assert_eq!(res.status, HttpStatus::Ok);
        assert_eq!(res.status.code(), 200);
        assert_eq!(res.status.message(), "OK");
        assert_eq!(res.headers.get("Content-Type").unwrap(), "application/json");
        assert_eq!(
            String::from_utf8(res.body).unwrap(),
            r#"{"app": "RuvelApp"}"#
        );

        // Test overridden store
        let req = Request {
            method: Method::POST,
            path: "/posts".to_string(),
            headers: HashMap::new(),
            body: b"new post".to_vec(),
        };
        let res = controller.store(req).await;
        assert_eq!(res.status, HttpStatus::Created);
        assert_eq!(res.status.code(), 201);
        assert_eq!(res.status.message(), "Created");
        assert_eq!(
            String::from_utf8(res.body).unwrap(),
            r#"{"created": "new post"}"#
        );

        // Test non-overridden action defaults to 501 Not Implemented
        let req = Request {
            method: Method::DELETE,
            path: "/posts/1".to_string(),
            headers: HashMap::new(),
            body: Vec::new(),
        };
        let res = controller.destroy(req).await;
        assert_eq!(res.status, HttpStatus::NotImplemented);
        assert_eq!(res.status.code(), 501);
    }

    #[tokio::test]
    async fn test_controller_integration_with_router() {
        let mut router = Router::new();
        let controller = Arc::new(PostController {
            app_name: "RuvelTest".to_string(),
        });

        let c = Arc::clone(&controller);
        router.get("/posts", move |req| {
            let c = Arc::clone(&c);
            Box::pin(async move { c.index(req).await })
        });

        let c = Arc::clone(&controller);
        router.post("/posts", move |req| {
            let c = Arc::clone(&c);
            Box::pin(async move { c.store(req).await })
        });

        // Dispatch GET /posts
        let req = Request {
            method: Method::GET,
            path: "/posts".to_string(),
            headers: HashMap::new(),
            body: Vec::new(),
        };
        let res = router.dispatch(req).await;
        assert_eq!(res.status, HttpStatus::Ok);
        assert_eq!(res.status.code(), 200);
        assert_eq!(
            String::from_utf8(res.body).unwrap(),
            r#"{"app": "RuvelTest"}"#
        );

        // Dispatch POST /posts
        let req = Request {
            method: Method::POST,
            path: "/posts".to_string(),
            headers: HashMap::new(),
            body: b"hello".to_vec(),
        };
        let res = router.dispatch(req).await;
        assert_eq!(res.status, HttpStatus::Created);
        assert_eq!(res.status.code(), 201);
        assert_eq!(
            String::from_utf8(res.body).unwrap(),
            r#"{"created": "hello"}"#
        );
    }
}
