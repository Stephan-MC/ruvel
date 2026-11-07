use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;

pub type BoxFuture<'a, T> = Pin<Box<dyn Future<Output = T> + Send + 'a>>;

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

pub type DestinationFn<T, R> = Arc<dyn Fn(T) -> BoxFuture<'static, R> + Send + Sync + 'static>;

/// Represents the next stage in the pipeline onion layers.
pub struct Next<T, R> {
    pipes: Arc<[Arc<dyn Pipe<T, R>>]>,
    index: usize,
    destination: DestinationFn<T, R>,
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

/// Pipeline builder that passes a payload through a sequence of pipes (middleware layers).
pub struct Pipeline<T, R = T> {
    passable: Option<T>,
    pipes: Vec<Arc<dyn Pipe<T, R>>>,
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
        P: Pipe<T, R> + 'static,
    {
        self.pipes = pipes
            .into_iter()
            .map(|p| -> Arc<dyn Pipe<T, R>> { Arc::new(p) })
            .collect();
        self
    }

    /// Add a single pipe to the pipeline.
    pub fn pipe<P>(mut self, pipe: P) -> Self
    where
        P: Pipe<T, R> + 'static,
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
