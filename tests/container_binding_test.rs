use ruvel::container::Container;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
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

#[test]
fn test_before_and_after_resolving() {
    let container = Container::new();
    container.instance(Service { id: 100 });

    let before_called = Arc::new(AtomicBool::new(false));
    let after_called = Arc::new(AtomicBool::new(false));
    let after_val = Arc::new(AtomicUsize::new(0));

    let b_clone = Arc::clone(&before_called);
    container.before_resolving::<Service, _>(move |_| {
        b_clone.store(true, Ordering::SeqCst);
    });

    let a_clone = Arc::clone(&after_called);
    let val_clone = Arc::clone(&after_val);
    container.after_resolving::<Service, _>(move |service, _| {
        a_clone.store(true, Ordering::SeqCst);
        val_clone.store(service.id, Ordering::SeqCst);
    });

    assert!(!before_called.load(Ordering::SeqCst));
    assert!(!after_called.load(Ordering::SeqCst));

    let resolved = container.make::<Service>().unwrap();

    assert_eq!(resolved.id, 100);
    assert!(before_called.load(Ordering::SeqCst));
    assert!(after_called.load(Ordering::SeqCst));
    assert_eq!(after_val.load(Ordering::SeqCst), 100);
}
