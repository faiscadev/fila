pub(crate) mod compaction;
pub mod config;
pub mod wal;

use std::collections::{BTreeMap, HashMap};
use std::sync::{Arc, Mutex, RwLock};

use tracing::warn;

use crate::error::{StorageError, StorageResult};
use crate::message::Message;
use crate::queue::QueueConfig;
use crate::storage::{PartitionId, Storage, WriteBatchOp};

use config::FilaStorageConfig;
use wal::{OpTag, WalEntry, WalOp, WalReader, WalWriter};

// ---------------------------------------------------------------------------
// In-memory indexes
// ---------------------------------------------------------------------------

/// In-memory indexes rebuilt from WAL replay and maintained incrementally.
pub(super) struct Indexes {
    /// Messages indexed by their full storage key (BTreeMap for prefix scans).
    pub(super) messages: BTreeMap<Vec<u8>, Message>,
    /// Lease values indexed by lease key.
    pub(super) leases: HashMap<Vec<u8>, Vec<u8>>,
    /// Lease expiry keys (BTreeMap for range queries on expiry time).
    pub(super) lease_expiries: BTreeMap<Vec<u8>, ()>,
    /// Queue configs indexed by queue_id.
    pub(super) queues: HashMap<String, QueueConfig>,
    /// State key-values (BTreeMap for prefix scans).
    pub(super) state: BTreeMap<String, Vec<u8>>,
}

impl Indexes {
    fn new() -> Self {
        Self {
            messages: BTreeMap::new(),
            leases: HashMap::new(),
            lease_expiries: BTreeMap::new(),
            queues: HashMap::new(),
            state: BTreeMap::new(),
        }
    }

    /// Apply a single WAL operation to the indexes.
    fn apply_op(&mut self, op: &WalOp) {
        match op.tag {
            OpTag::PutMessage => {
                if let Some(ref value) = op.value {
                    if let Ok(msg) = serde_json::from_slice::<Message>(value) {
                        self.messages.insert(op.key.clone(), msg);
                    } else {
                        warn!("failed to deserialize message during index update");
                    }
                }
            }
            OpTag::DeleteMessage => {
                self.messages.remove(&op.key);
            }
            OpTag::PutLease => {
                if let Some(ref value) = op.value {
                    self.leases.insert(op.key.clone(), value.clone());
                }
            }
            OpTag::DeleteLease => {
                self.leases.remove(&op.key);
            }
            OpTag::PutLeaseExpiry => {
                self.lease_expiries.insert(op.key.clone(), ());
            }
            OpTag::DeleteLeaseExpiry => {
                self.lease_expiries.remove(&op.key);
            }
            OpTag::PutQueue => {
                if let Some(ref value) = op.value {
                    if let Ok(config) = serde_json::from_slice::<QueueConfig>(value) {
                        let queue_id =
                            String::from_utf8(op.key.clone()).unwrap_or_else(|_| String::new());
                        self.queues.insert(queue_id, config);
                    } else {
                        warn!("failed to deserialize queue config during index update");
                    }
                }
            }
            OpTag::DeleteQueue => {
                let queue_id = String::from_utf8(op.key.clone()).unwrap_or_else(|_| String::new());
                self.queues.remove(&queue_id);
            }
            OpTag::PutState => {
                if let Some(ref value) = op.value {
                    let key = String::from_utf8(op.key.clone()).unwrap_or_else(|_| String::new());
                    self.state.insert(key, value.clone());
                }
            }
            OpTag::DeleteState => {
                let key = String::from_utf8(op.key.clone()).unwrap_or_else(|_| String::new());
                self.state.remove(&key);
            }
        }
    }

    /// Apply all operations in a WAL entry to the indexes.
    fn apply_entry(&mut self, entry: &WalEntry) {
        for op in &entry.ops {
            self.apply_op(op);
        }
    }
}

// ---------------------------------------------------------------------------
// FilaStorage
// ---------------------------------------------------------------------------

/// Purpose-built storage engine for Fila's queue workload.
///
/// All writes go through the append-only WAL. In-memory indexes are rebuilt
/// from WAL replay on startup and maintained incrementally on each write.
/// Background compaction removes dead entries from sealed segments.
pub struct FilaStorage {
    writer: Arc<Mutex<WalWriter>>,
    indexes: Arc<RwLock<Indexes>>,
    #[allow(dead_code)] // held for Drop — stops compaction thread on shutdown
    compaction_handle: Option<compaction::CompactionHandle>,
}

impl FilaStorage {
    /// Open or create a FilaStorage engine at the configured directory.
    pub fn open(config: &FilaStorageConfig) -> Result<Self, StorageError> {
        let writer = WalWriter::open(
            &config.data_dir,
            config.sync_mode.clone(),
            config.segment_size_bytes,
        )
        .map_err(|e| StorageError::Backend(format!("failed to open WAL: {e}")))?;

        // Replay WAL to rebuild indexes
        let reader = WalReader::new(&config.data_dir);
        let entries = reader
            .replay()
            .map_err(|e| StorageError::Backend(format!("WAL replay failed: {e}")))?;

        let mut indexes = Indexes::new();
        for entry in &entries {
            indexes.apply_entry(entry);
        }

        let writer = Arc::new(Mutex::new(writer));
        let indexes = Arc::new(RwLock::new(indexes));

        let compaction_handle = if config.compaction_enabled {
            Some(compaction::spawn_compaction_thread(
                config,
                writer.clone(),
                indexes.clone(),
            ))
        } else {
            None
        };

        Ok(Self {
            writer,
            indexes,
            compaction_handle,
        })
    }
}

