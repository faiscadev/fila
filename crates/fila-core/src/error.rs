/// Low-level storage errors (RocksDB, serialization).
/// This is the error type for the `Storage` trait — storage operations can only
/// fail with infrastructure errors, never domain errors.
#[derive(Debug, thiserror::Error)]
pub enum StorageError {
    #[error("rocksdb error: {0}")]
    RocksDb(String),

    #[error("serialization error: {0}")]
    Serialization(String),

    #[error("column family not found: {0}")]
    ColumnFamilyNotFound(&'static str),
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

pub type StorageResult<T> = std::result::Result<T, StorageError>;

// --- Per-command error types ---

#[derive(Debug, thiserror::Error)]
pub enum EnqueueError {
    #[error("queue not found: {0}")]
    QueueNotFound(String),

    #[error(transparent)]
    Storage(#[from] StorageError),
}

#[derive(Debug, thiserror::Error)]
pub enum AckError {
    #[error("message not found: {0}")]
    MessageNotFound(String),

    #[error(transparent)]
    Storage(#[from] StorageError),
}

#[derive(Debug, thiserror::Error)]
pub enum NackError {
    #[error("message not found: {0}")]
    MessageNotFound(String),

    #[error(transparent)]
    Storage(#[from] StorageError),
}

#[derive(Debug, thiserror::Error)]
pub enum CreateQueueError {
    #[error("queue already exists: {0}")]
    QueueAlreadyExists(String),

    #[error(transparent)]
    Storage(#[from] StorageError),
}

#[derive(Debug, thiserror::Error)]
pub enum DeleteQueueError {
    #[error("queue not found: {0}")]
    QueueNotFound(String),

    #[error(transparent)]
    Storage(#[from] StorageError),
}

// --- Broker lifecycle/channel errors ---

#[derive(Debug, thiserror::Error)]
pub enum BrokerError {
    #[error("failed to spawn scheduler thread: {0}")]
    SchedulerSpawn(String),

    #[error("scheduler command channel full")]
    ChannelFull,

    #[error("scheduler command channel disconnected")]
    ChannelDisconnected,

    #[error("scheduler thread panicked")]
    SchedulerPanicked,
}

pub type BrokerResult<T> = std::result::Result<T, BrokerError>;
