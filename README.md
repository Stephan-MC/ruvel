# Ruvel

[![Rust](https://img.shields.io/badge/rust-2024%20edition-orange.svg)](https://www.rust-lang.org/)
[![License](https://img.shields.io/badge/license-MIT%2FApache--2.0-blue.svg)](LICENSE)

> **This framework is not ready for production.**
>
> Ruvel `0.1.x` is an early preview. Routing, middleware, the service container, controllers, and a small HTTP/1.1 server are far enough along to try locally. Authentication, validation, sessions, a real database layer, TLS, and several builder hooks are missing or panic when called. Do not deploy an application built on Ruvel.

Ruvel is an asynchronous web framework for Rust, shaped after Laravel. It runs on [Tokio](https://tokio.rs/) and aims for the same split Laravel uses: service providers, an IoC container, resource controllers, and a `Responsable` contract.

---

## Implemented

### Routing

`Router` implements the `RouteRegistrar` trait and stores one [matchit](https://docs.rs/matchit) radix tree per HTTP method.

- Verbs & methods on `RouteRegistrar`: `get`, `post`, `put`, `delete`, `patch`, `any`, `r#match`, plus fluent route/group attributes (`domain`, `prefix`, `name`, `r#where`, `where_number`, `where_alpha`, `middleware`, `without_middleware`), each returning a `PendingRouteRegistrar`.
- Path parameters use `{name}` segments. A match fills `Request::params`, and `Request::param("id")` reads one value.
- The query string is split off the path and kept raw on `Request::query`. The router matches the path only.
- `router.middleware("auth").group(...)` or `router.prefix("/api").group(...)` attaches middleware or prefixes to the routes registered inside the closure. Names are resolved when the route is registered. An unknown name panics at startup.
- A missing route returns `404 Not Found`. Global middleware still runs for that response.
- A route with an empty pipeline skips the middleware machinery and calls the handler directly.
- A handler may return any type that implements `Responsable`.

Register middleware before routes. `with_middleware` installs the stacks on the router, and each route copies its pipeline at registration time. Routes added earlier keep an empty pipeline.

```rust
use ruvel::foundation::Application;

let app = Application::configure("")
    .with_middleware(|middleware| {
        middleware.alias("auth", Auth); // Auth: impl Middleware
    })
    .with_routing(|router| {
        router.get("/posts/{id}", |req| async move {
            format!("Post {}", req.param("id").unwrap_or("unknown"))
        });

        router.middleware("auth").group(|router| {
            router.post("/posts", |_req| async { "created" });
        });
    })
    .create();
```

### Middleware

The pipeline is the Laravel onion. Each layer receives the request and a `Next`. Calling `next.run(request).await` continues inward. The handler runs when the stack is exhausted. The response walks back out through the layers that called `next`.

The `Middleware` trait is object-safe. `handle` returns a boxed future:

```rust
use ruvel::http::{Middleware, Next, Request, Response};
use std::future::Future;
use std::pin::Pin;

struct RequestId;

impl Middleware for RequestId {
    fn handle<'a>(
        &'a self,
        request: Request,
        next: Next<'a>,
    ) -> Pin<Box<dyn Future<Output = Response> + Send + 'a>> {
        Box::pin(async move {
            next.run(request).await.header("X-Request-Id", "1")
        })
    }
}
```

`ApplicationBuilder::with_middleware` is the configuration surface, in the same spirit as Laravel's `bootstrap/app.php` middleware callback:

| Method | Effect |
| --- | --- |
| `append` | Add a layer at the end of the global stack. |
| `prepend` | Add a layer at the front. The latest prepend runs first. |
| `remove::<T>()` | Drop every global layer of type `T`. |
| `replace::<T, _>(middleware)` | Swap every global layer of type `T`. |
| `alias(name, middleware)` | Register one layer under a route name. |
| `group(name, layers)` | Register an ordered list under a route name. |

Global order is prepends, then the base stack, then appends. `remove` and `replace` apply to that combined stack. Groups and aliases are not part of the global stack. They are copied onto a route when that route names them.

There is no `terminate` hook, and aliases do not take parameters (`auth:api` is not parsed).

### Service container

`ruvel::container::Container` is a type-keyed IoC container. Lookups swap an `Arc` through `arc-swap`, so a resolve does not take a lock.

| Method | Laravel equivalent | Behavior |
| --- | --- | --- |
| `bind` | `$container->bind()` | Store a factory. `make` calls it on every resolve. |
| `singleton` | `$container->singleton()` | Store one instance and hand out clones of the `Arc`. |
| `alias` | `$container->alias()` | Map a string name to a `TypeId`. |
| `make::<T>()` | `$container->make()` | Resolve `T`. |
| `make_alias::<T>(name)` | resolve by alias | Resolve the type registered under `name`. |

```rust
use ruvel::container::Container;
use std::sync::Arc;

struct Database {
    url: String,
}

let container = Container::new();
container.singleton(Database {
    url: "postgres://localhost/app".into(),
});

let db: Arc<Database> = container.make()?;
```

`ServiceProvider` is the extension point, with Laravel's two phases:

1. `register` runs immediately when the provider is added. It receives `&mut Container` and should only bind services.
2. `boot` runs after every provider has been registered, when the application starts listening. It can resolve services and register routes. The default `boot` does nothing.

```rust
use async_trait::async_trait;
use ruvel::container::Container;
use ruvel::routing::Router;
use ruvel::support::ServiceProvider;

pub struct AppServiceProvider;

#[async_trait]
impl ServiceProvider for AppServiceProvider {
    async fn register(&self, container: &mut Container) {
        container.singleton(Database {
            url: "postgres://localhost/app".into(),
        });
    }

    async fn boot(&self, _container: &Container, router: &mut Router) {
        router.get("/health", |_req| async { "ok" });
    }
}
```

Attach a provider with `ApplicationBuilder::with_provider`, which is async. The container is not passed into route closures or controller methods. Resolving a service inside a handler means reaching a container you stored yourself.

### Controllers

`ruvel::http::Controller` mirrors a Laravel resource controller. Each action takes a `Request` and returns `impl Responsable`. Override the actions you need. The others answer `501 Not Implemented`.

| Action | Typical route |
| --- | --- |
| `index` | `GET /resources` |
| `create` | `GET /resources/create` |
| `store` | `POST /resources` |
| `show` | `GET /resources/{id}` |
| `edit` | `GET /resources/{id}/edit` |
| `update` | `PUT` or `PATCH /resources/{id}` |
| `destroy` | `DELETE /resources/{id}` |

```rust
use ruvel::http::{Controller, Request, Responsable, Status};

pub struct ArticleController;

impl Controller for ArticleController {
    async fn index(&self, _req: Request) -> impl Responsable {
        "<h1>Articles</h1>"
    }

    async fn store(&self, _req: Request) -> impl Responsable {
        (Status::Created, "<p>Article published</p>")
    }
}
```

You register each action on the router yourself. There is no `resource()` helper, no controller middleware attribute, and no constructor injection.

### Also in place

- **Application lifecycle.** `Application::configure(base_path)` returns a builder. `with_provider`, `with_routing`, and `with_middleware` shape it. `create()` returns the `Application`. `listen(host, port)` boots every provider, then accepts TCP connections.
- **HTTP/1.1 server.** Requests are parsed with `httparse`. One connection can serve keep-alive and pipelined requests. The body is framed with `Content-Length`. A request larger than 64 KiB is dropped. A non-identity `Transfer-Encoding` closes the connection.
- **`Responsable`.** Route handlers can return `&'static str`, `String`, `Cow<str>`, `Box<str>`, `Response`, `()`, `(Status, T)`, or `(u16, T)`. Strings become `200 OK` with `Content-Type: text/html; charset=utf-8`.
- **Responses.** `Response::ok`, `created`, `no_content`, `bad_request`, `not_found`, and `not_implemented`, plus `.text()`, `.json()`, `.html()`, and `.header()`. Numeric constants such as `Response::HTTP_OK` are available. `Status` covers the usual RFC codes, with `code()` and `message()`.
- **Headers.** `HeaderMap` looks up names without regard to case and writes the casing of the last insert.

```rust
use ruvel::foundation::Application;
use ruvel::http::Response;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let app = Application::configure("")
        .with_routing(|router| {
            router.get("/", |_req| async { "<h1>Welcome to Ruvel</h1>" });

            router.get("/health", |_req| async {
                Response::ok().json(r#"{"status":"healthy"}"#)
            });
        })
        .create();

    app.listen("127.0.0.1".into(), 8080).await?;
    Ok(())
}
```

---

## Still to be done

These builder methods are public and panic with `todo!` if you call them:

- `with_events`
- `with_commands`
- `with_kernels`
- `with_scheduler`

These types exist only as placeholders:

- `database::eloquent::Model` is an empty struct. As a `Responsable` it returns an empty `200 OK`. There is no query builder, connection, migration runner, or seeder.
- `config::Config` holds `name` and `debug`. Nothing loads `.env` files or config files, and the application does not read this struct.
- `contracts::http::Kernel` and `contracts::container::Container` are empty traits. The working container is the concrete `container::Container`.

Not started:

- Validation and form requests
- Authentication, authorization, sessions, cookies, and CSRF
- Views and templating
- Named routes, route-model binding, and automatic resource registration
- `HEAD`, `OPTIONS`, `any`, `match`, and custom fallback routes
- Middleware parameters and `terminate` callbacks
- Injecting container services into closures or controller methods, and a request-scoped container
- A parsed query map, multipart bodies, and file uploads
- Redirect and cookie helpers
- Chunked request bodies, TLS, and HTTP/2
- An Artisan-style CLI, queues, mail, cache, and notifications

---

## Layout

```text
src/
├── config/                  # Config stub (name, debug)
├── configuration/           # ApplicationBuilder and middleware config (crate-private)
├── container/               # Service container
├── contracts/
│   ├── support/responsable.rs
│   ├── container/           # Empty contract
│   └── http/kernel.rs       # Empty contract
├── database/eloquent/       # Empty Model stub
├── foundation/              # Application bootstrap and TCP listener
├── http/
│   ├── controllers.rs       # Resource controller trait
│   ├── headers.rs           # Case-insensitive HeaderMap
│   ├── middleware.rs        # Middleware trait and Next pipeline
│   ├── request.rs           # HTTP/1.1 parser
│   ├── response.rs          # Response builder
│   └── status.rs            # Status enum
├── routing/                 # Router and dispatch
└── support/                 # ServiceProvider
```

`configuration` is private. Call `Application::configure` and let the closure arguments infer `Middleware` and `Router`.

---

## License

Licensed under either of:

- MIT license
- Apache License, Version 2.0

at your option.
