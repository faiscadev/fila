use crate::error::StorageResult;
use crate::message::Message;
use crate::queue::QueueConfig;

/// Identifies a storage partition. In single-node mode, all operations use
/// `PartitionId::DEFAULT`. In clustered mode (Epic 14), different partitions
/// map to different physical storage shards.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct PartitionId(u32);

impl PartitionId {
    /// The default partition used in single-node mode. Produces identical key
    /// encoding to the pre-partition-aware storage, ensuring zero behavioral change.
    pub const DEFAULT: PartitionId = PartitionId(0);

    /// Create a new partition ID. Partition 0 is reserved for `DEFAULT`.
    pub fn new(id: u32) -> Self {
        Self(id)
    }

    /// Returns the raw partition number.
    pub fn id(&self) -> u32 {
        self.0
    }

    /// Returns true if this is the default (single-node) partition.
    pub fn is_default(&self) -> bool {
        self.0 == 0
    }
}

impl std::fmt::Display for PartitionId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "partition-{}", self.0)
    }
}

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
/// All methods accept a `PartitionId` to support partition-aware storage. In
/// single-node mode, all calls use `PartitionId::DEFAULT`. Implementations
/// must produce identical behavior and key encoding for the default partition
/// as pre-partition-aware storage.
///
/// All methods return `StorageResult` — only infrastructure errors (backend,
/// serialization) are possible. Domain errors (queue not found, etc.) are
/// handled at the broker/scheduler layer.
pub trait Storage: Send + Sync {
    // --- Message operations ---

    /// Store a message in the messages CF.
    fn put_message(
        &self,
        partition: &PartitionId,
        key: &[u8],
        message: &Message,
    ) -> StorageResult<()>;

    /// Retrieve a message by its full key.
    fn get_message(&self, partition: &PartitionId, key: &[u8]) -> StorageResult<Option<Message>>;

    /// Delete a message by its full key.
    fn delete_message(&self, partition: &PartitionId, key: &[u8]) -> StorageResult<()>;

    /// List messages whose keys start with the given prefix, in lexicographic order.
    fn list_messages(
        &self,
        partition: &PartitionId,
        prefix: &[u8],
    ) -> StorageResult<Vec<(Vec<u8>, Message)>>;

    // --- Lease operations ---

    /// Store a lease in the leases CF.
    fn put_lease(&self, partition: &PartitionId, key: &[u8], value: &[u8]) -> StorageResult<()>;

    /// Retrieve a lease value by key.
    fn get_lease(&self, partition: &PartitionId, key: &[u8]) -> StorageResult<Option<Vec<u8>>>;

    /// Delete a lease by key.
    fn delete_lease(&self, partition: &PartitionId, key: &[u8]) -> StorageResult<()>;

    /// List lease expiry entries whose keys are <= the given upper bound timestamp key.
    /// Returns (expiry_key, empty_value) pairs sorted by expiry time (earliest first).
    fn list_expired_leases(
        &self,
        partition: &PartitionId,
        up_to_key: &[u8],
    ) -> StorageResult<Vec<Vec<u8>>>;

    // --- Queue operations ---

    /// Store a queue config.
    fn put_queue(
        &self,
        partition: &PartitionId,
        queue_id: &str,
        config: &QueueConfig,
    ) -> StorageResult<()>;

    /// Retrieve a queue config by ID.
    fn get_queue(
        &self,
        partition: &PartitionId,
        queue_id: &str,
    ) -> StorageResult<Option<QueueConfig>>;

    /// Delete a queue config.
    fn delete_queue(&self, partition: &PartitionId, queue_id: &str) -> StorageResult<()>;

    /// List all queue configs.
    fn list_queues(&self, partition: &PartitionId) -> StorageResult<Vec<QueueConfig>>;

    // --- State operations ---

    /// Store a state key-value pair.
    fn put_state(&self, partition: &PartitionId, key: &str, value: &[u8]) -> StorageResult<()>;

    /// Retrieve a state value by key.
    fn get_state(&self, partition: &PartitionId, key: &str) -> StorageResult<Option<Vec<u8>>>;

    /// Delete a state key.
    fn delete_state(&self, partition: &PartitionId, key: &str) -> StorageResult<()>;

    /// List state entries whose keys start with the given prefix.
    /// At most `limit` entries are returned; pass `usize::MAX` for no cap.
    fn list_state_by_prefix(
        &self,
        partition: &PartitionId,
        prefix: &str,
        limit: usize,
    ) -> StorageResult<Vec<(String, Vec<u8>)>>;

    // --- Batch operations ---

    /// Atomically apply a batch of write operations across column families.
    fn write_batch(&self, partition: &PartitionId, ops: Vec<WriteBatchOp>) -> StorageResult<()>;

    // --- Lifecycle ---

    /// Flush the WAL to ensure all writes are durable.
    fn flush(&self) -> StorageResult<()>;
}
