/// Low-level storage engine errors (engine failures, serialization).
/// This is the error type for the `StorageEngine` trait — storage operations
/// can only fail with infrastructure errors, never domain errors.
#[derive(Debug, thiserror::Error)]
pub enum StorageError {
    #[error("storage engine error: {0}")]
    Engine(String),

    #[error("serialization error: {0}")]
    Serialization(String),

    #[error("store not found: {0}")]
    StoreNotFound(&'static str),

    #[error("corrupt data: {0}")]
    CorruptData(String),
}

impl From<rocksdb::Error> for StorageError {
    fn from(err: rocksdb::Error) -> Self {
        StorageError::Engine(err.into_string())
    }
}

impl From<serde_json::Error> for StorageError {
    fn from(err: serde_json::Error) -> Self {
        StorageError::Serialization(err.to_string())
    }
}

impl From<prost::DecodeError> for StorageError {
    fn from(err: prost::DecodeError) -> Self {
        StorageError::Serialization(err.to_string())
    }
}

impl From<crate::cluster::proto_convert::ConvertError> for StorageError {
    fn from(err: crate::cluster::proto_convert::ConvertError) -> Self {
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

    #[error("lua script compilation failed: {0}")]
    LuaCompilation(String),

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

#[derive(Debug, thiserror::Error)]
pub enum ConfigError {
    #[error("invalid config value: {0}")]
    InvalidValue(String),

    #[error(transparent)]
    Storage(#[from] StorageError),
}

#[derive(Debug, thiserror::Error)]
pub enum StatsError {
    #[error("queue not found: {0}")]
    QueueNotFound(String),

    #[error(transparent)]
    Storage(#[from] StorageError),
}

#[derive(Debug, thiserror::Error)]
pub enum RedriveError {
    #[error("queue not found: {0}")]
    QueueNotFound(String),

    #[error("not a dead-letter queue: {0}")]
    NotADLQ(String),

    #[error("parent queue not found: {0}")]
    ParentQueueNotFound(String),

    #[error(transparent)]
    Storage(#[from] StorageError),
}

#[derive(Debug, thiserror::Error)]
pub enum ListQueuesError {
    #[error(transparent)]
    Storage(#[from] StorageError),
}

#[derive(Debug, thiserror::Error)]
pub enum ConsumerGroupsError {
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
