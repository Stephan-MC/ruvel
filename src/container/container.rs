use arc_swap::ArcSwap;
use std::any::{Any, TypeId};
use std::collections::HashMap;
use std::sync::Arc;

/// Dynamic factory closure type for resolving transient bindings.
type BindingFactory = Arc<dyn Fn(&Container) -> Arc<dyn Any + Send + Sync> + Send + Sync>;

#[derive(Clone)]
enum ServiceBinding {
    /// Singleton instance: evaluated once and stored.
    Singleton(Arc<dyn Any + Send + Sync>),
    /// Transient factory: invoked every time `make::<T>()` is called.
    Factory(BindingFactory),
}

/// The core Container struct managing IoC resolution in `ruvel`.
#[derive(Default)]
pub struct Container {
    /// Immutable, lock-free lookup table swapping atomic pointers for request-time reads.
    bindings: ArcSwap<HashMap<TypeId, ServiceBinding>>,
    /// Named string aliases for abstract types (e.g., "db" -> TypeId of DatabasePool).
    aliases: ArcSwap<HashMap<String, TypeId>>,
}

impl Container {
    pub fn new() -> Self {
        Self {
            bindings: ArcSwap::from_pointee(HashMap::new()),
            aliases: ArcSwap::from_pointee(HashMap::new()),
        }
    }

    /// Bind a transient factory (equivalent to Laravel's `$container->bind()`).
    pub fn bind<T, F>(&self, factory: F)
    where
        T: Send + Sync + 'static,
        F: Fn(&Container) -> T + Send + Sync + 'static,
    {
        let mut map = (**self.bindings.load()).clone();
        let factory_fn: BindingFactory = Arc::new(move |c| Arc::new(factory(c)));
        map.insert(TypeId::of::<T>(), ServiceBinding::Factory(factory_fn));
        self.bindings.store(Arc::new(map));
    }

    /// Bind a pre-constructed singleton instance (equivalent to Laravel's `$container->singleton()`).
    pub fn singleton<T>(&self, instance: T)
    where
        T: Send + Sync + 'static,
    {
        let mut map = (**self.bindings.load()).clone();
        map.insert(
            TypeId::of::<T>(),
            ServiceBinding::Singleton(Arc::new(instance)),
        );
        self.bindings.store(Arc::new(map));
    }

    /// Register a string alias for a concrete type (e.g., `$container->alias('db', Database::class)`).
    pub fn alias<T: 'static>(&self, alias_name: &str) {
        let mut aliases = (**self.aliases.load()).clone();
        aliases.insert(alias_name.to_string(), TypeId::of::<T>());
        self.aliases.store(Arc::new(aliases));
    }

    /// Resolve an instance of type T (equivalent to Laravel's `$container->make()`).
    pub fn make<T: Send + Sync + 'static>(&self) -> Result<Arc<T>, String> {
        let type_id = TypeId::of::<T>();
        self.resolve_by_type_id(&type_id)
    }

    /// Resolve an instance using a string alias.
    pub fn make_alias<T: Send + Sync + 'static>(&self, alias_name: &str) -> Result<Arc<T>, String> {
        let aliases = self.aliases.load();
        let type_id = aliases
            .get(alias_name)
            .ok_or_else(|| format!("Alias '{}' not found in container", alias_name))?;

        self.resolve_by_type_id(type_id)
    }

    fn resolve_by_type_id<T: Send + Sync + 'static>(
        &self,
        type_id: &TypeId,
    ) -> Result<Arc<T>, String> {
        let bindings = self.bindings.load();

        match bindings.get(type_id) {
            Some(ServiceBinding::Singleton(instance)) => {
                instance.clone().downcast::<T>().map_err(|_| {
                    format!(
                        "Failed to downcast dependency to target type: {}",
                        std::any::type_name::<T>()
                    )
                })
            }
            Some(ServiceBinding::Factory(factory)) => {
                let raw_instance = factory(self);
                raw_instance.downcast::<T>().map_err(|_| {
                    format!(
                        "Failed to downcast dependency to target type: {}",
                        std::any::type_name::<T>()
                    )
                })
            }
            None => Err(format!(
                "Unbound dependency requested: {:?}",
                std::any::type_name::<T>()
            )),
        }
    }
}
