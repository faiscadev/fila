use crate::error::StorageResult;
use crate::message::Message;
use crate::queue::QueueConfig;

/// Represents a single operation in an atomic write batch.
#[derive(Debug)]
pub enum WriteBatchOp {
    PutMessage { key: Vec<u8>, value: Vec<u8> },
    DeleteMessage { key: Vec<u8> },
    PutLease { key: Vec<u8>, value: Vec<u8> },
    DeleteLease { key: Vec<u8> },
    PutLeaseExpiry { key: Vec<u8> },
    DeleteLeaseExpiry { key: Vec<u8> },
    PutQueue { key: Vec<u8>, value: Vec<u8> },
    DeleteQueue { key: Vec<u8> },
    PutState { key: Vec<u8>, value: Vec<u8> },
    DeleteState { key: Vec<u8> },
}

/// Storage trait for all persistence operations. Implementations must be thread-safe.
///
/// All methods return `StorageResult` — only infrastructure errors (RocksDB,
/// serialization) are possible. Domain errors (queue not found, etc.) are
/// handled at the broker/scheduler layer.
pub trait Storage: Send + Sync {
    // --- Message operations ---

    /// Store a message in the messages CF.
    fn put_message(&self, key: &[u8], message: &Message) -> StorageResult<()>;

    /// Retrieve a message by its full key.
    fn get_message(&self, key: &[u8]) -> StorageResult<Option<Message>>;

    /// Delete a message by its full key.
    fn delete_message(&self, key: &[u8]) -> StorageResult<()>;

    /// List messages whose keys start with the given prefix, in lexicographic order.
    fn list_messages(&self, prefix: &[u8]) -> StorageResult<Vec<(Vec<u8>, Message)>>;

    // --- Lease operations ---

    /// Store a lease in the leases CF.
    fn put_lease(&self, key: &[u8], value: &[u8]) -> StorageResult<()>;

    /// Retrieve a lease value by key.
    fn get_lease(&self, key: &[u8]) -> StorageResult<Option<Vec<u8>>>;

    /// Delete a lease by key.
    fn delete_lease(&self, key: &[u8]) -> StorageResult<()>;

    /// List lease expiry entries whose keys are <= the given upper bound timestamp key.
    /// Returns (expiry_key, empty_value) pairs sorted by expiry time (earliest first).
    fn list_expired_leases(&self, up_to_key: &[u8]) -> StorageResult<Vec<Vec<u8>>>;

    // --- Queue operations ---

    /// Store a queue config.
    fn put_queue(&self, queue_id: &str, config: &QueueConfig) -> StorageResult<()>;

    /// Retrieve a queue config by ID.
    fn get_queue(&self, queue_id: &str) -> StorageResult<Option<QueueConfig>>;

    /// Delete a queue config.
    fn delete_queue(&self, queue_id: &str) -> StorageResult<()>;

    /// List all queue configs.
    fn list_queues(&self) -> StorageResult<Vec<QueueConfig>>;

    // --- State operations ---

    /// Store a state key-value pair.
    fn put_state(&self, key: &str, value: &[u8]) -> StorageResult<()>;

    /// Retrieve a state value by key.
    fn get_state(&self, key: &str) -> StorageResult<Option<Vec<u8>>>;

    /// Delete a state key.
    fn delete_state(&self, key: &str) -> StorageResult<()>;

    /// List state entries whose keys start with the given prefix.
    /// At most `limit` entries are returned; pass `usize::MAX` for no cap.
    fn list_state_by_prefix(
        &self,
        prefix: &str,
        limit: usize,
    ) -> StorageResult<Vec<(String, Vec<u8>)>>;

    // --- Batch operations ---

    /// Atomically apply a batch of write operations across column families.
    fn write_batch(&self, ops: Vec<WriteBatchOp>) -> StorageResult<()>;

    // --- Lifecycle ---

    /// Flush the WAL to ensure all writes are durable.
    fn flush(&self) -> StorageResult<()>;
}
