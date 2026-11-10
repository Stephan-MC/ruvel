# AGENTS.md — Ruvel Framework Agentic Workflow Guide

This file defines the architectural invariants, coding standards, testing workflows, and operational rules for AI coding agents working on **Ruvel** (`ruvel`), an asynchronous web framework for Rust (Edition 2024) modeled after Laravel's architecture.

---

## 1. Project Overview & Laravel-to-Rust Design Philosophy

Ruvel brings Laravel's developer experience and architectural patterns (IoC Container, Service Providers, HTTP/Console Kernels, Onion Middleware Pipelines, Resource Controllers, Eloquent-style Models, and Contracts) to Rust on top of `tokio`.

### Core Architectural Invariants
- **Laravel Naming & Structural Parity**: Preserve Laravel's conceptual terminology and lifecycle separation:
  - **Contracts (`src/contracts/`)**: Interfaces/traits defining framework capabilities (`contracts::http::Kernel`, `contracts::console::Kernel`, `contracts::container::Container`, `contracts::pipeline::Pipeable`, `contracts::support::Responsable`).
  - **Service Providers (`src/support/`, `src/*/*_service_provider.rs`)**: Two-phase lifecycle:
    1. `register(&self, container: &mut Container)`: Bind services into the container only. Never resolve other services here.
    2. `boot(&self, container: &Container, router: &mut Router)`: Runs after all providers are registered; safe to resolve dependencies and register routes.
  - **IoC Container (`src/container/`)**: Type-keyed (`TypeId`) and string-aliased service container supporting transient factories (`bind`), lazy singletons (`singleton`), pre-built instances (`instance`), and resolution callbacks (`before_resolving`, `after_resolving`).
  - **Configuration Surface (`src/configuration/`)**: Keep `mod configuration;` **crate-private** in `src/lib.rs`. Users configure the framework fluently through `Application::configure(base_path)` (`with_routing`, `with_middleware`, `with_providers`, `with_kernels`, etc.), letting closure arguments infer internal builder types (`Middleware`, `Router`).
  - **Pipeline & Middleware (`src/pipeline/`, `src/http/middleware/`)**: Follow the Laravel onion pipeline (`Pipeline::new().send(req).through(layers).then(...)`).

### Directory Layout
```text
src/
├── config/          # Configuration structs and helpers
├── configuration/   # Crate-private ApplicationBuilder & Middleware configuration
├── console/         # Console Kernel & CLI command execution
├── container/       # Lock-free IoC service container (ArcSwap)
├── contracts/       # Framework-wide trait contracts (container, console, http, pipeline, support)
├── database/        # DatabaseManager (DB facade), Connections (SQLite/MySQL via sqlx), QueryBuilder, and Eloquent Model trait
├── events/          # Event dispatcher and EventServiceProvider
├── foundation/      # Application bootstrap, core bindings/aliases, and TCP HTTP/1.1 listener
├── http/            # Request, Response, Status, HeaderMap, Controller trait, HTTP Kernel, and Middleware
├── pipeline/        # Generic onion Pipeline and Pipe implementations
├── routing/         # Radix-tree Router (matchit), PendingRoute, and route middleware resolution
└── support/         # ServiceProvider trait and base providers (e.g., LogServiceProvider)
tests/               # Integration tests verifying end-to-end subsystem behavior
```

---

## 2. Agent Autonomy, Planning & Execution Rules

### When to Plan vs. Execute Directly
1. **Plan First (Require User Approval)**:
   - Introducing or changing public traits, builder signatures, or `src/contracts/` interfaces.
   - Modifying core lifecycle execution order in `Application` (`src/foundation/application.rs`) or `Kernel` (`src/http/kernel.rs`).
   - Adding new crate dependencies to `Cargo.toml`.
   - Implementing major unstarted subsystems listed in `README.md` (e.g., authentication, validation, sessions, queues).
2. **Execute Directly**:
   - Bug fixes, internal refactors, clippy/compiler warning cleanups, and documentation updates.
   - Writing unit or integration tests in `tests/`.
   - Implementing self-contained methods or helpers that follow established patterns.

