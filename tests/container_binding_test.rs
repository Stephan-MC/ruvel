use ruvel::container::Container;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

struct Service {
    id: usize,
}

#[test]
fn test_instance_binding() {
    let container = Container::new();
    container.instance(Service { id: 42 });

    let s1 = container.make::<Service>().unwrap();
    let s2 = container.make::<Service>().unwrap();

    assert_eq!(s1.id, 42);
    assert!(Arc::ptr_eq(&s1, &s2));
}

#[test]
fn test_lazy_singleton_binding() {
    let container = Container::new();
    let counter = Arc::new(AtomicUsize::new(0));

    let c_clone = Arc::clone(&counter);
    container.singleton(move |_| {
        let count = c_clone.fetch_add(1, Ordering::SeqCst);
        Service { id: count }
    });

    // Not instantiated yet
    assert_eq!(counter.load(Ordering::SeqCst), 0);

    let s1 = container.make::<Service>().unwrap();
    assert_eq!(s1.id, 0);
    assert_eq!(counter.load(Ordering::SeqCst), 1);

    // Reused instance
    let s2 = container.make::<Service>().unwrap();
    assert_eq!(s2.id, 0);
    assert_eq!(counter.load(Ordering::SeqCst), 1);
    assert!(Arc::ptr_eq(&s1, &s2));
}

#[test]
fn test_factory_transient_binding() {
    let container = Container::new();
    let counter = Arc::new(AtomicUsize::new(0));

    let c_clone = Arc::clone(&counter);
    container.bind(move |_| {
        let count = c_clone.fetch_add(1, Ordering::SeqCst);
        Service { id: count }
    });

    let s1 = container.make::<Service>().unwrap();
    assert_eq!(s1.id, 0);
    assert_eq!(counter.load(Ordering::SeqCst), 1);

    let s2 = container.make::<Service>().unwrap();
    assert_eq!(s2.id, 1);
    assert_eq!(counter.load(Ordering::SeqCst), 2);
    assert!(!Arc::ptr_eq(&s1, &s2));
}
