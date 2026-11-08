use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;

pub type BoxFuture<'a, T> = Pin<Box<dyn Future<Output = T> + Send + 'a>>;

pub type DestinationFn<T, R> = Arc<dyn Fn(T) -> BoxFuture<'static, R> + Send + Sync + 'static>;

/// Trait representing an onion-layer pipe.
///
/// Each pipe receives the passable data and a `Next` runner to call the next layer.
/// It can perform operations before and after `next.run(passable).await`.
pub trait Pipe<T, R = T>: Send + Sync + 'static {
    fn handle<'a>(&'a self, passable: T, next: Next<T, R>) -> BoxFuture<'a, R>;
}

/// Automatically allow `Arc<P>` to implement `Pipe<T, R>` where `P: Pipe<T, R> + ?Sized`.
impl<T, R, P> Pipe<T, R> for Arc<P>
where
    P: Pipe<T, R> + ?Sized,
{
    fn handle<'a>(&'a self, passable: T, next: Next<T, R>) -> BoxFuture<'a, R> {
        (**self).handle(passable, next)
    }
}

/// Allows closures `Fn(T, Next<T, R>) -> BoxFuture<'static, R>` to implement `Pipe<T, R>`.
impl<T, R, F> Pipe<T, R> for F
where
    T: Send + 'static,
    R: Send + 'static,
    F: Fn(T, Next<T, R>) -> BoxFuture<'static, R> + Send + Sync + 'static,
{
    fn handle<'a>(&'a self, passable: T, next: Next<T, R>) -> BoxFuture<'a, R> {
        (self)(passable, next)
    }
}

/// Represents the next stage in the pipeline onion layers.
pub struct Next<T, R> {
    pub(crate) pipes: Arc<[Arc<dyn Pipe<T, R>>]>,
    pub(crate) index: usize,
    pub(crate) destination: DestinationFn<T, R>,
}

impl<T: Send + 'static, R: Send + 'static> Next<T, R> {
    pub async fn run(self, passable: T) -> R {
        if self.index < self.pipes.len() {
            let pipe = Arc::clone(&self.pipes[self.index]);
            let next = Next {
                pipes: self.pipes,
                index: self.index + 1,
                destination: self.destination,
            };
            pipe.handle(passable, next).await
        } else {
            (self.destination)(passable).await
        }
    }
}
