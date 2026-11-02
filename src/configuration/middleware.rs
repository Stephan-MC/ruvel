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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::http::{BoxFuture, Next, Request, Response};

    struct M1;
    struct M2;
    struct M3;
    struct Replacement;

    impl HttpMiddleware for M1 {
        fn handle<'a>(&'a self, request: Request, next: Next<'a>) -> BoxFuture<'a, Response> {
            Box::pin(async move { next.run(request).await })
        }
    }

    impl HttpMiddleware for M2 {
        fn handle<'a>(&'a self, request: Request, next: Next<'a>) -> BoxFuture<'a, Response> {
            Box::pin(async move { next.run(request).await })
        }
    }

    impl HttpMiddleware for M3 {
        fn handle<'a>(&'a self, request: Request, next: Next<'a>) -> BoxFuture<'a, Response> {
            Box::pin(async move { next.run(request).await })
        }
    }

    impl HttpMiddleware for Replacement {
        fn handle<'a>(&'a self, request: Request, next: Next<'a>) -> BoxFuture<'a, Response> {
            Box::pin(async move { next.run(request).await })
        }
    }

    #[test]
    fn test_prepend_and_append_ordering() {
        let mut mw = Middleware::new();
        mw.append(M2);
        mw.prepend(M1);
        mw.append(M3);

        let resolved = mw.resolve();
        assert_eq!(resolved.global.len(), 3);
    }

    #[test]
    fn test_removal() {
        let mut mw = Middleware::new();
        mw.append(M1);
        mw.append(M2);
        mw.append(M3);
        mw.remove::<M2>();

        let resolved = mw.resolve();
        assert_eq!(resolved.global.len(), 2);
    }

    #[test]
    fn test_replacement() {
        let mut mw = Middleware::new();
        mw.append(M1);
        mw.append(M2);
        mw.replace::<M2, Replacement>(Replacement);

        let resolved = mw.resolve();
        assert_eq!(resolved.global.len(), 2);
    }

    #[test]
    fn test_groups_and_aliases() {
        let mut mw = Middleware::new();
        mw.group("api", vec![Arc::new(M1), Arc::new(M2)]);
        mw.alias("auth", M3);

        let resolved = mw.resolve();
        assert_eq!(resolved.groups.get("api").unwrap().len(), 2);
        assert!(resolved.aliases.contains_key("auth"));
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
