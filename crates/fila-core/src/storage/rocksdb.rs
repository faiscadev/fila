use std::path::Path;

use rocksdb::{
    ColumnFamilyDescriptor, DBWithThreadMode, IteratorMode, MultiThreaded, Options, WriteBatch,
};

use crate::error::{StorageError, StorageResult};
use crate::message::Message;
use crate::queue::QueueConfig;
use crate::storage::traits::{Storage, WriteBatchOp};

const CF_MESSAGES: &str = "messages";
const CF_LEASES: &str = "leases";
const CF_LEASE_EXPIRY: &str = "lease_expiry";
const CF_QUEUES: &str = "queues";
const CF_STATE: &str = "state";

/// All column family names (excluding `default` which RocksDB creates automatically).
const COLUMN_FAMILIES: &[&str] = &[CF_MESSAGES, CF_LEASES, CF_LEASE_EXPIRY, CF_QUEUES, CF_STATE];

type DB = DBWithThreadMode<MultiThreaded>;

/// RocksDB-backed storage implementation.
pub struct RocksDbStorage {
    db: DB,
}

impl RocksDbStorage {
    /// Open or create a RocksDB database at the given path with all column families.
    pub fn open(path: impl AsRef<Path>) -> StorageResult<Self> {
        let mut db_opts = Options::default();
        db_opts.create_if_missing(true);
        db_opts.create_missing_column_families(true);

        let cf_descriptors: Vec<ColumnFamilyDescriptor> = COLUMN_FAMILIES
            .iter()
            .map(|name| {
                let cf_opts = Options::default();
                ColumnFamilyDescriptor::new(*name, cf_opts)
            })
            .collect();

        let db = DB::open_cf_descriptors(&db_opts, path, cf_descriptors)?;
        Ok(Self { db })
    }
}

impl Storage for RocksDbStorage {
    fn put_message(&self, key: &[u8], message: &Message) -> StorageResult<()> {
        let cf = self.db.cf_handle(CF_MESSAGES).ok_or_else(|| {
            StorageError::RocksDb(format!("column family not found: {CF_MESSAGES}"))
        })?;
        let value = serde_json::to_vec(message)?;
        self.db.put_cf(&cf, key, &value)?;
        Ok(())
    }

    fn get_message(&self, key: &[u8]) -> StorageResult<Option<Message>> {
        let cf = self.db.cf_handle(CF_MESSAGES).ok_or_else(|| {
            StorageError::RocksDb(format!("column family not found: {CF_MESSAGES}"))
        })?;
        match self.db.get_cf(&cf, key)? {
            Some(value) => Ok(Some(serde_json::from_slice(&value)?)),
            None => Ok(None),
        }
    }

    fn delete_message(&self, key: &[u8]) -> StorageResult<()> {
        let cf = self.db.cf_handle(CF_MESSAGES).ok_or_else(|| {
            StorageError::RocksDb(format!("column family not found: {CF_MESSAGES}"))
        })?;
        self.db.delete_cf(&cf, key)?;
        Ok(())
    }

    fn list_messages(&self, prefix: &[u8]) -> StorageResult<Vec<(Vec<u8>, Message)>> {
        let cf = self.db.cf_handle(CF_MESSAGES).ok_or_else(|| {
            StorageError::RocksDb(format!("column family not found: {CF_MESSAGES}"))
        })?;
        let iter = self
            .db
            .iterator_cf(&cf, IteratorMode::From(prefix, rocksdb::Direction::Forward));
        let mut results = Vec::new();
        for item in iter {
            let (key, value) = item?;
            if !key.starts_with(prefix) {
                break;
            }
            let msg: Message = serde_json::from_slice(&value)?;
            results.push((key.to_vec(), msg));
        }
        Ok(results)
    }

    fn put_lease(&self, key: &[u8], value: &[u8]) -> StorageResult<()> {
        let cf = self.db.cf_handle(CF_LEASES).ok_or_else(|| {
            StorageError::RocksDb(format!("column family not found: {CF_LEASES}"))
        })?;
        self.db.put_cf(&cf, key, value)?;
        Ok(())
    }

