# Route-Level Middleware & PendingRouteRegistrar

This document describes the design and implementation of route-level middleware and the `PendingRouteRegistrar` pattern in Ruvel.

## Overview

Previously, middleware in Ruvel could only be attached globally or through `router.group(&["alias"], ...)`.

With the introduction of `PendingRouteRegistrar` and `IntoMiddleware`:
1. Route registration methods (`router.get`, `router.post`, `router.put`, `router.delete`, `router.patch`, `router.any`, `router.match`, etc.) return a `PendingRouteRegistrar`.
2. Middleware can be attached directly to individual routes using `.middleware(...)`.
3. Middleware can be excluded from route groups using `.without_middleware(...)`.
4. Route metadata and modifiers such as `.name(...)`, `.prefix(...)`, `.domain(...)`, `.where(...)`, `.where_number(...)`, and `.where_alpha(...)` can be chained fluently.
5. `PendingRouteRegistrar` automatically registers and commits the finalized route definition into the router when dropped (or when `.commit()` is explicitly called).
6. Fluent group registration is supported via `router.prefix(...).middleware(...).domain(...).group(|router| { ... })`.

---

## Key Components

### 1. `IntoMiddleware` Trait (`src/routing/into_middleware.rs`)

The `IntoMiddleware` trait enables flexible middleware attachment. It is implemented for:
- String alias/group names: `&str`, `String`, `&String`
- Collections of alias/group names: `&[&str]`, `[&str; N]`, `&[&str; N]`, `[String; N]`, `Vec<&str>`, `Vec<String>`
- Direct middleware instances: `Arc<M>` (where `M: Middleware + 'static`), `Vec<Arc<dyn Middleware>>`, `&[Arc<dyn Middleware>]`, `[Arc<dyn Middleware>; N]`, `&[Arc<dyn Middleware>; N]`

### 2. `PendingRouteRegistrar` (`src/routing/pending_route.rs`)

A RAII builder that wraps pending route registration:
- **`.middleware(impl IntoMiddleware)`**: Attach one or multiple middleware (aliases, groups, or direct instances) to the specific route.
- **`.without_middleware(impl IntoMiddleware)`**: Exclude specified middleware aliases or groups inherited from group scopes.
- **`.name(impl Into<String>)`**: Assign a name to the route metadata.
- **`.prefix(impl Into<String>)`**: Prepend an additional path prefix to this route.
- **`.domain(impl Into<String>)`**: Attach a domain constraint or metadata.
- **`.where(param, pattern)`**: Attach parameter validation regex pattern.
- **`.where_number(param)`**: Shortcut for numeric parameter validation (`^[0-9]+$`).
- **`.where_alpha(param)`**: Shortcut for alphabetic parameter validation (`^[a-zA-Z]+$`).
- **`Drop` impl**: Automatically commits the route to the router if not explicitly committed.

### 3. `RouteGroupRegistrar` (`src/routing/pending_route.rs`)

Enables fluent group definitions directly from `Router`:
- `router.prefix("/api").middleware("auth").group(|router| { ... })`
- `router.domain("admin.example.com").group(|router| { ... })`

### 4. `Router` Updates (`src/routing/router.rs`)

- All verb methods (`get`, `post`, `put`, `delete`, `patch`, `any`, `match`, and `add_route`) return `PendingRouteRegistrar`.
- Group scoping with stacked prefixes and middleware layers.
- Route pipeline compilation resolves global middleware + group middleware + route-level middleware, filtering out any excluded middleware specified by `.without_middleware(...)`.

---

## Examples

### 1. Attaching route-level middleware with an alias
```rust
router.get("/dashboard", |_req| async { "Dashboard" })
    .middleware("auth");
```

### 2. Attaching direct middleware instances & multiple layers
```rust
let rate_limiter = Arc::new(RateLimiter::new());

router.post("/submit", |_req| async { "Created" })
    .middleware(rate_limiter)
    .middleware(["throttle", "log"]);
```

### 3. Naming and prefixes
```rust
router.get("/profile", |_req| async { "Profile" })
    .name("user.profile")
    .prefix("/account");
// Accessible at GET /account/profile, named "user.profile"
```

### 4. Fluent route grouping & excluding middleware
```rust
router.prefix("/api/v1")
    .middleware("auth")
    .group(|router| {
        // Protected route
        router.get("/orders", |_req| async { "Orders" });

        // Public route in protected group
        router.get("/public-catalog", |_req| async { "Catalog" })
            .without_middleware("auth");
    });
```
