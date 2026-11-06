use arc_swap::ArcSwap;
use std::any::{Any, TypeId};
use std::collections::HashMap;
use std::sync::{Arc, OnceLock};

/// Dynamic factory closure type for resolving bindings.
type BindingFactory = Arc<dyn Fn(&Container) -> Arc<dyn Any + Send + Sync> + Send + Sync>;

#[derive(Clone)]
enum ServiceBinding {
    /// Instance binding: pre-constructed instance wrapped in Arc and stored directly into the container.
    Instance(Arc<dyn Any + Send + Sync>),
    /// Lazy singleton binding: instantiated once on the first request and cached for subsequent requests.
    Singleton {
        factory: BindingFactory,
        instance: Arc<OnceLock<Arc<dyn Any + Send + Sync>>>,
    },
    /// Transient factory binding: invoked every time `make::<T>()` is called.
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
    /// Invoked every time `make::<T>()` is resolved.
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

    /// Bind a lazy singleton (equivalent to Laravel's `$container->singleton()`).
    /// Instantiated once on the first resolution and reused across subsequent requests.
    pub fn singleton<T, F>(&self, factory: F)
    where
        T: Send + Sync + 'static,
        F: Fn(&Container) -> T + Send + Sync + 'static,
    {
        let mut map = (**self.bindings.load()).clone();
        let factory_fn: BindingFactory = Arc::new(move |c| Arc::new(factory(c)));
        map.insert(
            TypeId::of::<T>(),
            ServiceBinding::Singleton {
                factory: factory_fn,
                instance: Arc::new(OnceLock::new()),
            },
        );
        self.bindings.store(Arc::new(map));
    }

    /// Save an existing instance into the container (equivalent to Laravel's `$container->instance()`).
    /// Automatically wraps the instance in `Arc::new(...)` before storing.
    pub fn instance<T>(&self, instance: T)
    where
        T: Send + Sync + 'static,
    {
        let mut map = (**self.bindings.load()).clone();
        map.insert(
            TypeId::of::<T>(),
            ServiceBinding::Instance(Arc::new(instance)),
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
            Some(ServiceBinding::Instance(instance)) => {
                instance.clone().downcast::<T>().map_err(|_| {
                    format!(
                        "Failed to downcast dependency to target type: {}",
                        std::any::type_name::<T>()
                    )
                })
            }
            Some(ServiceBinding::Singleton { factory, instance }) => {
                let resolved = instance.get_or_init(|| factory(self));
                resolved.clone().downcast::<T>().map_err(|_| {
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