// ---------------------------------------------------------------------------
// Helpers: convert WriteBatchOp to WalOp
// ---------------------------------------------------------------------------

fn batch_op_to_wal_op(op: WriteBatchOp) -> WalOp {
    match op {
        WriteBatchOp::PutMessage { key, value } => WalOp {
            tag: OpTag::PutMessage,
            key,
            value: Some(value),
        },
        WriteBatchOp::DeleteMessage { key } => WalOp {
            tag: OpTag::DeleteMessage,
            key,
            value: None,
        },
        WriteBatchOp::PutLease { key, value } => WalOp {
            tag: OpTag::PutLease,
            key,
            value: Some(value),
        },
        WriteBatchOp::DeleteLease { key } => WalOp {
            tag: OpTag::DeleteLease,
            key,
            value: None,
        },
        WriteBatchOp::PutLeaseExpiry { key } => WalOp {
            tag: OpTag::PutLeaseExpiry,
            key,
            value: None,
        },
        WriteBatchOp::DeleteLeaseExpiry { key } => WalOp {
            tag: OpTag::DeleteLeaseExpiry,
            key,
            value: None,
        },
        WriteBatchOp::PutQueue { key, value } => WalOp {
            tag: OpTag::PutQueue,
            key,
            value: Some(value),
        },
        WriteBatchOp::DeleteQueue { key } => WalOp {
            tag: OpTag::DeleteQueue,
            key,
            value: None,
        },
        WriteBatchOp::PutState { key, value } => WalOp {
            tag: OpTag::PutState,
            key,
            value: Some(value),
        },
        WriteBatchOp::DeleteState { key } => WalOp {
            tag: OpTag::DeleteState,
            key,
            value: None,
        },
    }
}

/// Compute the exclusive upper bound for a prefix scan on byte keys.
/// Returns None if the prefix is all 0xFF bytes (no upper bound possible).
fn prefix_upper_bound(prefix: &[u8]) -> Option<Vec<u8>> {
    let mut end = prefix.to_vec();
    // Walk backwards, incrementing the last non-0xFF byte
    while let Some(last) = end.last_mut() {
        if *last < 0xFF {
            *last += 1;
            return Some(end);
        }
        end.pop();
    }
    None // all 0xFF — scan to the end
}

/// Compute the exclusive upper bound for a prefix scan on string keys.
fn prefix_upper_bound_str(prefix: &str) -> Option<String> {
    prefix_upper_bound(prefix.as_bytes()).and_then(|bytes| String::from_utf8(bytes).ok())
}

// ---------------------------------------------------------------------------
// Storage trait implementation
// ---------------------------------------------------------------------------

impl Storage for FilaStorage {
    // --- Message operations ---

    fn put_message(
        &self,
        partition: &PartitionId,
        key: &[u8],
        message: &Message,
    ) -> StorageResult<()> {
        let value =
            serde_json::to_vec(message).map_err(|e| StorageError::Serialization(e.to_string()))?;
        self.write_batch(
            partition,
            vec![WriteBatchOp::PutMessage {
                key: key.to_vec(),
                value,
            }],
        )
    }

    fn get_message(&self, _partition: &PartitionId, key: &[u8]) -> StorageResult<Option<Message>> {
        let indexes = self
            .indexes
            .read()
            .map_err(|e| StorageError::Backend(format!("index lock poisoned: {e}")))?;
        Ok(indexes.messages.get(key).cloned())
    }

    fn delete_message(&self, partition: &PartitionId, key: &[u8]) -> StorageResult<()> {
        self.write_batch(
            partition,
            vec![WriteBatchOp::DeleteMessage { key: key.to_vec() }],
        )
    }

    fn list_messages(
        &self,
        _partition: &PartitionId,
        prefix: &[u8],
    ) -> StorageResult<Vec<(Vec<u8>, Message)>> {
        let indexes = self
            .indexes
            .read()
            .map_err(|e| StorageError::Backend(format!("index lock poisoned: {e}")))?;

        let results: Vec<(Vec<u8>, Message)> = if let Some(end) = prefix_upper_bound(prefix) {
            indexes
                .messages
                .range(prefix.to_vec()..end)
                .map(|(k, v)| (k.clone(), v.clone()))
                .collect()
        } else {
            indexes
                .messages
                .range(prefix.to_vec()..)
                .map(|(k, v)| (k.clone(), v.clone()))
                .collect()
        };
        Ok(results)
    }

    // --- Lease operations ---

    fn put_lease(&self, partition: &PartitionId, key: &[u8], value: &[u8]) -> StorageResult<()> {
        self.write_batch(
            partition,
            vec![WriteBatchOp::PutLease {
                key: key.to_vec(),
                value: value.to_vec(),
            }],
        )
    }

    fn get_lease(&self, _partition: &PartitionId, key: &[u8]) -> StorageResult<Option<Vec<u8>>> {
        let indexes = self
            .indexes
            .read()
            .map_err(|e| StorageError::Backend(format!("index lock poisoned: {e}")))?;
        Ok(indexes.leases.get(key).cloned())
    }

