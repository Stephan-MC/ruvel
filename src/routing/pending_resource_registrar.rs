use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use super::into_middleware::{IntoMiddleware, MiddlewareItem};
use super::route::{HandlerFn, Route};
use super::router::Router;
use crate::contracts::support::Responsable;
use crate::http::{Controller, Method};

/// The RESTful actions supported by a resource controller.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ResourceAction {
    Index,
    Create,
    Store,
    Show,
    Edit,
    Update,
    Destroy,
}

/// Helper trait allowing both `ResourceAction` and `&str` to be passed into `.only()` and `.except()`.
pub trait IntoResourceAction {
    fn to_resource_action(&self) -> Option<ResourceAction>;
}

impl IntoResourceAction for ResourceAction {
    fn to_resource_action(&self) -> Option<ResourceAction> {
        Some(*self)
    }
}

impl IntoResourceAction for &str {
    fn to_resource_action(&self) -> Option<ResourceAction> {
        match self.to_lowercase().as_str() {
            "index" => Some(ResourceAction::Index),
            "create" => Some(ResourceAction::Create),
            "store" => Some(ResourceAction::Store),
            "show" => Some(ResourceAction::Show),
            "edit" => Some(ResourceAction::Edit),
            "update" => Some(ResourceAction::Update),
            "destroy" | "delete" => Some(ResourceAction::Destroy),
            _ => None,
        }
    }
}

impl IntoResourceAction for String {
    fn to_resource_action(&self) -> Option<ResourceAction> {
        self.as_str().to_resource_action()
    }
}

struct ResourceHandlers {
    index: HandlerFn,
    create: HandlerFn,
    store: HandlerFn,
    show: HandlerFn,
    edit: HandlerFn,
    update: HandlerFn,
    destroy: HandlerFn,
}

/// A fluent builder for configuring and filtering resource controller routes before
/// committing them to the underlying [`Router`].
pub struct PendingResourceRegistration<'a> {
    router: &'a mut Router,
    resource: String,
    base_prefix: Option<String>,
    base_name: Option<String>,
    custom_name: Option<String>,
    parameter_name: String,
    domain: Option<String>,
    middleware: Vec<MiddlewareItem>,
    excluded_middleware: Vec<String>,
    wheres: HashMap<String, String>,
    actions: HashSet<ResourceAction>,
    handlers: ResourceHandlers,
    committed: bool,
}

