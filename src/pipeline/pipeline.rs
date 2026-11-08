use super::pipe::{DestinationFn, Next};
use crate::contracts::pipeline::Pipeable;
use std::future::Future;
use std::sync::Arc;

/// Trait converting items to `Arc<dyn Pipeable<T, R>>` avoiding duplicate Arc allocations.
pub trait IntoPipeable<T, R> {
    fn into_pipeable(self) -> Arc<dyn Pipeable<T, R>>;
}

impl<T: Send + 'static, R: Send + 'static> IntoPipeable<T, R> for Arc<dyn Pipeable<T, R>> {
    #[inline]
    fn into_pipeable(self) -> Arc<dyn Pipeable<T, R>> {
        self
    }
}

/// Pipeline builder that passes a payload through a sequence of pipes (middleware layers).
pub struct Pipeline<T, R = T> {
    passable: Option<T>,
    pipes: Vec<Arc<dyn Pipeable<T, R>>>,
}

impl<T: Send + 'static, R: Send + 'static> Default for Pipeline<T, R> {
    fn default() -> Self {
        Self::new()
    }
}

impl<T: Send + 'static, R: Send + 'static> Pipeline<T, R> {
    /// Create a new Pipeline instance.
    pub fn new() -> Self {
        Self {
            passable: None,
            pipes: Vec::new(),
        }
    }

    /// Set the object being sent through the pipeline.
    pub fn send(mut self, passable: T) -> Self {
        self.passable = Some(passable);
        self
    }

    /// Set the array of pipes.
    pub fn through<I, P>(mut self, pipes: I) -> Self
    where
        I: IntoIterator<Item = P>,
        P: Pipeable<T, R> + 'static,
    {
        self.pipes = pipes
            .into_iter()
            .map(|p| -> Arc<dyn Pipeable<T, R>> {
                // If already an Arc<P>, the compiler sees P implements Pipeable
                // and Into/Arc cleanly encapsulates it.
                Arc::new(p)
            })
            .collect();
        self
    }

    /// Add a single pipe to the pipeline.
    pub fn pipe<P>(mut self, pipe: P) -> Self
    where
        P: Pipeable<T, R> + 'static,
    {
        self.pipes.push(Arc::new(pipe));
        self
    }

    /// Run the pipeline with a final destination callback and get the result.
    pub async fn then<F, Fut>(self, destination: F) -> R
    where
        F: Fn(T) -> Fut + Send + Sync + 'static,
        Fut: Future<Output = R> + Send + 'static,
    {
        let passable = self
            .passable
            .expect("Cannot execute pipeline without setting a passable payload via send()");

        let destination_fn: DestinationFn<T, R> =
            Arc::new(move |passable| Box::pin(destination(passable)));

        if self.pipes.is_empty() {
            return (destination_fn)(passable).await;
        }

        let next = Next {
            pipes: self.pipes.into(),
            index: 0,
            destination: destination_fn,
        };

        next.run(passable).await
    }
}

impl<T: Send + 'static> Pipeline<T, T> {
    /// Run the pipeline and return the resulting passable directly.
    /// Equivalent to Laravel's `$pipeline->thenReturn()`.
    pub async fn then_return(self) -> T {
        self.then(|passable| async move { passable }).await
    }
}
