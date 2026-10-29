use crate::contracts::support::Responsable;
use crate::http::{Request, Response};

/// Base controller trait mirroring Laravel's resourceful controller actions.
///
/// Each method accepts a [`Request`] and returns a [`Response`].
/// Any type implementing [`Responsable`] can be converted using `.to_response()`.
/// Default implementations return a `501 Not Implemented` response.
#[allow(async_fn_in_trait)]
pub trait Controller: Send + Sync {
    /// Display a listing of the resource (GET /resources).
    async fn index(&self, _req: Request) -> impl Responsable {
        Response::not_implemented().text("501 Not Implemented: index")
    }

    /// Show the form for creating a new resource (GET /resources/create).
    async fn create(&self, _req: Request) -> impl Responsable {
        Response::not_implemented().text("501 Not Implemented: create")
    }

    /// Store a newly created resource in storage (POST /resources).
    async fn store(&self, _req: Request) -> impl Responsable {
        Response::not_implemented().text("501 Not Implemented: store")
    }

    /// Display the specified resource (GET /resources/{id}).
    async fn show(&self, _req: Request) -> impl Responsable {
        Response::not_implemented().text("501 Not Implemented: show")
    }

    /// Show the form for editing the specified resource (GET /resources/{id}/edit).
    async fn edit(&self, _req: Request) -> impl Responsable {
        Response::not_implemented().text("501 Not Implemented: edit")
    }

    /// Update the specified resource in storage (PUT/PATCH /resources/{id}).
    async fn update(&self, _req: Request) -> impl Responsable {
        Response::not_implemented().text("501 Not Implemented: update")
    }

    /// Remove the specified resource from storage (DELETE /resources/{id}).
    async fn destroy(&self, _req: Request) -> impl Responsable {
        Response::not_implemented().text("501 Not Implemented: destroy")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::http::HttpStatus;

    struct DummyController;

    impl Controller for DummyController {}

    struct BlogController;

    impl Controller for BlogController {
        // Returns Responsable via .to_response()
        async fn index(&self, _req: Request) -> impl Responsable {
            "Blog Index".to_response()
        }

        // Returns status tuple via .to_response()
        async fn store(&self, _req: Request) -> impl Responsable {
            (HttpStatus::Created, format!("Post created: {}", 1)).to_response()
        }

        // Returns Response directly
        async fn show(&self, _req: Request) -> impl Responsable {
            Response::ok().json(r#"{"id": 1}"#)
        }
    }

    #[tokio::test]
    async fn test_default_controller_methods() {
        let controller = DummyController;
        let req = Request {
            method: crate::http::Method::GET,
            path: "/test".to_string(),
            headers: std::collections::HashMap::new(),
            body: Vec::new(),
        };

        let res = controller.index(req).await;
        assert_eq!(res.status, HttpStatus::NotImplemented);
        assert_eq!(res.status.code(), 501);
    }

    #[tokio::test]
    async fn test_controller_responsable_return_types() {
        let controller = BlogController;
        let dummy_req = || Request {
            method: crate::http::Method::GET,
            path: "/test".to_string(),
            headers: std::collections::HashMap::new(),
            body: Vec::new(),
        };

        // 1. &'static str return
        let res_str = controller.index(dummy_req()).await;
        assert_eq!(res_str.status, HttpStatus::Ok);
        assert_eq!(res_str.status.code(), 200);
        assert_eq!(String::from_utf8(res_str.body).unwrap(), "Blog Index");

        // 2. (HttpStatus, String) return
        let res_tuple = controller.store(dummy_req()).await;
        assert_eq!(res_tuple.status, HttpStatus::Created);
        assert_eq!(res_tuple.status.code(), 201);
        assert_eq!(
            String::from_utf8(res_tuple.body).unwrap(),
            "Post created: 1"
        );

        // 3. Response return
        let res_resp = controller.show(dummy_req()).await;
        assert_eq!(res_resp.status, HttpStatus::Ok);
        assert_eq!(res_resp.status.code(), 200);
        assert_eq!(String::from_utf8(res_resp.body).unwrap(), r#"{"id": 1}"#);
    }
}
