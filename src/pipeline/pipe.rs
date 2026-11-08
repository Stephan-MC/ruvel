use crate::contracts::pipeline::Pipeable;
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;

pub type BoxFuture<'a, T> = Pin<Box<dyn Future<Output = T> + Send + 'a>>;

pub type DestinationFn<T, R> = Arc<dyn Fn(T) -> BoxFuture<'static, R> + Send + Sync + 'static>;

/// Helper wrapper allowing closures `Fn(T, Next<T, R>) -> BoxFuture<'static, R>` to be used as `Pipeable<T, R>`.
pub struct FnPipe<F>(pub F);

impl<T, R, F> Pipeable<T, R> for FnPipe<F>
where
    T: Send + 'static,
    R: Send + 'static,
    F: Fn(T, Next<T, R>) -> BoxFuture<'static, R> + Send + Sync + 'static,
{
    fn handle<'a>(&'a self, passable: T, next: Next<T, R>) -> BoxFuture<'a, R> {
        (self.0)(passable, next)
    }
}

/// Represents the next stage in the pipeline onion layers.
pub struct Next<T, R> {
    pub(crate) pipes: Arc<[Arc<dyn Pipeable<T, R>>]>,
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