impl<'a> PendingResourceRegistration<'a> {
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn new<C: Controller + 'static>(
        router: &'a mut Router,
        resource: &str,
        controller: C,
        base_prefix: Option<String>,
        base_name: Option<String>,
        domain: Option<String>,
        middleware: Vec<MiddlewareItem>,
        excluded_middleware: Vec<String>,
        wheres: HashMap<String, String>,
    ) -> Self {
        let c = Arc::new(controller);

        let index_c = Arc::clone(&c);
        let index: HandlerFn = Arc::new(move |req| {
            let c = Arc::clone(&index_c);
            Box::pin(async move { c.index(req).await.to_response() })
        });

        let create_c = Arc::clone(&c);
        let create: HandlerFn = Arc::new(move |req| {
            let c = Arc::clone(&create_c);
            Box::pin(async move { c.create(req).await.to_response() })
        });

        let store_c = Arc::clone(&c);
        let store: HandlerFn = Arc::new(move |req| {
            let c = Arc::clone(&store_c);
            Box::pin(async move { c.store(req).await.to_response() })
        });

        let show_c = Arc::clone(&c);
        let show: HandlerFn = Arc::new(move |req| {
            let c = Arc::clone(&show_c);
            Box::pin(async move { c.show(req).await.to_response() })
        });

        let edit_c = Arc::clone(&c);
        let edit: HandlerFn = Arc::new(move |req| {
            let c = Arc::clone(&edit_c);
            Box::pin(async move { c.edit(req).await.to_response() })
        });

        let update_c = Arc::clone(&c);
        let update: HandlerFn = Arc::new(move |req| {
            let c = Arc::clone(&update_c);
            Box::pin(async move { c.update(req).await.to_response() })
        });

        let destroy_c = Arc::clone(&c);
        let destroy: HandlerFn = Arc::new(move |req| {
            let c = Arc::clone(&destroy_c);
            Box::pin(async move { c.destroy(req).await.to_response() })
        });

        let mut actions = HashSet::new();
        actions.insert(ResourceAction::Index);
        actions.insert(ResourceAction::Create);
        actions.insert(ResourceAction::Store);
        actions.insert(ResourceAction::Show);
        actions.insert(ResourceAction::Edit);
        actions.insert(ResourceAction::Update);
        actions.insert(ResourceAction::Destroy);

        Self {
            router,
            resource: resource.to_string(),
            base_prefix,
            base_name,
            custom_name: None,
            parameter_name: "id".to_string(),
            domain,
            middleware,
            excluded_middleware,
            wheres,
            actions,
            handlers: ResourceHandlers {
                index,
                create,
                store,
                show,
                edit,
                update,
                destroy,
            },
            committed: false,
        }
    }

    /// Limit the registered resource routes to the specified actions.
    pub fn only<A: IntoResourceAction>(mut self, actions: &[A]) -> Self {
        let set: HashSet<ResourceAction> = actions
            .iter()
            .filter_map(|a| a.to_resource_action())
            .collect();
        self.actions.retain(|a| set.contains(a));
        self
    }

    /// Exclude the specified actions from the registered resource routes.
    pub fn except<A: IntoResourceAction>(mut self, actions: &[A]) -> Self {
        let set: HashSet<ResourceAction> = actions
            .iter()
            .filter_map(|a| a.to_resource_action())
            .collect();
        self.actions.retain(|a| !set.contains(a));
        self
    }

    /// Override the resource route parameter name in paths (defaults to `"id"`).
    pub fn parameter(mut self, param: impl Into<String>) -> Self {
        self.parameter_name = param.into();
        self
    }

    /// Override the route name prefix for all resource actions (defaults to the resource name).
    pub fn names(mut self, name: impl Into<String>) -> Self {
        self.custom_name = Some(name.into());
        self
    }

    /// Attach middleware to all routes in this resource.
    pub fn middleware(mut self, middleware: impl IntoMiddleware) -> Self {
        self.middleware.extend(middleware.into_middleware_items());
        self
    }

    /// Exclude middleware aliases from all routes in this resource.
    pub fn without_middleware(mut self, middleware: impl IntoMiddleware) -> Self {
        for item in middleware.into_middleware_items() {
            if let MiddlewareItem::Alias(name) = item {
                self.excluded_middleware.push(name);
            }
        }
        self
    }

    /// Commit all active resource routes into the router immediately.
    pub fn commit(&mut self) {
        if self.committed {
            return;
        }
        self.committed = true;

        let compiled_middleware = self
            .router
            .compile_route_middlewares(&self.middleware, &self.excluded_middleware);

        let param_suffix = format!("{{{}}}", self.parameter_name);
        let edit_suffix = format!("{{{}}}/edit", self.parameter_name);

        let base_name = self
            .custom_name
            .clone()
            .unwrap_or_else(|| self.resource.trim_matches('/').replace('/', "."));

        let make_route_name = |action: &str| -> String {
            match &self.base_name {
                Some(p) => {
                    if p.ends_with('.') || p.ends_with('-') {
                        format!("{}{}.{}", p, base_name, action)
                    } else {
                        format!("{}.{}.{}", p, base_name, action)
                    }
                }
                None => format!("{}.{}", base_name, action),
            }
        };

        let route_specs = [
            (ResourceAction::Index, vec![Method::GET], None, "index"),
            (
                ResourceAction::Create,
                vec![Method::GET],
                Some("create"),
                "create",
            ),
            (ResourceAction::Store, vec![Method::POST], None, "store"),
            (
                ResourceAction::Show,
                vec![Method::GET],
                Some(param_suffix.as_str()),
                "show",
            ),
            (
                ResourceAction::Edit,
                vec![Method::GET],
                Some(edit_suffix.as_str()),
                "edit",
            ),
            (
                ResourceAction::Update,
                vec![Method::PUT, Method::PATCH],
                Some(param_suffix.as_str()),
                "update",
            ),
            (
                ResourceAction::Destroy,
                vec![Method::DELETE],
                Some(param_suffix.as_str()),
                "destroy",
            ),
        ];

        for (action, methods, suffix, name_suffix) in route_specs {
            if !self.actions.contains(&action) {
                continue;
            }

            let handler = match action {
                ResourceAction::Index => Arc::clone(&self.handlers.index),
                ResourceAction::Create => Arc::clone(&self.handlers.create),
                ResourceAction::Store => Arc::clone(&self.handlers.store),
                ResourceAction::Show => Arc::clone(&self.handlers.show),
                ResourceAction::Edit => Arc::clone(&self.handlers.edit),
                ResourceAction::Update => Arc::clone(&self.handlers.update),
                ResourceAction::Destroy => Arc::clone(&self.handlers.destroy),
            };

            let path = build_path(self.base_prefix.as_deref(), &self.resource, suffix);
            let path = if path != "/" {
                path.trim_end_matches('/').to_string()
            } else {
                path
            };

            let route_name = make_route_name(name_suffix);

            let route = Route::with_details(
                handler,
                compiled_middleware.clone(),
                Some(route_name),
                self.domain.clone(),
                self.wheres.clone(),
            );

            self.router.insert_route_for_methods(&methods, &path, route);
        }
    }
}

impl<'a> Drop for PendingResourceRegistration<'a> {
    fn drop(&mut self) {
        self.commit();
    }
}

fn build_path(prefix: Option<&str>, resource: &str, suffix: Option<&str>) -> String {
    let mut parts: Vec<&str> = Vec::new();

    if let Some(p) = prefix {
        let trimmed_p = p.trim_matches('/');
        if !trimmed_p.is_empty() {
            parts.push(trimmed_p);
        }
    }

    let trimmed_r = resource.trim_matches('/');
    if !trimmed_r.is_empty() {
        parts.push(trimmed_r);
    }

    if let Some(s) = suffix {
        let trimmed_s = s.trim_matches('/');
        if !trimmed_s.is_empty() {
            parts.push(trimmed_s);
        }
    }

    if parts.is_empty() {
        "/".to_string()
    } else {
        format!("/{}", parts.join("/"))
    }
}
