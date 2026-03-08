pub mod config;
pub mod wal;

use std::sync::Mutex;

use tracing::warn;

use crate::error::{StorageError, StorageResult};
use crate::message::Message;
use crate::queue::QueueConfig;
use crate::storage::{PartitionId, Storage, WriteBatchOp};

use config::FilaStorageConfig;
use wal::{OpTag, WalEntry, WalOp, WalWriter};

/// Purpose-built storage engine for Fila's queue workload.
///
/// In this story (13.2) only the write path is implemented: all writes go
/// through the WAL. Read methods are stubbed and will be implemented in
/// Story 13.3 (Read Path & Indexing).
pub struct FilaStorage {
    writer: Mutex<WalWriter>,
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

        Ok(Self {
            writer: Mutex::new(writer),
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

    fn get_message(&self, _partition: &PartitionId, _key: &[u8]) -> StorageResult<Option<Message>> {
        warn!("FilaStorage::get_message is not yet implemented (Story 13.3)");
        Ok(None)
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
        _prefix: &[u8],
    ) -> StorageResult<Vec<(Vec<u8>, Message)>> {
        warn!("FilaStorage::list_messages is not yet implemented (Story 13.3)");
        Ok(vec![])
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

    fn get_lease(&self, _partition: &PartitionId, _key: &[u8]) -> StorageResult<Option<Vec<u8>>> {
        warn!("FilaStorage::get_lease is not yet implemented (Story 13.3)");
        Ok(None)
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
        _up_to_key: &[u8],
    ) -> StorageResult<Vec<Vec<u8>>> {
        warn!("FilaStorage::list_expired_leases is not yet implemented (Story 13.3)");
        Ok(vec![])
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
        _queue_id: &str,
    ) -> StorageResult<Option<QueueConfig>> {
        warn!("FilaStorage::get_queue is not yet implemented (Story 13.3)");
        Ok(None)
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
        warn!("FilaStorage::list_queues is not yet implemented (Story 13.3)");
        Ok(vec![])
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

    fn get_state(&self, _partition: &PartitionId, _key: &str) -> StorageResult<Option<Vec<u8>>> {
        warn!("FilaStorage::get_state is not yet implemented (Story 13.3)");
        Ok(None)
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
        _prefix: &str,
        _limit: usize,
    ) -> StorageResult<Vec<(String, Vec<u8>)>> {
        warn!("FilaStorage::list_state_by_prefix is not yet implemented (Story 13.3)");
        Ok(vec![])
    }

    // --- Batch operations ---

    fn write_batch(&self, _partition: &PartitionId, ops: Vec<WriteBatchOp>) -> StorageResult<()> {
        if ops.is_empty() {
            return Ok(());
        }

        let wal_ops: Vec<WalOp> = ops.into_iter().map(batch_op_to_wal_op).collect();
        let entry = WalEntry { ops: wal_ops };

        let mut writer = self
            .writer
            .lock()
            .map_err(|e| StorageError::Backend(format!("WAL writer lock poisoned: {e}")))?;

        writer
            .append(&entry)
            .map_err(|e| StorageError::Backend(format!("WAL append failed: {e}")))
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

    #[test]
    fn fila_storage_implements_storage_trait() {
        let dir = tempfile::tempdir().unwrap();
        let config = FilaStorageConfig::new(dir.path().to_path_buf());
        let storage = FilaStorage::open(&config).unwrap();

        // Verify it satisfies Send + Sync (compile-time check)
        fn assert_send_sync<T: Send + Sync>() {}
        assert_send_sync::<FilaStorage>();

        // Write via trait methods
        let msg = Message {
            id: uuid::Uuid::now_v7(),
            queue_id: "test-q".to_string(),
            headers: Default::default(),
            payload: b"hello".to_vec(),
            fairness_key: "default".to_string(),
            weight: 1,
            throttle_keys: vec![],
            attempt_count: 0,
            enqueued_at: 12345,
            leased_at: None,
        };

        storage.put_message(P, b"msg-key-1", &msg).unwrap();
        storage
            .put_lease(P, b"lease-key-1", b"consumer:999")
            .unwrap();
        storage.delete_message(P, b"msg-key-1").unwrap();
        storage.flush().unwrap();

        // Verify WAL contains the entries
        let reader = wal::WalReader::new(dir.path());
        let entries = reader.replay().unwrap();
        assert_eq!(entries.len(), 3);

        // First entry: PutMessage
        assert_eq!(entries[0].ops.len(), 1);
        assert_eq!(entries[0].ops[0].tag, OpTag::PutMessage);
        assert_eq!(entries[0].ops[0].key, b"msg-key-1");

        // Second entry: PutLease
        assert_eq!(entries[1].ops[0].tag, OpTag::PutLease);

        // Third entry: DeleteMessage
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
        // All 3 ops in a single WAL entry (atomic batch)
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].ops.len(), 3);
    }

    #[test]
    fn fila_storage_read_stubs_return_empty() {
        let dir = tempfile::tempdir().unwrap();
        let config = FilaStorageConfig::new(dir.path().to_path_buf());
        let storage = FilaStorage::open(&config).unwrap();

        assert!(storage.get_message(P, b"k").unwrap().is_none());
        assert!(storage.list_messages(P, b"prefix").unwrap().is_empty());
        assert!(storage.get_lease(P, b"k").unwrap().is_none());
        assert!(storage.list_expired_leases(P, b"k").unwrap().is_empty());
        assert!(storage.get_queue(P, "q").unwrap().is_none());
        assert!(storage.list_queues(P).unwrap().is_empty());
        assert!(storage.get_state(P, "k").unwrap().is_none());
        assert!(storage.list_state_by_prefix(P, "p", 10).unwrap().is_empty());
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
}
