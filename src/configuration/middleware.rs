use std::any::TypeId;
use std::collections::HashMap;
use std::sync::Arc;

use crate::http::Middleware as HttpMiddleware;

struct Layer {
    id: TypeId,
    middleware: Arc<dyn HttpMiddleware>,
}

impl Layer {
    fn new<M: HttpMiddleware + 'static>(middleware: M) -> Self {
        Self {
            id: TypeId::of::<M>(),
            middleware: Arc::new(middleware),
        }
    }
}

/// Laravel-style middleware configuration.
///
/// Global middleware run on every request, from the outside in:
/// `prepend`s, then the base stack, then `append`s. `remove` and `replace`
/// apply to that global stack. Groups and aliases are attached to routes.
#[derive(Default)]
pub struct Middleware {
    global: Vec<Layer>,
    groups: HashMap<String, Vec<Arc<dyn HttpMiddleware>>>,
    aliases: HashMap<String, Arc<dyn HttpMiddleware>>,
    appends: Vec<Layer>,
    prepends: Vec<Layer>,
    removals: Vec<TypeId>,
    replacements: Vec<(TypeId, Layer)>,
}

/// Middleware installed on the router.
pub(crate) struct ResolvedMiddleware {
    pub global: Vec<Arc<dyn HttpMiddleware>>,
    pub groups: HashMap<String, Vec<Arc<dyn HttpMiddleware>>>,
    pub aliases: HashMap<String, Arc<dyn HttpMiddleware>>,
}

impl Middleware {
    pub fn new() -> Self {
        Self::default()
    }

    /// Add middleware to the end of the global stack.
    pub fn append(&mut self, middleware: impl HttpMiddleware + 'static) -> &mut Self {
        self.appends.push(Layer::new(middleware));
        self
    }

    /// Add middleware to the front of the global stack.
    ///
    /// The most recently prepended layer runs first.
    pub fn prepend(&mut self, middleware: impl HttpMiddleware + 'static) -> &mut Self {
        self.prepends.insert(0, Layer::new(middleware));
        self
    }

    /// Drop every global layer of type `T`.
    pub fn remove<T: HttpMiddleware + 'static>(&mut self) -> &mut Self {
        self.removals.push(TypeId::of::<T>());
        self
    }

    /// Replace every global layer of type `T` with `middleware`.
    pub fn replace<T, R>(&mut self, middleware: R) -> &mut Self
    where
        T: HttpMiddleware + 'static,
        R: HttpMiddleware + 'static,
    {
        self.replacements
            .push((TypeId::of::<T>(), Layer::new(middleware)));
        self
    }

    /// Register a named group. A group expands to these layers, in order.
    pub fn group(&mut self, group: &str, middlewares: Vec<Arc<dyn HttpMiddleware>>) -> &mut Self {
        self.groups.insert(group.to_string(), middlewares);
        self
    }

    /// Register a single middleware under a route name.
    pub fn alias(&mut self, name: &str, middleware: impl HttpMiddleware + 'static) -> &mut Self {
        self.aliases.insert(name.to_string(), Arc::new(middleware));
        self
    }

    pub(crate) fn resolve(self) -> ResolvedMiddleware {
        let mut stack = self.prepends;
        stack.extend(self.global);
        stack.extend(self.appends);

        stack.retain(|layer| !self.removals.contains(&layer.id));

        for layer in &mut stack {
            if let Some((_, replacement)) = self
                .replacements
                .iter()
                .rev()
                .find(|(id, _)| *id == layer.id)
            {
                *layer = Layer {
                    id: replacement.id,
                    middleware: Arc::clone(&replacement.middleware),
                };
            }
        }

        ResolvedMiddleware {
            global: stack.into_iter().map(|layer| layer.middleware).collect(),
            groups: self.groups,
            aliases: self.aliases,
        }
    }
}
