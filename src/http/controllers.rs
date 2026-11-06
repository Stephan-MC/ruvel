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

