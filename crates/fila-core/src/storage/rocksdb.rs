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

    fn cf(
        &self,
        name: &'static str,
    ) -> StorageResult<std::sync::Arc<rocksdb::BoundColumnFamily<'_>>> {
        self.db
            .cf_handle(name)
            .ok_or(StorageError::ColumnFamilyNotFound(name))
    }
}

impl Storage for RocksDbStorage {
    fn put_message(&self, key: &[u8], message: &Message) -> StorageResult<()> {
        let cf = self.cf(CF_MESSAGES)?;
        let value = serde_json::to_vec(message)?;
        self.db.put_cf(&cf, key, &value)?;
        Ok(())
    }

    fn get_message(&self, key: &[u8]) -> StorageResult<Option<Message>> {
        let cf = self.cf(CF_MESSAGES)?;
        match self.db.get_cf(&cf, key)? {
            Some(value) => Ok(Some(serde_json::from_slice(&value)?)),
            None => Ok(None),
        }
    }

    fn delete_message(&self, key: &[u8]) -> StorageResult<()> {
        let cf = self.cf(CF_MESSAGES)?;
        self.db.delete_cf(&cf, key)?;
        Ok(())
    }

    fn list_messages(&self, prefix: &[u8]) -> StorageResult<Vec<(Vec<u8>, Message)>> {
        let cf = self.cf(CF_MESSAGES)?;
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
        let cf = self.cf(CF_LEASES)?;
        self.db.put_cf(&cf, key, value)?;
        Ok(())
    }

    fn get_lease(&self, key: &[u8]) -> StorageResult<Option<Vec<u8>>> {
        let cf = self.cf(CF_LEASES)?;
        Ok(self.db.get_cf(&cf, key)?.map(|v| v.to_vec()))
    }

    fn delete_lease(&self, key: &[u8]) -> StorageResult<()> {
        let cf = self.cf(CF_LEASES)?;
        self.db.delete_cf(&cf, key)?;
        Ok(())
    }

    fn list_expired_leases(&self, up_to_key: &[u8]) -> StorageResult<Vec<Vec<u8>>> {
        let cf = self.cf(CF_LEASE_EXPIRY)?;
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
        let cf = self.cf(CF_QUEUES)?;
        let value = serde_json::to_vec(config)?;
        self.db.put_cf(&cf, queue_id.as_bytes(), &value)?;
        Ok(())
    }

    fn get_queue(&self, queue_id: &str) -> StorageResult<Option<QueueConfig>> {
        let cf = self.cf(CF_QUEUES)?;
        match self.db.get_cf(&cf, queue_id.as_bytes())? {
            Some(value) => Ok(Some(serde_json::from_slice(&value)?)),
            None => Ok(None),
        }
    }

    fn delete_queue(&self, queue_id: &str) -> StorageResult<()> {
        let cf = self.cf(CF_QUEUES)?;
        self.db.delete_cf(&cf, queue_id.as_bytes())?;
        Ok(())
    }

    fn list_queues(&self) -> StorageResult<Vec<QueueConfig>> {
        let cf = self.cf(CF_QUEUES)?;
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
        let cf = self.cf(CF_STATE)?;
        self.db.put_cf(&cf, key.as_bytes(), value)?;
        Ok(())
    }

    fn get_state(&self, key: &str) -> StorageResult<Option<Vec<u8>>> {
        let cf = self.cf(CF_STATE)?;
        Ok(self.db.get_cf(&cf, key.as_bytes())?.map(|v| v.to_vec()))
    }

    fn delete_state(&self, key: &str) -> StorageResult<()> {
        let cf = self.cf(CF_STATE)?;
        self.db.delete_cf(&cf, key.as_bytes())?;
        Ok(())
    }

    fn list_state_by_prefix(&self, prefix: &str) -> StorageResult<Vec<(String, Vec<u8>)>> {
        let cf = self.cf(CF_STATE)?;
        let mut result = Vec::new();
        let iter = self.db.prefix_iterator_cf(&cf, prefix.as_bytes());
        for item in iter {
            let (key, value) = item?;
            let key_str = std::str::from_utf8(&key)
                .map_err(|e| StorageError::CorruptData(format!("non-UTF8 state key: {e}")))?;
            if !key_str.starts_with(prefix) {
                break;
            }
            result.push((key_str.to_string(), value.to_vec()));
        }
        Ok(result)
    }

    fn write_batch(&self, ops: Vec<WriteBatchOp>) -> StorageResult<()> {
        let mut batch = WriteBatch::default();

        for op in ops {
            match op {
                WriteBatchOp::PutMessage { key, value } => {
                    batch.put_cf(&self.cf(CF_MESSAGES)?, &key, &value);
                }
                WriteBatchOp::DeleteMessage { key } => {
                    batch.delete_cf(&self.cf(CF_MESSAGES)?, &key);
                }
                WriteBatchOp::PutLease { key, value } => {
                    batch.put_cf(&self.cf(CF_LEASES)?, &key, &value);
                }
                WriteBatchOp::DeleteLease { key } => {
                    batch.delete_cf(&self.cf(CF_LEASES)?, &key);
                }
                WriteBatchOp::PutLeaseExpiry { key } => {
                    batch.put_cf(&self.cf(CF_LEASE_EXPIRY)?, &key, b"");
                }
                WriteBatchOp::DeleteLeaseExpiry { key } => {
                    batch.delete_cf(&self.cf(CF_LEASE_EXPIRY)?, &key);
                }
                WriteBatchOp::PutQueue { key, value } => {
                    batch.put_cf(&self.cf(CF_QUEUES)?, &key, &value);
                }
                WriteBatchOp::DeleteQueue { key } => {
                    batch.delete_cf(&self.cf(CF_QUEUES)?, &key);
                }
                WriteBatchOp::PutState { key, value } => {
                    batch.put_cf(&self.cf(CF_STATE)?, &key, &value);
                }
                WriteBatchOp::DeleteState { key } => {
                    batch.delete_cf(&self.cf(CF_STATE)?, &key);
                }
            }
        }

        self.db.write(batch)?;
        Ok(())
    }

    fn flush(&self) -> StorageResult<()> {
        self.db
            .flush_wal(true)
            .map_err(|e| StorageError::RocksDb(e.to_string()))?;
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
    fn list_state_by_prefix_returns_matching_entries() {
        let (storage, _dir) = test_storage();

        storage.put_state("throttle.a", b"10,100").unwrap();
        storage.put_state("throttle.b", b"20,200").unwrap();
        storage.put_state("app.flag", b"true").unwrap();

        let entries = storage.list_state_by_prefix("throttle.").unwrap();
        assert_eq!(entries.len(), 2);
        assert!(entries
            .iter()
            .any(|(k, v)| k == "throttle.a" && v == b"10,100"));
        assert!(entries
            .iter()
            .any(|(k, v)| k == "throttle.b" && v == b"20,200"));

        // Non-matching prefix returns empty
        let entries = storage.list_state_by_prefix("nonexistent.").unwrap();
        assert!(entries.is_empty());
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