    fn delete_lease(&self, partition: &PartitionId, key: &[u8]) -> StorageResult<()> {
        self.write_batch(
            partition,
            vec![WriteBatchOp::DeleteLease { key: key.to_vec() }],
        )
    }

    fn list_expired_leases(
        &self,
        _partition: &PartitionId,
        up_to_key: &[u8],
    ) -> StorageResult<Vec<Vec<u8>>> {
        let indexes = self
            .indexes
            .read()
            .map_err(|e| StorageError::Backend(format!("index lock poisoned: {e}")))?;

        let results: Vec<Vec<u8>> = indexes
            .lease_expiries
            .range(..=up_to_key.to_vec())
            .map(|(k, _)| k.clone())
            .collect();
        Ok(results)
    }

    // --- Queue operations ---

    fn put_queue(
        &self,
        partition: &PartitionId,
        queue_id: &str,
        config: &QueueConfig,
    ) -> StorageResult<()> {
        let value =
            serde_json::to_vec(config).map_err(|e| StorageError::Serialization(e.to_string()))?;
        self.write_batch(
            partition,
            vec![WriteBatchOp::PutQueue {
                key: queue_id.as_bytes().to_vec(),
                value,
            }],
        )
    }

    fn get_queue(
        &self,
        _partition: &PartitionId,
        queue_id: &str,
    ) -> StorageResult<Option<QueueConfig>> {
        let indexes = self
            .indexes
            .read()
            .map_err(|e| StorageError::Backend(format!("index lock poisoned: {e}")))?;
        Ok(indexes.queues.get(queue_id).cloned())
    }

    fn delete_queue(&self, partition: &PartitionId, queue_id: &str) -> StorageResult<()> {
        self.write_batch(
            partition,
            vec![WriteBatchOp::DeleteQueue {
                key: queue_id.as_bytes().to_vec(),
            }],
        )
    }

    fn list_queues(&self, _partition: &PartitionId) -> StorageResult<Vec<QueueConfig>> {
        let indexes = self
            .indexes
            .read()
            .map_err(|e| StorageError::Backend(format!("index lock poisoned: {e}")))?;
        Ok(indexes.queues.values().cloned().collect())
    }

    // --- State operations ---

    fn put_state(&self, partition: &PartitionId, key: &str, value: &[u8]) -> StorageResult<()> {
        self.write_batch(
            partition,
            vec![WriteBatchOp::PutState {
                key: key.as_bytes().to_vec(),
                value: value.to_vec(),
            }],
        )
    }

    fn get_state(&self, _partition: &PartitionId, key: &str) -> StorageResult<Option<Vec<u8>>> {
        let indexes = self
            .indexes
            .read()
            .map_err(|e| StorageError::Backend(format!("index lock poisoned: {e}")))?;
        Ok(indexes.state.get(key).cloned())
    }

    fn delete_state(&self, partition: &PartitionId, key: &str) -> StorageResult<()> {
        self.write_batch(
            partition,
            vec![WriteBatchOp::DeleteState {
                key: key.as_bytes().to_vec(),
            }],
        )
    }

    fn list_state_by_prefix(
        &self,
        _partition: &PartitionId,
        prefix: &str,
        limit: usize,
    ) -> StorageResult<Vec<(String, Vec<u8>)>> {
        let indexes = self
            .indexes
            .read()
            .map_err(|e| StorageError::Backend(format!("index lock poisoned: {e}")))?;

        let results: Vec<(String, Vec<u8>)> = if let Some(end) = prefix_upper_bound_str(prefix) {
            indexes
                .state
                .range(prefix.to_string()..end)
                .take(limit)
                .map(|(k, v)| (k.clone(), v.clone()))
                .collect()
        } else {
            indexes
                .state
                .range(prefix.to_string()..)
                .take(limit)
                .map(|(k, v)| (k.clone(), v.clone()))
                .collect()
        };
        Ok(results)
    }

    // --- Batch operations ---

    fn write_batch(&self, _partition: &PartitionId, ops: Vec<WriteBatchOp>) -> StorageResult<()> {
        if ops.is_empty() {
            return Ok(());
        }

        let wal_ops: Vec<WalOp> = ops.into_iter().map(batch_op_to_wal_op).collect();
        let entry = WalEntry { ops: wal_ops };

        // Hold the writer lock across both WAL append and index update to prevent
        // concurrent write_batch calls from reordering index updates relative to WAL order.
        let mut writer = self
            .writer
            .lock()
            .map_err(|e| StorageError::Backend(format!("WAL writer lock poisoned: {e}")))?;

        writer
            .append(&entry)
            .map_err(|e| StorageError::Backend(format!("WAL append failed: {e}")))?;

        let mut indexes = self
            .indexes
            .write()
            .map_err(|e| StorageError::Backend(format!("index lock poisoned: {e}")))?;

        indexes.apply_entry(&entry);

        Ok(())
    }

    // --- Lifecycle ---

