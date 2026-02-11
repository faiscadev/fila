#[derive(Debug, thiserror::Error)]
pub enum FilaError {
    #[error("queue not found: {0}")]
    QueueNotFound(String),

    #[error("message not found: {0}")]
    MessageNotFound(String),

    #[error("lua script error: {0}")]
    LuaError(String),

    #[error("storage error: {0}")]
    StorageError(String),

    #[error("invalid configuration: {0}")]
    InvalidConfig(String),

    #[error("queue already exists: {0}")]
    QueueAlreadyExists(String),
}

impl From<rocksdb::Error> for FilaError {
    fn from(err: rocksdb::Error) -> Self {
        FilaError::StorageError(err.into_string())
    }
}

impl From<serde_json::Error> for FilaError {
    fn from(err: serde_json::Error) -> Self {
        FilaError::StorageError(format!("serialization error: {err}"))
    }
}

pub type Result<T> = std::result::Result<T, FilaError>;
