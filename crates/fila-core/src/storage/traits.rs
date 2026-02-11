use crate::error::Result;
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
pub trait Storage: Send + Sync {
    // --- Message operations ---

    /// Store a message in the messages CF.
    fn put_message(&self, key: &[u8], message: &Message) -> Result<()>;

    /// Retrieve a message by its full key.
    fn get_message(&self, key: &[u8]) -> Result<Option<Message>>;

    /// Delete a message by its full key.
    fn delete_message(&self, key: &[u8]) -> Result<()>;

    /// List messages whose keys start with the given prefix, in lexicographic order.
    fn list_messages(&self, prefix: &[u8]) -> Result<Vec<(Vec<u8>, Message)>>;

    // --- Lease operations ---

    /// Store a lease in the leases CF.
    fn put_lease(&self, key: &[u8], value: &[u8]) -> Result<()>;

    /// Retrieve a lease value by key.
    fn get_lease(&self, key: &[u8]) -> Result<Option<Vec<u8>>>;

    /// Delete a lease by key.
    fn delete_lease(&self, key: &[u8]) -> Result<()>;

    /// List lease expiry entries whose keys are <= the given upper bound timestamp key.
    /// Returns (expiry_key, empty_value) pairs sorted by expiry time (earliest first).
    fn list_expired_leases(&self, up_to_key: &[u8]) -> Result<Vec<Vec<u8>>>;

    // --- Queue operations ---

    /// Store a queue config.
    fn put_queue(&self, queue_id: &str, config: &QueueConfig) -> Result<()>;

    /// Retrieve a queue config by ID.
    fn get_queue(&self, queue_id: &str) -> Result<Option<QueueConfig>>;

    /// Delete a queue config.
    fn delete_queue(&self, queue_id: &str) -> Result<()>;

    /// List all queue configs.
    fn list_queues(&self) -> Result<Vec<QueueConfig>>;

    // --- State operations ---

    /// Store a state key-value pair.
    fn put_state(&self, key: &str, value: &[u8]) -> Result<()>;

    /// Retrieve a state value by key.
    fn get_state(&self, key: &str) -> Result<Option<Vec<u8>>>;

    /// Delete a state key.
    fn delete_state(&self, key: &str) -> Result<()>;

    // --- Batch operations ---

    /// Atomically apply a batch of write operations across column families.
    fn write_batch(&self, ops: Vec<WriteBatchOp>) -> Result<()>;
}
