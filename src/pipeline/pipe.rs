use crate::contracts::pipeline::Pipeable;
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;

pub type BoxFuture<'a, T> = Pin<Box<dyn Future<Output = T> + Send + 'a>>;

pub type StepFn<T, R> = Arc<dyn Fn(T) -> BoxFuture<'static, R> + Send + Sync + 'static>;

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
/// It contains only the step callback, with minimal runtime footprint.
pub struct Next<T, R> {
    pub(crate) step: StepFn<T, R>,
}

impl<T: Send + 'static, R: Send + 'static> Next<T, R> {
    pub fn new(step: StepFn<T, R>) -> Self {
        Self { step }
    }

    #[inline]
    pub async fn run(self, passable: T) -> R {
        (self.step)(passable).await
    }
}