### Subagent & Context Management
- For broad codebase exploration or multi-file impact analysis, delegate read-only searches to a research subagent rather than polluting the primary context window.
- When working on isolated features that touch distinct modules (e.g., `database` vs. `routing`), keep changes scoped and verify each module independently.
- Maintain existing comments and docstrings unless they are directly invalidated by your code changes.

---

## 3. Coding Standards & Rust Idioms (Edition 2024)

### Async Traits & Object Safety
- **Prefer Native `async fn` in Traits**: Rust 2024 supports native `async fn` in traits (`async_fn_in_trait = "allow"` is configured in `Cargo.toml`). Avoid adding `#[async_trait]` to traits that do not require dynamic dispatch (`dyn Trait`), such as `Eloquent::Model` or `http::Controller`.
- **Dynamic Dispatch (`dyn Trait`)**: When a trait must be object-safe (e.g., stored in collections like `Vec<Arc<dyn Middleware>>` or `Vec<Box<dyn ServiceProvider>>`), use explicit boxed futures (`BoxFuture<'a, Output>` / `Pin<Box<dyn Future<Output = T> + Send + 'a>>`) or synchronous registration hooks where appropriate.

### Concurrency & Hot-Path Performance
- **Lock-Free Reads on Hot Paths**: Prefer `arc_swap::ArcSwap`, `dashmap::DashMap`, and `std::sync::OnceLock` for state read on every HTTP request or container resolution (see `Container` and `Router`).
- Avoid holding standard `Mutex` or `RwLock` guards across `.await` points.

### Error Handling
- **No Unchecked Panics in Library Runtime Paths**: Avoid `.unwrap()` and `.expect()` in request handling, database queries, routing dispatch, and container resolution.
- **Structured Errors via `thiserror`**: Define domain-specific error enums deriving `thiserror::Error` (see `src/database/error.rs` with `DatabaseError` and `ModelNotFoundException`) and propagate errors using `Result<T, E>` and `?`.
- **Startup Configuration**: Explicit panics are only acceptable for unrecoverable startup misconfigurations (e.g., referencing an unregistered middleware alias during route table construction) when the builder API intentionally enforces fail-fast startup.

---

## 4. Testing, Verification & Documentation Workflow

Before considering any task complete, agents **must** run the verification suite:

```bash
# 1. Compile and run all unit & integration tests
cargo test

# 2. Run Clippy lints and ensure zero warnings
cargo clippy --all-targets -- -D warnings

# 3. Check formatting
cargo fmt --check
```

### Testing Guidelines
- Place integration tests in `tests/<feature>_test.rs` (e.g., `tests/container_binding_test.rs`, `tests/eloquent_model_test.rs`, `tests/route_middleware_test.rs`).
- For database tests, use in-memory SQLite (`sqlite::memory:`) via `DatabaseManager` so tests remain fast, isolated, and self-contained without requiring external database daemons.

### Documentation Sync (`README.md`)
- **Strict Invariant**: Whenever a feature transitions from a stub/`todo!()` to a working implementation—or when public API examples change—update `README.md` in the same task so the **"Implemented"** and **"Still to be done"** sections remain accurate.

---

## 5. Git & Commit Conventions

- **Base Branch**: Target feature and fix branches against `dev`.
- **Branch Naming**: Use descriptive prefixes such as `feat/<topic>`, `fix/<topic>`, or `refactor/<topic>`.
- **Conventional Commits**: Format all commit messages using Conventional Commits:
  - `feat(<scope>): <description>` (e.g., `feat(database): implement Eloquent models and base providers`)
  - `fix(<scope>): <description>` (e.g., `fix(routing): preserve query string on Request`)
  - `refactor(<scope>): <description>` (e.g., `refactor(structure): organize service providers and managers`)
  - `test(<scope>): <description>`
  - `docs: <description>`
- Never commit code that fails `cargo test` or introduces compiler/clippy warnings.
