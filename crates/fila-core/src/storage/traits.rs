use crate::error::StorageResult;
use crate::message::Message;
use crate::queue::QueueConfig;

/// A single mutation to apply atomically as part of a batch.
///
/// Used by `StorageEngine::apply_mutations` to group multiple writes and
/// deletes into a single atomic operation. In a Raft-replicated system,
/// committed log entries are applied to the local state machine via a
/// batch of `Mutation`s.
#[derive(Debug)]
pub enum Mutation {
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

/// Storage engine trait for all persistence operations.
///
/// Defines what Fila needs from its storage layer using domain terms:
/// message store, lease store, queue config store, and state store.
/// Implementations must be thread-safe (`Send + Sync`).
///
/// All methods return `StorageResult` — only infrastructure errors
/// (engine failures, serialization) are possible. Domain errors
/// (queue not found, etc.) are handled at the broker/scheduler layer.
///
/// In a Raft-replicated system, the storage engine serves as the local
/// state machine backend: it applies committed entries and serves reads.
pub trait StorageEngine: Send + Sync {
    // --- Message store ---

    /// Store a message.
    fn put_message(&self, key: &[u8], message: &Message) -> StorageResult<()>;

    /// Retrieve a message by its full key.
    fn get_message(&self, key: &[u8]) -> StorageResult<Option<Message>>;

    /// Delete a message by its full key.
    fn delete_message(&self, key: &[u8]) -> StorageResult<()>;

    /// List messages whose keys start with the given prefix, in lexicographic order.
    fn list_messages(&self, prefix: &[u8]) -> StorageResult<Vec<(Vec<u8>, Message)>>;

    // --- Lease store ---

    /// Store a lease record.
    fn put_lease(&self, key: &[u8], value: &[u8]) -> StorageResult<()>;

    /// Retrieve a lease value by key.
    fn get_lease(&self, key: &[u8]) -> StorageResult<Option<Vec<u8>>>;

    /// Delete a lease by key.
    fn delete_lease(&self, key: &[u8]) -> StorageResult<()>;

    /// List lease expiry entries whose keys are <= the given upper bound timestamp key.
    /// Returns expiry keys sorted by expiry time (earliest first).
    fn list_expired_leases(&self, up_to_key: &[u8]) -> StorageResult<Vec<Vec<u8>>>;

    // --- Queue config store ---

    /// Store a queue config.
    fn put_queue(&self, queue_id: &str, config: &QueueConfig) -> StorageResult<()>;

    /// Retrieve a queue config by ID.
    fn get_queue(&self, queue_id: &str) -> StorageResult<Option<QueueConfig>>;

    /// Delete a queue config.
    fn delete_queue(&self, queue_id: &str) -> StorageResult<()>;

    /// List all queue configs.
    fn list_queues(&self) -> StorageResult<Vec<QueueConfig>>;

    // --- State store ---

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

    // --- Batch mutations ---

    /// Atomically apply a batch of mutations.
    ///
    /// All mutations in the batch are applied as a single atomic operation.
    /// In a Raft-replicated system, this is the method used to apply
    /// committed log entries to the local state machine.
    fn apply_mutations(&self, mutations: Vec<Mutation>) -> StorageResult<()>;

    // --- Lifecycle ---

    /// Flush the write-ahead log to ensure all writes are durable.
    fn flush(&self) -> StorageResult<()>;
}
