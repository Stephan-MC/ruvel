use super::pipe::{Next, StepFn};
use crate::contracts::pipeline::Pipeable;
use std::future::Future;
use std::sync::Arc;

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
            .map(|p| -> Arc<dyn Pipeable<T, R>> { Arc::new(p) })
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

        let mut current_step: StepFn<T, R> =
            Arc::new(move |passable| Box::pin(destination(passable)));

        // Compose the onion layers in reverse order: innermost layer wraps destination,
        // outermost layer is called first (identical to Laravel's array_reduce pipeline).
        for pipe in self.pipes.into_iter().rev() {
            let next_step = current_step;
            current_step = Arc::new(move |passable| {
                let p = Arc::clone(&pipe);
                let n = Next::new(Arc::clone(&next_step));
                Box::pin(async move { p.handle(passable, n).await })
            });
        }

        current_step(passable).await
    }
}

impl<T: Send + 'static> Pipeline<T, T> {
    /// Run the pipeline and return the resulting passable directly.
    /// Equivalent to Laravel's `$pipeline->thenReturn()`.
    pub async fn then_return(self) -> T {
        self.then(|passable| async move { passable }).await
    }
}
