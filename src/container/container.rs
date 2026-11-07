use arc_swap::ArcSwap;
use std::any::{Any, TypeId};
use std::collections::HashMap;
use std::sync::{Arc, OnceLock};

/// Dynamic factory closure type for resolving bindings.
type BindingFactory = Arc<dyn Fn(&Container) -> Arc<dyn Any + Send + Sync> + Send + Sync>;

/// Callback executed before an item is resolved.
type BeforeResolvingCallback = Arc<dyn Fn(&Container) + Send + Sync>;

/// Callback executed after an item is resolved.
type AfterResolvingCallback = Arc<dyn Fn(&Arc<dyn Any + Send + Sync>, &Container) + Send + Sync>;

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
    /// Callbacks invoked before resolving a type.
    before_resolving: ArcSwap<HashMap<TypeId, Vec<BeforeResolvingCallback>>>,
    /// Callbacks invoked after resolving a type.
    after_resolving: ArcSwap<HashMap<TypeId, Vec<AfterResolvingCallback>>>,
}

impl Container {
    pub fn new() -> Self {
        Self {
            bindings: ArcSwap::from_pointee(HashMap::new()),
            aliases: ArcSwap::from_pointee(HashMap::new()),
            before_resolving: ArcSwap::from_pointee(HashMap::new()),
            after_resolving: ArcSwap::from_pointee(HashMap::new()),
        }
    }

    /// Register a callback to be run before an item of type `T` is resolved.
    pub fn before_resolving<T: 'static, F>(&self, callback: F)
    where
        F: Fn(&Container) + Send + Sync + 'static,
    {
        let mut map = (**self.before_resolving.load()).clone();
        map.entry(TypeId::of::<T>())
            .or_default()
            .push(Arc::new(callback));
        self.before_resolving.store(Arc::new(map));
    }

    /// Register a callback to be run after an item of type `T` is resolved.
    pub fn after_resolving<T: Send + Sync + 'static, F>(&self, callback: F)
    where
        F: Fn(&Arc<T>, &Container) + Send + Sync + 'static,
    {
        let callback_wrapper: AfterResolvingCallback = Arc::new(move |any_arc, container| {
            if let Ok(downcasted) = any_arc.clone().downcast::<T>() {
                callback(&downcasted, container);
            }
        });

        let mut map = (**self.after_resolving.load()).clone();
        map.entry(TypeId::of::<T>())
            .or_default()
            .push(callback_wrapper);
        self.after_resolving.store(Arc::new(map));
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
        // 1. Fire beforeResolving callbacks for this type
        if let Some(callbacks) = self.before_resolving.load().get(type_id) {
            for callback in callbacks {
                callback(self);
            }
        }

        // 2. Resolve or build the instance
        let bindings = self.bindings.load();
        let raw_instance: Arc<dyn Any + Send + Sync> = match bindings.get(type_id) {
            Some(ServiceBinding::Instance(instance)) => Arc::clone(instance),
            Some(ServiceBinding::Singleton { factory, instance }) => {
                let resolved = instance.get_or_init(|| factory(self));
                Arc::clone(resolved)
            }
            Some(ServiceBinding::Factory(factory)) => factory(self),
            None => {
                return Err(format!(
                    "Unbound dependency requested: {:?}",
                    std::any::type_name::<T>()
                ))
            }
        };

        // 3. Fire afterResolving callbacks for this type
        if let Some(callbacks) = self.after_resolving.load().get(type_id) {
            for callback in callbacks {
                callback(&raw_instance, self);
            }
        }

        // 4. Downcast and return typed Arc<T>
        raw_instance.downcast::<T>().map_err(|_| {
            format!(
                "Failed to downcast dependency to target type: {}",
                std::any::type_name::<T>()
            )
        })
    }
}