    fn get_lease(&self, key: &[u8]) -> StorageResult<Option<Vec<u8>>> {
        let cf = self.db.cf_handle(CF_LEASES).ok_or_else(|| {
            StorageError::RocksDb(format!("column family not found: {CF_LEASES}"))
        })?;
        Ok(self.db.get_cf(&cf, key)?.map(|v| v.to_vec()))
    }

    fn delete_lease(&self, key: &[u8]) -> StorageResult<()> {
        let cf = self.db.cf_handle(CF_LEASES).ok_or_else(|| {
            StorageError::RocksDb(format!("column family not found: {CF_LEASES}"))
        })?;
        self.db.delete_cf(&cf, key)?;
        Ok(())
    }

    fn list_expired_leases(&self, up_to_key: &[u8]) -> StorageResult<Vec<Vec<u8>>> {
        let cf = self.db.cf_handle(CF_LEASE_EXPIRY).ok_or_else(|| {
            StorageError::RocksDb(format!("column family not found: {CF_LEASE_EXPIRY}"))
        })?;
        let iter = self.db.iterator_cf(&cf, IteratorMode::Start);
        let mut results = Vec::new();
        for item in iter {
            let (key, _) = item?;
            if key.as_ref() > up_to_key {
                break;
            }
            results.push(key.to_vec());
        }
        Ok(results)
    }

    fn put_queue(&self, queue_id: &str, config: &QueueConfig) -> StorageResult<()> {
        let cf = self.db.cf_handle(CF_QUEUES).ok_or_else(|| {
            StorageError::RocksDb(format!("column family not found: {CF_QUEUES}"))
        })?;
        let value = serde_json::to_vec(config)?;
        self.db.put_cf(&cf, queue_id.as_bytes(), &value)?;
        Ok(())
    }

    fn get_queue(&self, queue_id: &str) -> StorageResult<Option<QueueConfig>> {
        let cf = self.db.cf_handle(CF_QUEUES).ok_or_else(|| {
            StorageError::RocksDb(format!("column family not found: {CF_QUEUES}"))
        })?;
        match self.db.get_cf(&cf, queue_id.as_bytes())? {
            Some(value) => Ok(Some(serde_json::from_slice(&value)?)),
            None => Ok(None),
        }
    }

    fn delete_queue(&self, queue_id: &str) -> StorageResult<()> {
        let cf = self.db.cf_handle(CF_QUEUES).ok_or_else(|| {
            StorageError::RocksDb(format!("column family not found: {CF_QUEUES}"))
        })?;
        self.db.delete_cf(&cf, queue_id.as_bytes())?;
        Ok(())
    }

    fn list_queues(&self) -> StorageResult<Vec<QueueConfig>> {
        let cf = self.db.cf_handle(CF_QUEUES).ok_or_else(|| {
            StorageError::RocksDb(format!("column family not found: {CF_QUEUES}"))
        })?;
        let iter = self.db.iterator_cf(&cf, IteratorMode::Start);
        let mut results = Vec::new();
        for item in iter {
            let (_, value) = item?;
            let config: QueueConfig = serde_json::from_slice(&value)?;
            results.push(config);
        }
        Ok(results)
    }

    fn put_state(&self, key: &str, value: &[u8]) -> StorageResult<()> {
        let cf = self
            .db
            .cf_handle(CF_STATE)
            .ok_or_else(|| StorageError::RocksDb(format!("column family not found: {CF_STATE}")))?;
        self.db.put_cf(&cf, key.as_bytes(), value)?;
        Ok(())
    }

    fn get_state(&self, key: &str) -> StorageResult<Option<Vec<u8>>> {
        let cf = self
            .db
            .cf_handle(CF_STATE)
            .ok_or_else(|| StorageError::RocksDb(format!("column family not found: {CF_STATE}")))?;
        Ok(self.db.get_cf(&cf, key.as_bytes())?.map(|v| v.to_vec()))
    }

    fn delete_state(&self, key: &str) -> StorageResult<()> {
        let cf = self
            .db
            .cf_handle(CF_STATE)
            .ok_or_else(|| StorageError::RocksDb(format!("column family not found: {CF_STATE}")))?;
        self.db.delete_cf(&cf, key.as_bytes())?;
        Ok(())
    }

