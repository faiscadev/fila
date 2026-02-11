/// Low-level storage errors (RocksDB, serialization).
/// This is the error type for the `Storage` trait — storage operations can only
/// fail with infrastructure errors, never domain errors.
#[derive(Debug, thiserror::Error)]
pub enum StorageError {
    #[error("rocksdb error: {0}")]
    RocksDb(String),

    #[error("serialization error: {0}")]
    Serialization(String),
}

impl From<rocksdb::Error> for StorageError {
    fn from(err: rocksdb::Error) -> Self {
        StorageError::RocksDb(err.into_string())
    }
}

impl From<serde_json::Error> for StorageError {
    fn from(err: serde_json::Error) -> Self {
        StorageError::Serialization(err.to_string())
    }
}

/// Application-level errors for domain and broker logic.
#[derive(Debug, thiserror::Error)]
pub enum FilaError {
    #[error("queue not found: {0}")]
    QueueNotFound(String),

    #[error("message not found: {0}")]
    MessageNotFound(String),

    #[error("queue already exists: {0}")]
    QueueAlreadyExists(String),

    #[error("invalid configuration: {0}")]
    InvalidConfig(String),

    #[error("lua script error: {0}")]
    LuaError(String),

    #[error(transparent)]
    Storage(#[from] StorageError),
}

pub type StorageResult<T> = std::result::Result<T, StorageError>;
pub type Result<T> = std::result::Result<T, FilaError>;
