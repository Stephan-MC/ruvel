use thiserror::Error;

#[derive(Debug, Error)]
pub enum DatabaseError {
    #[error("Database error: {0}")]
    Sqlx(#[from] sqlx::Error),

    #[error("Serialization/deserialization error: {0}")]
    Serialization(#[from] serde_json::Error),

    #[error("Connection '{0}' not found")]
    ConnectionNotFound(String),

    #[error("No default connection configured")]
    NoDefaultConnection,

    #[error("Model not found: {0}")]
    ModelNotFound(#[from] ModelNotFoundException),

    #[error("Database configuration error: {0}")]
    Configuration(String),

    #[error("Query error: {0}")]
    Query(String),
}

#[derive(Debug, Clone, Error)]
#[error("No query results for model [{model}]")]
pub struct ModelNotFoundException {
    pub model: String,
}

impl ModelNotFoundException {
    pub fn new(model: impl Into<String>) -> Self {
        Self {
            model: model.into(),
        }
    }
}