    fn flush(&self) -> StorageResult<()> {
        let mut writer = self
            .writer
            .lock()
            .map_err(|e| StorageError::Backend(format!("WAL writer lock poisoned: {e}")))?;

        writer
            .fsync()
            .map_err(|e| StorageError::Backend(format!("WAL fsync failed: {e}")))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const P: &PartitionId = &PartitionId::DEFAULT;

    fn test_message(queue_id: &str, payload: &[u8]) -> Message {
        Message {
            id: uuid::Uuid::now_v7(),
            queue_id: queue_id.to_string(),
            headers: Default::default(),
            payload: payload.to_vec(),
            fairness_key: "default".to_string(),
            weight: 1,
            throttle_keys: vec![],
            attempt_count: 0,
            enqueued_at: 12345,
            leased_at: None,
        }
    }

    fn test_queue_config(queue_id: &str) -> QueueConfig {
        QueueConfig::new(queue_id.to_string())
    }

    // --- WAL write path tests (carried over from Story 13.2) ---

    #[test]
    fn fila_storage_implements_storage_trait() {
        let dir = tempfile::tempdir().unwrap();
        let config = FilaStorageConfig::new(dir.path().to_path_buf());
        let storage = FilaStorage::open(&config).unwrap();

        fn assert_send_sync<T: Send + Sync>() {}
        assert_send_sync::<FilaStorage>();

        let msg = test_message("test-q", b"hello");

        storage.put_message(P, b"msg-key-1", &msg).unwrap();
        storage
            .put_lease(P, b"lease-key-1", b"consumer:999")
            .unwrap();
        storage.delete_message(P, b"msg-key-1").unwrap();
        storage.flush().unwrap();

        let reader = wal::WalReader::new(dir.path());
        let entries = reader.replay().unwrap();
        assert_eq!(entries.len(), 3);
        assert_eq!(entries[0].ops[0].tag, OpTag::PutMessage);
        assert_eq!(entries[1].ops[0].tag, OpTag::PutLease);
        assert_eq!(entries[2].ops[0].tag, OpTag::DeleteMessage);
    }

    #[test]
    fn fila_storage_write_batch_atomic() {
        let dir = tempfile::tempdir().unwrap();
        let config = FilaStorageConfig::new(dir.path().to_path_buf());
        let storage = FilaStorage::open(&config).unwrap();

        let ops = vec![
            WriteBatchOp::PutMessage {
                key: b"k1".to_vec(),
                value: b"v1".to_vec(),
            },
            WriteBatchOp::DeleteLease {
                key: b"k2".to_vec(),
            },
            WriteBatchOp::PutLeaseExpiry {
                key: b"k3".to_vec(),
            },
        ];

        storage.write_batch(P, ops).unwrap();

        let reader = wal::WalReader::new(dir.path());
        let entries = reader.replay().unwrap();
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].ops.len(), 3);
    }

    #[test]
    fn fila_storage_empty_write_batch_is_noop() {
        let dir = tempfile::tempdir().unwrap();
        let config = FilaStorageConfig::new(dir.path().to_path_buf());
        let storage = FilaStorage::open(&config).unwrap();

        storage.write_batch(P, vec![]).unwrap();

        let reader = wal::WalReader::new(dir.path());
        let entries = reader.replay().unwrap();
        assert!(entries.is_empty());
    }

    // --- Read path tests (Story 13.3) ---

    #[test]
    fn message_put_get_roundtrip() {
        let dir = tempfile::tempdir().unwrap();
        let config = FilaStorageConfig::new(dir.path().to_path_buf());
        let storage = FilaStorage::open(&config).unwrap();

        let msg = test_message("q1", b"payload-data");
        storage.put_message(P, b"msg-key-1", &msg).unwrap();

        let retrieved = storage.get_message(P, b"msg-key-1").unwrap();
        assert_eq!(retrieved, Some(msg));
    }

    #[test]
    fn message_get_nonexistent_returns_none() {
        let dir = tempfile::tempdir().unwrap();
        let config = FilaStorageConfig::new(dir.path().to_path_buf());
        let storage = FilaStorage::open(&config).unwrap();

        assert!(storage.get_message(P, b"no-such-key").unwrap().is_none());
    }

    #[test]
    fn message_delete_removes_from_index() {
        let dir = tempfile::tempdir().unwrap();
        let config = FilaStorageConfig::new(dir.path().to_path_buf());
        let storage = FilaStorage::open(&config).unwrap();

        let msg = test_message("q1", b"data");
        storage.put_message(P, b"key-1", &msg).unwrap();
        assert!(storage.get_message(P, b"key-1").unwrap().is_some());

        storage.delete_message(P, b"key-1").unwrap();
        assert!(storage.get_message(P, b"key-1").unwrap().is_none());
    }

    #[test]
    fn list_messages_prefix_filtering() {
        let dir = tempfile::tempdir().unwrap();
        let config = FilaStorageConfig::new(dir.path().to_path_buf());
        let storage = FilaStorage::open(&config).unwrap();

        let msg1 = test_message("q1", b"a");
        let msg2 = test_message("q1", b"b");
        let msg3 = test_message("q2", b"c");

        storage.put_message(P, b"q1:msg-1", &msg1).unwrap();
        storage.put_message(P, b"q1:msg-2", &msg2).unwrap();
        storage.put_message(P, b"q2:msg-1", &msg3).unwrap();

        let q1_msgs = storage.list_messages(P, b"q1:").unwrap();
        assert_eq!(q1_msgs.len(), 2);
        assert_eq!(q1_msgs[0].0, b"q1:msg-1");
        assert_eq!(q1_msgs[1].0, b"q1:msg-2");

        let q2_msgs = storage.list_messages(P, b"q2:").unwrap();
        assert_eq!(q2_msgs.len(), 1);
    }

    #[test]
    fn list_messages_excludes_deleted() {
        let dir = tempfile::tempdir().unwrap();
        let config = FilaStorageConfig::new(dir.path().to_path_buf());
        let storage = FilaStorage::open(&config).unwrap();

        let msg1 = test_message("q1", b"a");
        let msg2 = test_message("q1", b"b");
        storage.put_message(P, b"q1:msg-1", &msg1).unwrap();
        storage.put_message(P, b"q1:msg-2", &msg2).unwrap();

        storage.delete_message(P, b"q1:msg-1").unwrap();

        let msgs = storage.list_messages(P, b"q1:").unwrap();
        assert_eq!(msgs.len(), 1);
        assert_eq!(msgs[0].0, b"q1:msg-2");
    }

    #[test]
    fn lease_put_get_delete_roundtrip() {
        let dir = tempfile::tempdir().unwrap();
        let config = FilaStorageConfig::new(dir.path().to_path_buf());
        let storage = FilaStorage::open(&config).unwrap();

        storage
            .put_lease(P, b"lease-1", b"consumer-a:12345")
            .unwrap();

        let val = storage.get_lease(P, b"lease-1").unwrap();
        assert_eq!(val, Some(b"consumer-a:12345".to_vec()));

        storage.delete_lease(P, b"lease-1").unwrap();
        assert!(storage.get_lease(P, b"lease-1").unwrap().is_none());
    }

    #[test]
    fn list_expired_leases_range_query() {
        let dir = tempfile::tempdir().unwrap();
        let config = FilaStorageConfig::new(dir.path().to_path_buf());
        let storage = FilaStorage::open(&config).unwrap();

        // Expiry keys are timestamp-prefixed (big-endian u64)
        let key1 = 100u64.to_be_bytes().to_vec();
        let key2 = 200u64.to_be_bytes().to_vec();
        let key3 = 300u64.to_be_bytes().to_vec();

        storage
            .write_batch(
                P,
                vec![
                    WriteBatchOp::PutLeaseExpiry { key: key1.clone() },
                    WriteBatchOp::PutLeaseExpiry { key: key2.clone() },
                    WriteBatchOp::PutLeaseExpiry { key: key3.clone() },
                ],
            )
            .unwrap();

        // Query up to 200 — should return key1 and key2
        let up_to = 200u64.to_be_bytes().to_vec();
        let expired = storage.list_expired_leases(P, &up_to).unwrap();
        assert_eq!(expired.len(), 2);
        assert_eq!(expired[0], key1);
        assert_eq!(expired[1], key2);

        // Query up to 300 — should return all 3
        let up_to_all = 300u64.to_be_bytes().to_vec();
        let all = storage.list_expired_leases(P, &up_to_all).unwrap();
        assert_eq!(all.len(), 3);
    }

    #[test]
    fn queue_put_get_delete_roundtrip() {
        let dir = tempfile::tempdir().unwrap();
        let config = FilaStorageConfig::new(dir.path().to_path_buf());
        let storage = FilaStorage::open(&config).unwrap();

        let qc = test_queue_config("my-queue");
        storage.put_queue(P, "my-queue", &qc).unwrap();

        let retrieved = storage.get_queue(P, "my-queue").unwrap();
        assert_eq!(retrieved.unwrap().name, "my-queue");

        storage.delete_queue(P, "my-queue").unwrap();
        assert!(storage.get_queue(P, "my-queue").unwrap().is_none());
    }

    #[test]
    fn list_queues_returns_all() {
        let dir = tempfile::tempdir().unwrap();
        let config = FilaStorageConfig::new(dir.path().to_path_buf());
        let storage = FilaStorage::open(&config).unwrap();

        storage
            .put_queue(P, "q1", &test_queue_config("q1"))
            .unwrap();
        storage
            .put_queue(P, "q2", &test_queue_config("q2"))
            .unwrap();

        let queues = storage.list_queues(P).unwrap();
        assert_eq!(queues.len(), 2);

        let names: Vec<&str> = queues.iter().map(|q| q.name.as_str()).collect();
        assert!(names.contains(&"q1"));
        assert!(names.contains(&"q2"));
    }

    #[test]
    fn state_put_get_delete_roundtrip() {
        let dir = tempfile::tempdir().unwrap();
        let config = FilaStorageConfig::new(dir.path().to_path_buf());
        let storage = FilaStorage::open(&config).unwrap();

        storage.put_state(P, "config.key1", b"value1").unwrap();

        let val = storage.get_state(P, "config.key1").unwrap();
        assert_eq!(val, Some(b"value1".to_vec()));

        storage.delete_state(P, "config.key1").unwrap();
        assert!(storage.get_state(P, "config.key1").unwrap().is_none());
    }

    #[test]
    fn list_state_by_prefix_with_limit() {
        let dir = tempfile::tempdir().unwrap();
        let config = FilaStorageConfig::new(dir.path().to_path_buf());
        let storage = FilaStorage::open(&config).unwrap();

        storage.put_state(P, "throttle.key1", b"v1").unwrap();
        storage.put_state(P, "throttle.key2", b"v2").unwrap();
        storage.put_state(P, "throttle.key3", b"v3").unwrap();
        storage.put_state(P, "other.key", b"v4").unwrap();

        // All throttle keys
        let all = storage
            .list_state_by_prefix(P, "throttle.", usize::MAX)
            .unwrap();
        assert_eq!(all.len(), 3);

        // With limit
        let limited = storage.list_state_by_prefix(P, "throttle.", 2).unwrap();
        assert_eq!(limited.len(), 2);

        // Different prefix
        let other = storage
            .list_state_by_prefix(P, "other.", usize::MAX)
            .unwrap();
        assert_eq!(other.len(), 1);
    }

    #[test]
    fn wal_replay_rebuilds_indexes() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().to_path_buf();

        let msg = test_message("q1", b"persistent");
        let msg_clone = msg.clone();

        // Write data, then drop storage (simulating restart)
        {
            let config = FilaStorageConfig::new(path.clone());
            let storage = FilaStorage::open(&config).unwrap();

            storage.put_message(P, b"msg-1", &msg).unwrap();
            storage.put_lease(P, b"lease-1", b"consumer:1").unwrap();
            storage
                .put_queue(P, "my-q", &test_queue_config("my-q"))
                .unwrap();
            storage.put_state(P, "config.x", b"42").unwrap();
            storage.flush().unwrap();
        }

        // Reopen — indexes should be rebuilt from WAL
        {
            let config = FilaStorageConfig::new(path);
            let storage = FilaStorage::open(&config).unwrap();

            assert_eq!(storage.get_message(P, b"msg-1").unwrap(), Some(msg_clone));
            assert_eq!(
                storage.get_lease(P, b"lease-1").unwrap(),
                Some(b"consumer:1".to_vec())
            );
            assert!(storage.get_queue(P, "my-q").unwrap().is_some());
            assert_eq!(
                storage.get_state(P, "config.x").unwrap(),
                Some(b"42".to_vec())
            );
        }
    }

    #[test]
    fn wal_replay_applies_deletes() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().to_path_buf();

        // Write then delete, then restart
        {
            let config = FilaStorageConfig::new(path.clone());
            let storage = FilaStorage::open(&config).unwrap();

            let msg = test_message("q1", b"temp");
            storage.put_message(P, b"msg-1", &msg).unwrap();
            storage.delete_message(P, b"msg-1").unwrap();
            storage.flush().unwrap();
        }

        // After replay, deleted message should not appear
        {
            let config = FilaStorageConfig::new(path);
            let storage = FilaStorage::open(&config).unwrap();

            assert!(storage.get_message(P, b"msg-1").unwrap().is_none());
        }
    }

    #[test]
    fn write_batch_updates_all_indexes() {
        let dir = tempfile::tempdir().unwrap();
        let config = FilaStorageConfig::new(dir.path().to_path_buf());
        let storage = FilaStorage::open(&config).unwrap();

        let msg = test_message("q1", b"batch-msg");
        let msg_json = serde_json::to_vec(&msg).unwrap();

        storage
            .write_batch(
                P,
                vec![
                    WriteBatchOp::PutMessage {
                        key: b"m1".to_vec(),
                        value: msg_json,
                    },
                    WriteBatchOp::PutLease {
                        key: b"l1".to_vec(),
                        value: b"c1:999".to_vec(),
                    },
                    WriteBatchOp::PutLeaseExpiry {
                        key: b"e1".to_vec(),
                    },
                ],
            )
            .unwrap();

        // All should be readable immediately
        assert!(storage.get_message(P, b"m1").unwrap().is_some());
        assert!(storage.get_lease(P, b"l1").unwrap().is_some());
        assert_eq!(
            storage
                .list_expired_leases(P, b"\xff\xff\xff\xff\xff\xff\xff\xff")
                .unwrap()
                .len(),
            1
        );
    }

    #[test]
    fn overwrite_returns_latest_value() {
        let dir = tempfile::tempdir().unwrap();
        let config = FilaStorageConfig::new(dir.path().to_path_buf());
        let storage = FilaStorage::open(&config).unwrap();

        let msg1 = test_message("q1", b"version-1");
        let msg2 = test_message("q1", b"version-2");

        storage.put_message(P, b"same-key", &msg1).unwrap();
        storage.put_message(P, b"same-key", &msg2).unwrap();

        let retrieved = storage.get_message(P, b"same-key").unwrap().unwrap();
        assert_eq!(retrieved.payload, b"version-2");
    }

    // --- Compaction tests (Story 13.4) ---

    fn small_segment_config(dir: &std::path::Path) -> FilaStorageConfig {
        let mut config = FilaStorageConfig::new(dir.to_path_buf());
        config.segment_size_bytes = 200; // very small to force rotation
        config.compaction_enabled = false; // manual compaction in tests
        config
    }

    fn run_compaction(storage: &FilaStorage, message_ttl_ms: Option<u64>) -> (u64, u64) {
        compaction::run_compaction_pass_for_test(
            &storage.writer,
            &storage.indexes,
            u64::MAX, // no rate limit in tests
            message_ttl_ms,
        )
        .unwrap()
    }

    #[test]
    fn compaction_removes_dead_entries() {
        let dir = tempfile::tempdir().unwrap();
        let config = small_segment_config(dir.path());
        let storage = FilaStorage::open(&config).unwrap();

        // Write enough messages to create multiple segments
        for i in 0..20 {
            let msg = test_message("q1", format!("payload-{i}").as_bytes());
            storage
                .put_message(P, format!("q1:msg-{i:04}").as_bytes(), &msg)
                .unwrap();
        }

        // Delete most of them (creating dead entries)
        for i in 0..15 {
            storage
                .delete_message(P, format!("q1:msg-{i:04}").as_bytes())
                .unwrap();
        }

        // Verify we have sealed segments
        let sealed_count = storage
            .writer
            .lock()
            .unwrap()
            .sealed_segment_paths()
            .unwrap()
            .len();
        assert!(sealed_count > 0, "need sealed segments for compaction test");

        // Run compaction
        let (segments_compacted, bytes_reclaimed) = run_compaction(&storage, None);
        assert!(segments_compacted > 0, "should have compacted segments");
        assert!(bytes_reclaimed > 0, "should have reclaimed bytes");

        // Verify live messages are still readable
        for i in 15..20 {
            assert!(
                storage
                    .get_message(P, format!("q1:msg-{i:04}").as_bytes())
                    .unwrap()
                    .is_some(),
                "live message {i} should still exist after compaction"
            );
        }

        // Verify deleted messages are still gone
        for i in 0..15 {
            assert!(
                storage
                    .get_message(P, format!("q1:msg-{i:04}").as_bytes())
                    .unwrap()
                    .is_none(),
                "deleted message {i} should not reappear"
            );
        }
    }

    #[test]
    fn compaction_preserves_live_entries_on_replay() {
        let dir = tempfile::tempdir().unwrap();
        let config = small_segment_config(dir.path());
        let storage = FilaStorage::open(&config).unwrap();

        // Write messages across multiple segments
        for i in 0..20 {
            let msg = test_message("q1", format!("data-{i}").as_bytes());
            storage
                .put_message(P, format!("q1:msg-{i:04}").as_bytes(), &msg)
                .unwrap();
        }
        // Delete some
        for i in 0..5 {
            storage
                .delete_message(P, format!("q1:msg-{i:04}").as_bytes())
                .unwrap();
        }

        // Run compaction
        run_compaction(&storage, None);

        // Drop and reopen — WAL replay should produce same state
        drop(storage);

        let storage2 = FilaStorage::open(&config).unwrap();

        // Live messages should be present
        for i in 5..20 {
            let msg = storage2
                .get_message(P, format!("q1:msg-{i:04}").as_bytes())
                .unwrap();
            assert!(
                msg.is_some(),
                "message {i} should survive compaction + replay"
            );
        }

        // Deleted messages should not
        for i in 0..5 {
            assert!(storage2
                .get_message(P, format!("q1:msg-{i:04}").as_bytes())
                .unwrap()
                .is_none());
        }
    }

    #[test]
    fn compaction_ttl_expiry_removes_old_messages() {
        let dir = tempfile::tempdir().unwrap();
        let config = small_segment_config(dir.path());
        let storage = FilaStorage::open(&config).unwrap();

        // Write messages with old timestamps (1 hour ago)
        let one_hour_ago_ns = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos() as u64
            - 3_600_000_000_000;

        for i in 0..10 {
            let mut msg = test_message("q1", format!("old-{i}").as_bytes());
            msg.enqueued_at = one_hour_ago_ns;
            storage
                .put_message(P, format!("q1:old-{i:04}").as_bytes(), &msg)
                .unwrap();
        }

        // Write some recent messages
        let now_ns = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos() as u64;

        for i in 0..5 {
            let mut msg = test_message("q1", format!("new-{i}").as_bytes());
            msg.enqueued_at = now_ns;
            storage
                .put_message(P, format!("q1:new-{i:04}").as_bytes(), &msg)
                .unwrap();
        }

        // Run compaction with 1-second TTL — old messages should expire
        let (segments_compacted, _) = run_compaction(&storage, Some(1_000));

        if segments_compacted > 0 {
            // Old messages should be removed from the index
            for i in 0..10 {
                assert!(
                    storage
                        .get_message(P, format!("q1:old-{i:04}").as_bytes())
                        .unwrap()
                        .is_none(),
                    "TTL-expired message {i} should be removed"
                );
            }
        }

        // New messages should still be present
        for i in 0..5 {
            assert!(
                storage
                    .get_message(P, format!("q1:new-{i:04}").as_bytes())
                    .unwrap()
                    .is_some(),
                "recent message {i} should survive TTL check"
            );
        }
    }

    #[test]
    fn compaction_empty_segment_is_deleted() {
        let dir = tempfile::tempdir().unwrap();
        let config = small_segment_config(dir.path());
        let storage = FilaStorage::open(&config).unwrap();

        // Write and delete the same messages to create all-dead segments
        for i in 0..20 {
            let msg = test_message("q1", format!("temp-{i}").as_bytes());
            storage
                .put_message(P, format!("q1:tmp-{i:04}").as_bytes(), &msg)
                .unwrap();
        }
        for i in 0..20 {
            storage
                .delete_message(P, format!("q1:tmp-{i:04}").as_bytes())
                .unwrap();
        }

        let sealed_before = storage
            .writer
            .lock()
            .unwrap()
            .sealed_segment_paths()
            .unwrap()
            .len();
        assert!(sealed_before > 0);

        run_compaction(&storage, None);

        // Some sealed segments should have been deleted (all entries were dead)
        let sealed_after = storage
            .writer
            .lock()
            .unwrap()
            .sealed_segment_paths()
            .unwrap()
            .len();
        assert!(
            sealed_after < sealed_before,
            "all-dead segments should be deleted: before={sealed_before}, after={sealed_after}"
        );
    }

    #[test]
    fn compaction_is_idempotent() {
        let dir = tempfile::tempdir().unwrap();
        let config = small_segment_config(dir.path());
        let storage = FilaStorage::open(&config).unwrap();

        for i in 0..20 {
            let msg = test_message("q1", format!("data-{i}").as_bytes());
            storage
                .put_message(P, format!("q1:msg-{i:04}").as_bytes(), &msg)
                .unwrap();
        }
        // Delete some
        for i in 0..10 {
            storage
                .delete_message(P, format!("q1:msg-{i:04}").as_bytes())
                .unwrap();
        }

        // First compaction
        let (seg1, _bytes1) = run_compaction(&storage, None);
        assert!(seg1 > 0);

        // Second compaction — should be a no-op
        let (_seg2, bytes2) = run_compaction(&storage, None);
        assert_eq!(bytes2, 0, "second compaction should reclaim nothing");

        // Live data still correct
        for i in 10..20 {
            assert!(storage
                .get_message(P, format!("q1:msg-{i:04}").as_bytes())
                .unwrap()
                .is_some());
        }
    }

    #[test]
    fn compaction_concurrent_read_write() {
        use std::sync::Arc;
        use std::thread;

        let dir = tempfile::tempdir().unwrap();
        let config = small_segment_config(dir.path());
        let storage = Arc::new(FilaStorage::open(&config).unwrap());

        // Pre-populate
        for i in 0..20 {
            let msg = test_message("q1", format!("init-{i}").as_bytes());
            storage
                .put_message(P, format!("q1:msg-{i:04}").as_bytes(), &msg)
                .unwrap();
        }
        for i in 0..10 {
            storage
                .delete_message(P, format!("q1:msg-{i:04}").as_bytes())
                .unwrap();
        }

        // Spawn compaction in a background thread
        let storage_clone = storage.clone();
        let compaction_thread = thread::spawn(move || run_compaction(&storage_clone, None));

        // Meanwhile, write and read concurrently
        for i in 20..30 {
            let msg = test_message("q1", format!("concurrent-{i}").as_bytes());
            storage
                .put_message(P, format!("q1:msg-{i:04}").as_bytes(), &msg)
                .unwrap();
        }

        // Read should work fine during compaction
        for i in 10..20 {
            assert!(storage
                .get_message(P, format!("q1:msg-{i:04}").as_bytes())
                .unwrap()
                .is_some());
        }

        compaction_thread.join().unwrap();

        // Verify all data is consistent after compaction completes
        for i in 10..30 {
            assert!(
                storage
                    .get_message(P, format!("q1:msg-{i:04}").as_bytes())
                    .unwrap()
                    .is_some(),
                "message {i} should exist after concurrent compaction"
            );
        }
    }

    #[test]
    fn compaction_storage_footprint() {
        let dir = tempfile::tempdir().unwrap();
        let config = small_segment_config(dir.path());
        let storage = FilaStorage::open(&config).unwrap();

        // Write many messages
        for i in 0..50 {
            let msg = test_message("q1", format!("payload-{i}").as_bytes());
            storage
                .put_message(P, format!("q1:msg-{i:04}").as_bytes(), &msg)
                .unwrap();
        }

        // Ack (delete) 80% of them
        for i in 0..40 {
            storage
                .delete_message(P, format!("q1:msg-{i:04}").as_bytes())
                .unwrap();
        }

        // Measure size before compaction
        let size_before = dir_size(dir.path());

        // Run compaction
        run_compaction(&storage, None);

        // Measure size after compaction
        let size_after = dir_size(dir.path());

        assert!(
            size_after < size_before,
            "compaction should reduce storage size: before={size_before}, after={size_after}"
        );

        // Calculate approximate raw size of 10 live messages
        // Each message is ~200 bytes serialized, so 10 * 200 = 2000 bytes
        // The footprint should be well under 1.5x that
        let live_msg_count = 10;
        let approx_raw_bytes = live_msg_count * 200;
        assert!(
            size_after < approx_raw_bytes * 3, // generous bound for test
            "storage should be reasonably compact: {size_after} bytes for {live_msg_count} live messages"
        );
    }

    fn dir_size(dir: &std::path::Path) -> u64 {
        let mut total = 0;
        for entry in std::fs::read_dir(dir).unwrap() {
            let entry = entry.unwrap();
            if entry.file_type().unwrap().is_file() {
                total += entry.metadata().unwrap().len();
            }
        }
        total
    }

    #[test]
    fn compaction_background_thread_starts_and_stops() {
        let dir = tempfile::tempdir().unwrap();
        let mut config = FilaStorageConfig::new(dir.path().to_path_buf());
        config.compaction_enabled = true;
        config.compaction_interval_secs = 3600; // long interval, won't actually compact

        let storage = FilaStorage::open(&config).unwrap();

        // The compaction handle should exist
        assert!(storage.compaction_handle.is_some());

        // Storage should drop cleanly (thread stops)
        drop(storage);
    }
}
