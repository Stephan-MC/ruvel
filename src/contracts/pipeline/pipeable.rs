use crate::pipeline::Next;
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;

pub type BoxFuture<'a, T> = Pin<Box<dyn Future<Output = T> + Send + 'a>>;

/// Trait representing an onion-layer pipeable item.
///
/// Each pipeable receives the passable data and a `Next` runner to call the next layer.
/// It can perform operations before and after `next.run(passable).await`.
pub trait Pipeable<T, R = T>: Send + Sync + 'static {
    fn handle<'a>(&'a self, passable: T, next: Next<T, R>) -> BoxFuture<'a, R>;
}

/// Automatically allow `Arc<P>` to implement `Pipeable<T, R>` where `P: Pipeable<T, R> + ?Sized`.
impl<T, R, P> Pipeable<T, R> for Arc<P>
where
    P: Pipeable<T, R> + ?Sized,
{
    fn handle<'a>(&'a self, passable: T, next: Next<T, R>) -> BoxFuture<'a, R> {
        (**self).handle(passable, next)
    }
}
