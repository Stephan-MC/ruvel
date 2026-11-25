use std::fmt;

#[derive(Debug)]
pub enum DatabaseError {
    Sqlx(sqlx::Error),
    Serialization(serde_json::Error),
    ConnectionNotFound(String),
    NoDefaultConnection,
    ModelNotFound(ModelNotFoundException),
    Configuration(String),
    Query(String),
}

impl fmt::Display for DatabaseError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Sqlx(err) => write!(f, "Database error: {err}"),
            Self::Serialization(err) => write!(f, "Serialization/deserialization error: {err}"),
            Self::ConnectionNotFound(name) => write!(f, "Connection '{name}' not found"),
            Self::NoDefaultConnection => f.write_str("No default connection configured"),
            Self::ModelNotFound(err) => write!(f, "Model not found: {err}"),
            Self::Configuration(msg) => write!(f, "Database configuration error: {msg}"),
            Self::Query(msg) => write!(f, "Query error: {msg}"),
        }
    }
}

impl std::error::Error for DatabaseError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Sqlx(err) => Some(err),
            Self::Serialization(err) => Some(err),
            Self::ModelNotFound(err) => Some(err),
            Self::ConnectionNotFound(_)
            | Self::NoDefaultConnection
            | Self::Configuration(_)
            | Self::Query(_) => None,
        }
    }
}

impl From<sqlx::Error> for DatabaseError {
    fn from(err: sqlx::Error) -> Self {
        Self::Sqlx(err)
    }
}

impl From<serde_json::Error> for DatabaseError {
    fn from(err: serde_json::Error) -> Self {
        Self::Serialization(err)
    }
}

impl From<ModelNotFoundException> for DatabaseError {
    fn from(err: ModelNotFoundException) -> Self {
        Self::ModelNotFound(err)
    }
}

#[derive(Debug, Clone)]
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

impl fmt::Display for ModelNotFoundException {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "No query results for model [{}]", self.model)
    }
}

impl std::error::Error for ModelNotFoundException {}
