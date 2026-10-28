# Ruvel Framework

[![Rust](https://img.shields.io/badge/rust-2024%20edition-orange.svg)](https://www.rust-lang.org/)
[![License](https://img.shields.io/badge/license-MIT%2FApache--2.0-blue.svg)](LICENSE)

**Ruvel** is an asynchronous, expressive web framework for Rust inspired directly by **Laravel's architecture and developer ergonomics**. Built on top of [Tokio](https://tokio.rs/), Ruvel brings the elegant patterns of PHP's most beloved framework—Service Providers, IoC Containers, Resource Controllers, and `Responsable` contracts—into the high-performance, type-safe world of Rust.

---

## Features

- **Application Kernel & Two-Phase Lifecycle**: Clean separation between service registration (`register`) and bootstrapping (`boot`).
- **Laravel-Inspired Service Providers**: Decouple components with `ServiceProvider` contracts (`Illuminate\Support\ServiceProvider`).
- **Flexible HTTP Routing**: Expressive routing for `GET`, `POST`, `PUT`, `DELETE`, and `PATCH`.
- **`Responsable` Contract**: Route handlers can return anything implementing `Responsable` (`&'static str`, `String`, tuples, or models) without manual boilerplate.
- **Smart Defaults**: Defaults to `Content-Type: text/html; charset=utf-8` for web responses, with fluent helpers for `.json()`, `.text()`, and `.html()`.
- **RFC-Compliant `HttpStatus` & Case-Insensitive Headers**: Strongly typed `HttpStatus` enum with numeric codes, canonical reason phrases, and Symfony-style constants (`Response::HTTP_OK`, `Response::HTTP_CREATED`).
- **Resource Controllers**: Pre-configured RESTful actions (`index`, `create`, `store`, `show`, `edit`, `update`, `destroy`) with sensible default fallback responses.

---

## Project Structure

```text
src/
├── config/             # Application configuration
├── container/          # IoC / Dependency Injection container
├── contracts/          # Core framework contracts & interfaces
│   └── support/
│       └── responsable.rs  # Responsable trait (Illuminate\Contracts\Support\Responsable)
├── database/           # Database layer and Eloquent ORM
│   └── eloquent/       # Eloquent model implementations
├── foundation/         # Application bootstrap & TCP connection listener
├── http/               # HTTP kernel, Request, Response, Headers, and Controllers
│   ├── controllers.rs  # RESTful resource Controller trait
│   ├── headers.rs      # Case-insensitive HeaderMap
│   ├── request.rs      # HTTP Request parser & methods
│   ├── response.rs     # Fluent HTTP Response builder
│   └── status.rs       # Strongly-typed HttpStatus enum & RFC status codes
├── routing/            # Router & dispatch engine
└── support/            # ServiceProvider trait & utilities
```

---

## Quick Start

### 1. Basic Application

```rust
use ruvel::foundation::Application;
use ruvel::http::{HttpStatus, Request, Response};

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut app = Application::new();

    // 1. Returning a string literal (automatically 200 OK text/html)
    app.router.get("/", |_req: Request| async {
        "<h1>Welcome to Ruvel!</h1>"
    });

    // 2. Returning a dynamic String (200 OK text/html)
    app.router.get("/greeting", |_req: Request| async {
        format!("<h2>Hello from worker thread!</h2>")
    });

    // 3. Returning a status tuple (201 Created)
    app.router.post("/items", |_req: Request| async {
        (HttpStatus::Created, "<p>Item created successfully</p>")
    });

    // 4. Returning a fluent JSON Response
    app.router.get("/api/health", |_req: Request| async {
        Response::ok().json(r#"{"status": "healthy"}"#)
    });

    // Start server on port 8080
    app.listen(8080).await?;

    Ok(())
}
```

---

## Core Concepts

### Service Providers

Just like Laravel's `AppServiceProvider`, providers in Ruvel have two distinct lifecycle stages:

1. `register(&self, container: &mut Container)`: Bind singletons or services into the container.
2. `boot(&self, container: &Container, router: &mut Router)`: Register routes or resolve initialized dependencies.

```rust
use async_trait::async_trait;
use ruvel::container::Container;
use ruvel::routing::Router;
use ruvel::support::ServiceProvider;

pub struct RouteServiceProvider;

#[async_trait]
impl ServiceProvider for RouteServiceProvider {
    async fn register(&self, _container: &mut Container) {
        // Register service bindings here
    }

    async fn boot(&self, _container: &Container, router: &mut Router) {
        router.get("/about", |_req| async {
            "<h1>About Ruvel</h1>"
        });
    }
}
```

Register providers during application setup:

```rust
let app = Application::new()
    .register_provider(RouteServiceProvider)
    .await;
```

---

### The `Responsable` Contract

Ruvel mirrors `Illuminate\Contracts\Support\Responsable`. Any type implementing `Responsable` can be returned directly from route handlers:

```rust
use ruvel::contracts::support::Responsable;
use ruvel::http::Response;

pub struct UserProfile {
    pub username: String,
}

impl Responsable for UserProfile {
    fn to_response(self) -> Response {
        Response::ok().json(format!(r#"{{"user": "{}"}}"#, self.username))
    }
}

// In your router:
router.get("/me", |_req| async {
    UserProfile {
        username: "ferris".to_string(),
    }
});
```

---

### Resource Controllers

Resource controllers provide default implementations for all standard actions. Implement only what your resource needs:

```rust
use async_trait::async_trait;
use ruvel::contracts::support::Responsable;
use ruvel::http::{Controller, HttpStatus, Request, Response};

pub struct ArticleController;

#[async_trait]
impl Controller for ArticleController {
    // Overriding index (GET /articles)
    async fn index(&self, _req: Request) -> Response {
        "<h1>Articles List</h1>".to_response()
    }

    // Overriding store (POST /articles)
    async fn store(&self, req: Request) -> Response {
        (HttpStatus::Created, "<p>Article published</p>").to_response()
    }

    // show, edit, update, and destroy default to 501 Not Implemented
}
```

---

### HTTP Status Codes & Headers

- **`HttpStatus`**: RFC-compliant enum with `status.code()` (e.g. `200`, `404`) and `status.message()` (e.g. `"OK"`, `"Not Found"`).
- **Constants**: Access constants directly via `Response::HTTP_OK`, `Response::HTTP_CREATED`, `Response::HTTP_NOT_FOUND`, etc.
- **Case-Insensitive Headers**: Headers can be fetched using any casing (`Content-Type`, `content-type`, `CONTENT-TYPE`).

```rust
let res = Response::ok()
    .header("X-Custom-Header", "value123")
    .html("<h1>Rendered HTML</h1>");

assert_eq!(res.headers.get("x-custom-header"), Some("value123"));
assert_eq!(res.headers.get("X-CUSTOM-HEADER"), Some("value123"));
```

---

## Roadmap

- [ ] Route parameters & regex matching (`/posts/{id}`)
- [ ] Onion-style HTTP middleware pipeline (`Request -> Next -> Response`)
- [ ] Type-safe dependency resolution in IoC container (`Arc<dyn Any>`)
- [ ] Validation & FormRequest abstractions
- [ ] Eloquent query builder & migrations
- [ ] Artisan CLI integration with `clap`

---

## License

This project is licensed under the MIT License or Apache-2.0 License.
