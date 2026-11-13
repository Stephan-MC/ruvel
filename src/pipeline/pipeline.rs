use super::pipe::{Next, StepFn};
use crate::contracts::pipeline::Pipeable;
use std::future::Future;
use std::sync::Arc;

/// Pipeline builder that passes a payload through a sequence of pipes (middleware layers).
pub struct Pipeline<T, R = T> {
    passable: Option<Box<T>>,
    pipes: Vec<Arc<dyn Pipeable<T, R>>>,
}

impl<T, R> Clone for Pipeline<T, R> {
    fn clone(&self) -> Self {
        Self {
            passable: None,
            pipes: self.pipes.clone(),
        }
    }
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

    /// Returns `true` if the pipeline contains no pipes.
    pub fn is_empty(&self) -> bool {
        self.pipes.is_empty()
    }

    /// Set the object being sent through the pipeline.
    pub fn send(mut self, passable: T) -> Self {
        self.passable = Some(Box::new(passable));
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

    /// Prepend a collection of pipes to the front of the current pipes.
    pub fn prepend<I, P>(mut self, pipes: I) -> Self
    where
        I: IntoIterator<Item = P>,
        P: Pipeable<T, R> + 'static,
    {
        let prepended: Vec<Arc<dyn Pipeable<T, R>>> = pipes
            .into_iter()
            .map(|p| -> Arc<dyn Pipeable<T, R>> { Arc::new(p) })
            .collect();

        self.pipes.splice(0..0, prepended);

        self
    }

    /// Append a collection of pipes to the end of the current pipes.
    pub fn append<I, P>(mut self, pipes: I) -> Self
    where
        I: IntoIterator<Item = P>,
        P: Pipeable<T, R> + 'static,
    {
        self.pipes.extend(
            pipes
                .into_iter()
                .map(|p| -> Arc<dyn Pipeable<T, R>> { Arc::new(p) }),
        );

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

    /// Compose all pipes around `destination` into a single reusable step.
    ///
    /// Returns `destination` untouched when the pipeline contains no pipes.
    pub(crate) fn compose(&self, destination: StepFn<T, R>) -> StepFn<T, R> {
        self.pipes.iter().rev().fold(destination, |next, pipe| {
            let pipe = Arc::clone(pipe);
            Arc::new(move |passable| {
                let p = Arc::clone(&pipe);
                let n = Next::new(Arc::clone(&next));
                Box::pin(async move { p.handle(passable, n).await })
            })
        })
    }

    /// Run the pipeline with a final destination callback and get the result.
    pub async fn then<F, Fut>(mut self, destination: F) -> R
    where
        F: Fn(T) -> Fut + Send + Sync + 'static,
        Fut: Future<Output = R> + Send + 'static,
    {
        let passable = *self
            .passable
            .take()
            .expect("Cannot execute pipeline without setting a passable payload via send()");

        let dest: StepFn<T, R> = Arc::new(move |passable| Box::pin(destination(passable)));
        let chain = self.compose(dest);
        chain(passable).await
    }
}

impl<T: Send + 'static> Pipeline<T, T> {
    /// Run the pipeline and return the resulting passable directly.
    /// Equivalent to Laravel's `$pipeline->thenReturn()`.
    pub async fn then_return(self) -> T {
        self.then(|passable| async move { passable }).await
    }
}