    fn write_batch(&self, ops: Vec<WriteBatchOp>) -> StorageResult<()> {
        let mut batch = WriteBatch::default();

        for op in ops {
            match op {
                WriteBatchOp::PutMessage { key, value } => {
                    let cf = self.db.cf_handle(CF_MESSAGES).ok_or_else(|| {
                        StorageError::RocksDb(format!("column family not found: {CF_MESSAGES}"))
                    })?;
                    batch.put_cf(&cf, &key, &value);
                }
                WriteBatchOp::DeleteMessage { key } => {
                    let cf = self.db.cf_handle(CF_MESSAGES).ok_or_else(|| {
                        StorageError::RocksDb(format!("column family not found: {CF_MESSAGES}"))
                    })?;
                    batch.delete_cf(&cf, &key);
                }
                WriteBatchOp::PutLease { key, value } => {
                    let cf = self.db.cf_handle(CF_LEASES).ok_or_else(|| {
                        StorageError::RocksDb(format!("column family not found: {CF_LEASES}"))
                    })?;
                    batch.put_cf(&cf, &key, &value);
                }
                WriteBatchOp::DeleteLease { key } => {
                    let cf = self.db.cf_handle(CF_LEASES).ok_or_else(|| {
                        StorageError::RocksDb(format!("column family not found: {CF_LEASES}"))
                    })?;
                    batch.delete_cf(&cf, &key);
                }
                WriteBatchOp::PutLeaseExpiry { key } => {
                    let cf = self.db.cf_handle(CF_LEASE_EXPIRY).ok_or_else(|| {
                        StorageError::RocksDb(format!("column family not found: {CF_LEASE_EXPIRY}"))
                    })?;
                    batch.put_cf(&cf, &key, b"");
                }
                WriteBatchOp::DeleteLeaseExpiry { key } => {
                    let cf = self.db.cf_handle(CF_LEASE_EXPIRY).ok_or_else(|| {
                        StorageError::RocksDb(format!("column family not found: {CF_LEASE_EXPIRY}"))
                    })?;
                    batch.delete_cf(&cf, &key);
                }
                WriteBatchOp::PutQueue { key, value } => {
                    let cf = self.db.cf_handle(CF_QUEUES).ok_or_else(|| {
                        StorageError::RocksDb(format!("column family not found: {CF_QUEUES}"))
                    })?;
                    batch.put_cf(&cf, &key, &value);
                }
                WriteBatchOp::DeleteQueue { key } => {
                    let cf = self.db.cf_handle(CF_QUEUES).ok_or_else(|| {
                        StorageError::RocksDb(format!("column family not found: {CF_QUEUES}"))
                    })?;
                    batch.delete_cf(&cf, &key);
                }
                WriteBatchOp::PutState { key, value } => {
                    let cf = self.db.cf_handle(CF_STATE).ok_or_else(|| {
                        StorageError::RocksDb(format!("column family not found: {CF_STATE}"))
                    })?;
                    batch.put_cf(&cf, &key, &value);
                }
                WriteBatchOp::DeleteState { key } => {
                    let cf = self.db.cf_handle(CF_STATE).ok_or_else(|| {
                        StorageError::RocksDb(format!("column family not found: {CF_STATE}"))
                    })?;
                    batch.delete_cf(&cf, &key);
                }
            }
        }

        self.db.write(batch)?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::storage::keys;
    use std::collections::HashMap;
    use uuid::Uuid;

    fn test_storage() -> (RocksDbStorage, tempfile::TempDir) {
        let dir = tempfile::tempdir().unwrap();
        let storage = RocksDbStorage::open(dir.path()).unwrap();
        (storage, dir)
    }

    fn test_message(queue_id: &str, fairness_key: &str) -> Message {
        Message {
            id: Uuid::now_v7(),
            queue_id: queue_id.to_string(),
            headers: HashMap::new(),
            payload: vec![1, 2, 3],
            fairness_key: fairness_key.to_string(),
            weight: 1,
            throttle_keys: vec![],
            attempt_count: 0,
            enqueued_at: 1_000_000_000,
            leased_at: None,
        }
    }

    #[test]
    fn open_creates_all_column_families() {
        let (storage, _dir) = test_storage();
        for cf_name in COLUMN_FAMILIES {
            assert!(
                storage.db.cf_handle(cf_name).is_some(),
                "column family '{cf_name}' should exist"
            );
        }
    }

    #[test]
    fn message_put_get_delete() {
        let (storage, _dir) = test_storage();
        let msg = test_message("q1", "default");
        let key = keys::message_key(&msg.queue_id, &msg.fairness_key, msg.enqueued_at, &msg.id);

        storage.put_message(&key, &msg).unwrap();
        let retrieved = storage.get_message(&key).unwrap().unwrap();
        assert_eq!(retrieved, msg);

        storage.delete_message(&key).unwrap();
        assert!(storage.get_message(&key).unwrap().is_none());
    }

    #[test]
    fn message_get_nonexistent_returns_none() {
        let (storage, _dir) = test_storage();
        let id = Uuid::now_v7();
        let key = keys::message_key("q1", "default", 1000, &id);
        assert!(storage.get_message(&key).unwrap().is_none());
    }

    #[test]
    fn list_messages_by_prefix() {
        let (storage, _dir) = test_storage();

        let msg1 = test_message("q1", "tenant_a");
        let msg2 = {
            let mut m = test_message("q1", "tenant_a");
            m.enqueued_at = 2_000_000_000;
            m
        };
        let msg3 = test_message("q2", "tenant_a");

        let k1 = keys::message_key(
            &msg1.queue_id,
            &msg1.fairness_key,
            msg1.enqueued_at,
            &msg1.id,
        );
        let k2 = keys::message_key(
            &msg2.queue_id,
            &msg2.fairness_key,
            msg2.enqueued_at,
            &msg2.id,
        );
        let k3 = keys::message_key(
            &msg3.queue_id,
            &msg3.fairness_key,
            msg3.enqueued_at,
            &msg3.id,
        );

        storage.put_message(&k1, &msg1).unwrap();
        storage.put_message(&k2, &msg2).unwrap();
        storage.put_message(&k3, &msg3).unwrap();

        // List all q1 messages
        let prefix = keys::message_prefix("q1");
        let results = storage.list_messages(&prefix).unwrap();
        assert_eq!(results.len(), 2, "should find 2 messages in q1");

        // List q1 + tenant_a messages
        let prefix = keys::message_prefix_with_key("q1", "tenant_a");
        let results = storage.list_messages(&prefix).unwrap();
        assert_eq!(
            results.len(),
            2,
            "should find 2 messages for tenant_a in q1"
        );

        // List q2 messages
        let prefix = keys::message_prefix("q2");
        let results = storage.list_messages(&prefix).unwrap();
        assert_eq!(results.len(), 1, "should find 1 message in q2");
    }

    #[test]
    fn queue_put_get_delete_list() {
        let (storage, _dir) = test_storage();

        let config = QueueConfig::new("test-queue".to_string());
        storage.put_queue("test-queue", &config).unwrap();

        let retrieved = storage.get_queue("test-queue").unwrap().unwrap();
        assert_eq!(retrieved, config);

        let queues = storage.list_queues().unwrap();
        assert_eq!(queues.len(), 1);

        storage.delete_queue("test-queue").unwrap();
        assert!(storage.get_queue("test-queue").unwrap().is_none());
    }

    #[test]
    fn state_put_get_delete() {
        let (storage, _dir) = test_storage();

        storage.put_state("config:rate_limit", b"100").unwrap();
        let val = storage.get_state("config:rate_limit").unwrap().unwrap();
        assert_eq!(val, b"100");

        storage.delete_state("config:rate_limit").unwrap();
        assert!(storage.get_state("config:rate_limit").unwrap().is_none());
    }

    #[test]
    fn lease_put_get_delete() {
        let (storage, _dir) = test_storage();
        let id = Uuid::now_v7();

        let key = keys::lease_key("q1", &id);
        let value = keys::lease_value("consumer-1", 5_000_000_000);

        storage.put_lease(&key, &value).unwrap();
        let retrieved = storage.get_lease(&key).unwrap().unwrap();
        assert_eq!(retrieved, value);

        storage.delete_lease(&key).unwrap();
        assert!(storage.get_lease(&key).unwrap().is_none());
    }

    #[test]
    fn list_expired_leases() {
        let (storage, _dir) = test_storage();
        let id1 = Uuid::now_v7();
        let id2 = Uuid::now_v7();
        let id3 = Uuid::now_v7();

        // Create lease expiry entries at different times
        let ek1 = keys::lease_expiry_key(1000, "q1", &id1);
        let ek2 = keys::lease_expiry_key(2000, "q1", &id2);
        let ek3 = keys::lease_expiry_key(5000, "q1", &id3);

        storage
            .write_batch(vec![
                WriteBatchOp::PutLeaseExpiry { key: ek1.clone() },
                WriteBatchOp::PutLeaseExpiry { key: ek2.clone() },
                WriteBatchOp::PutLeaseExpiry { key: ek3.clone() },
            ])
            .unwrap();

        // Query leases expired up to timestamp 3000
        let up_to = keys::lease_expiry_key(3000, "q1", &Uuid::max());
        let expired = storage.list_expired_leases(&up_to).unwrap();
        assert_eq!(
            expired.len(),
            2,
            "should find 2 expired leases (1000, 2000)"
        );
        assert_eq!(expired[0], ek1);
        assert_eq!(expired[1], ek2);
    }

    #[test]
    fn write_batch_atomicity() {
        let (storage, _dir) = test_storage();
        let msg = test_message("q1", "default");
        let msg_key = keys::message_key(&msg.queue_id, &msg.fairness_key, msg.enqueued_at, &msg.id);
        let lease_key = keys::lease_key("q1", &msg.id);
        let expiry_key = keys::lease_expiry_key(5_000_000_000, "q1", &msg.id);
        let lease_val = keys::lease_value("consumer-1", 5_000_000_000);
        let msg_value = serde_json::to_vec(&msg).unwrap();

        // Atomic write: message + lease + lease_expiry
        storage
            .write_batch(vec![
                WriteBatchOp::PutMessage {
                    key: msg_key.clone(),
                    value: msg_value,
                },
                WriteBatchOp::PutLease {
                    key: lease_key.clone(),
                    value: lease_val.clone(),
                },
                WriteBatchOp::PutLeaseExpiry {
                    key: expiry_key.clone(),
                },
            ])
            .unwrap();

        // All three should exist
        assert!(storage.get_message(&msg_key).unwrap().is_some());
        assert!(storage.get_lease(&lease_key).unwrap().is_some());
        let up_to = keys::lease_expiry_key(u64::MAX, "q1", &Uuid::max());
        let expired = storage.list_expired_leases(&up_to).unwrap();
        assert_eq!(expired.len(), 1);

        // Atomic delete: message + lease + lease_expiry
        storage
            .write_batch(vec![
                WriteBatchOp::DeleteMessage {
                    key: msg_key.clone(),
                },
                WriteBatchOp::DeleteLease {
                    key: lease_key.clone(),
                },
                WriteBatchOp::DeleteLeaseExpiry { key: expiry_key },
            ])
            .unwrap();

        // All three should be gone
        assert!(storage.get_message(&msg_key).unwrap().is_none());
        assert!(storage.get_lease(&lease_key).unwrap().is_none());
        let expired = storage.list_expired_leases(&up_to).unwrap();
        assert!(expired.is_empty());
    }

    #[test]
    fn reopen_preserves_data() {
        let dir = tempfile::tempdir().unwrap();

        // Write data
        {
            let storage = RocksDbStorage::open(dir.path()).unwrap();
            let config = QueueConfig::new("persistent-queue".to_string());
            storage.put_queue("persistent-queue", &config).unwrap();
            storage.put_state("my-key", b"my-value").unwrap();
        }

        // Reopen and verify
        {
            let storage = RocksDbStorage::open(dir.path()).unwrap();
            let config = storage.get_queue("persistent-queue").unwrap().unwrap();
            assert_eq!(config.name, "persistent-queue");
            let val = storage.get_state("my-key").unwrap().unwrap();
            assert_eq!(val, b"my-value");
        }
    }
}
