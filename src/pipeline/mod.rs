pub mod pipe;
pub mod pipeline;

pub use crate::contracts::pipeline::Pipeable;
pub use pipe::{BoxFuture, DestinationFn, FnPipe, Next};
pub use pipeline::Pipeline;
