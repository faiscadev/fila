use std::path::Path;

use rocksdb::{
    BlockBasedOptions, Cache, ColumnFamilyDescriptor, DBCompressionType, DBWithThreadMode,
    IteratorMode, MultiThreaded, Options, ReadOptions, WriteBatch,
};

use prost::Message as ProstMessage;

use crate::broker::config::RocksDbConfig;
use crate::error::{StorageError, StorageResult};
use crate::message::Message;
use crate::queue::QueueConfig;
use crate::storage::traits::{Mutation, RaftKeyValueStore, StorageEngine};

const CF_MESSAGES: &str = "messages";
const CF_LEASES: &str = "leases";
const CF_LEASE_EXPIRY: &str = "lease_expiry";
const CF_QUEUES: &str = "queues";
const CF_STATE: &str = "state";
const CF_RAFT_LOG: &str = "raft_log";
/// Message index: maps `{queue_id}:{msg_id}` → full message key for O(1) ack/nack.
const CF_MSG_INDEX: &str = "msg_index";

/// All column family names (excluding `default` which RocksDB creates automatically).
/// Used in tests to verify all CFs exist after open.
#[cfg(test)]
const COLUMN_FAMILIES: &[&str] = &[
    CF_MESSAGES,
    CF_LEASES,
    CF_LEASE_EXPIRY,
    CF_QUEUES,
    CF_STATE,
    CF_RAFT_LOG,
    CF_MSG_INDEX,
];

type DB = DBWithThreadMode<MultiThreaded>;

/// RocksDB-backed storage implementation.
pub struct RocksDbEngine {
    db: DB,
}

/// Compute the exclusive upper bound for a byte prefix.
///
/// Increments the last non-0xFF byte to produce the first key that does
/// *not* match the prefix. Returns an empty `Vec` when the prefix is all
/// 0xFF (meaning the prefix covers the remainder of the keyspace).
fn prefix_upper_bound(prefix: &[u8]) -> Vec<u8> {
    let mut upper = prefix.to_vec();
    while let Some(last) = upper.last_mut() {
        if *last < 0xFF {
            *last += 1;
            return upper;
        }
        upper.pop();
    }
    // All 0xFF — no upper bound possible (covers entire keyspace).
    vec![]
}

impl RocksDbEngine {
    /// Open or create a RocksDB database at the given path with default options.
    ///
    /// Convenience wrapper around [`open_with_config`](Self::open_with_config)
    /// using [`RocksDbConfig::default()`].
    pub fn open(path: impl AsRef<Path>) -> StorageResult<Self> {
        Self::open_with_config(path, &RocksDbConfig::default())
    }

    /// Open or create a RocksDB database with queue-optimized tuning.
    pub fn open_with_config(path: impl AsRef<Path>, config: &RocksDbConfig) -> StorageResult<Self> {
        // Shared LRU block cache across all column families.
        let cache = Cache::new_lru_cache(config.block_cache_mb * 1024 * 1024);

        // --- DB-level options ---
        let mut db_opts = Options::default();
        db_opts.create_if_missing(true);
        db_opts.create_missing_column_families(true);
        db_opts.set_enable_pipelined_write(config.pipelined_write);
        db_opts.set_manual_wal_flush(config.manual_wal_flush);
        db_opts.set_wal_bytes_per_sync(config.wal_bytes_per_sync);

        // --- Per-CF options ---
        let messages_opts =
            Self::messages_cf_opts(config, &cache);
        let leases_opts = Self::leases_cf_opts(config, &cache);
        let lease_expiry_opts = Self::lease_expiry_cf_opts(&cache);
        let raft_log_opts = Self::raft_log_cf_opts(config, &cache);
        let msg_index_opts = Self::msg_index_cf_opts(config, &cache);
        let queues_opts = Self::small_cf_opts(config, &cache);
        let state_opts = Self::small_cf_opts(config, &cache);

        let cf_descriptors = vec![
            ColumnFamilyDescriptor::new(CF_MESSAGES, messages_opts),
            ColumnFamilyDescriptor::new(CF_LEASES, leases_opts),
            ColumnFamilyDescriptor::new(CF_LEASE_EXPIRY, lease_expiry_opts),
            ColumnFamilyDescriptor::new(CF_QUEUES, queues_opts),
            ColumnFamilyDescriptor::new(CF_STATE, state_opts),
            ColumnFamilyDescriptor::new(CF_RAFT_LOG, raft_log_opts),
            ColumnFamilyDescriptor::new(CF_MSG_INDEX, msg_index_opts),
        ];

        let db = DB::open_cf_descriptors(&db_opts, path, cf_descriptors)?;
        Ok(Self { db })
    }

    /// Block-based table options with shared cache and optional bloom filter.
    fn table_opts(cache: &Cache, bloom_bits: i32) -> BlockBasedOptions {
        let mut opts = BlockBasedOptions::default();
        opts.set_block_cache(cache);
        opts.set_cache_index_and_filter_blocks(true);
        opts.set_pin_l0_filter_and_index_blocks_in_cache(true);
        if bloom_bits > 0 {
            opts.set_bloom_filter(bloom_bits as f64, false);
        }
        opts
    }

    /// Per-level compression: no compression for L0-L1, Lz4 for L2+.
    fn compression_per_level() -> [DBCompressionType; 7] {
        [
            DBCompressionType::None, // L0
            DBCompressionType::None, // L1
            DBCompressionType::Lz4,  // L2
            DBCompressionType::Lz4,  // L3
            DBCompressionType::Lz4,  // L4
            DBCompressionType::Lz4,  // L5
            DBCompressionType::Lz4,  // L6
        ]
    }

    /// Messages CF: large write buffers, bloom filter, per-level compression,
    /// CompactOnDeletionCollector (queue messages are deleted after ack).
    fn messages_cf_opts(config: &RocksDbConfig, cache: &Cache) -> Options {
        let table = Self::table_opts(cache, config.bloom_filter_bits);
        let mut opts = Options::default();
        opts.set_block_based_table_factory(&table);
        opts.set_write_buffer_size(config.messages_write_buffer_mb * 1024 * 1024);
        opts.set_max_write_buffer_number(config.messages_max_write_buffers);
        opts.set_min_write_buffer_number_to_merge(config.messages_min_write_buffers_to_merge);
        opts.set_memtable_prefix_bloom_ratio(0.1);
        opts.set_compression_per_level(&Self::compression_per_level());
        if config.compact_on_deletion {
            // window=128, trigger=1, ratio=0.5 — trigger compaction when >50% of
            // keys in a sliding 128-key window are deletion tombstones.
            opts.add_compact_on_deletion_collector_factory(128, 1, 0.5);
        }
        opts
    }

    /// Leases CF: moderate write buffers, bloom filter, per-level compression.
    fn leases_cf_opts(config: &RocksDbConfig, cache: &Cache) -> Options {
        let table = Self::table_opts(cache, config.bloom_filter_bits);
        let mut opts = Options::default();
        opts.set_block_based_table_factory(&table);
        opts.set_write_buffer_size(config.leases_write_buffer_mb * 1024 * 1024);
        opts.set_compression_per_level(&Self::compression_per_level());
        if config.compact_on_deletion {
            opts.add_compact_on_deletion_collector_factory(128, 1, 0.5);
        }
        opts
    }

    /// Lease expiry CF: range-scan only, no bloom filter, no compression.
    fn lease_expiry_cf_opts(cache: &Cache) -> Options {
        let table = Self::table_opts(cache, 0);
        let mut opts = Options::default();
        opts.set_block_based_table_factory(&table);
        opts
    }

    /// Raft log CF: CompactOnDeletionCollector (log entries are trimmed after
    /// snapshot), bloom filters for point lookups.
    fn raft_log_cf_opts(config: &RocksDbConfig, cache: &Cache) -> Options {
        let table = Self::table_opts(cache, config.bloom_filter_bits);
        let mut opts = Options::default();
        opts.set_block_based_table_factory(&table);
        opts.set_compression_per_level(&Self::compression_per_level());
        if config.compact_on_deletion {
            opts.add_compact_on_deletion_collector_factory(128, 1, 0.5);
        }
        opts
    }

    /// Msg index CF: point lookups only, bloom filters.
    fn msg_index_cf_opts(config: &RocksDbConfig, cache: &Cache) -> Options {
        let table = Self::table_opts(cache, config.bloom_filter_bits);
        let mut opts = Options::default();
        opts.set_block_based_table_factory(&table);
        opts.set_compression_per_level(&Self::compression_per_level());
        opts
    }

    /// Small CF options for queues and state (low volume, bloom filters).
    fn small_cf_opts(config: &RocksDbConfig, cache: &Cache) -> Options {
        let table = Self::table_opts(cache, config.bloom_filter_bits);
        let mut opts = Options::default();
        opts.set_block_based_table_factory(&table);
        opts
    }

    fn cf(
        &self,
        name: &'static str,
    ) -> StorageResult<std::sync::Arc<rocksdb::BoundColumnFamily<'_>>> {
        self.db
            .cf_handle(name)
            .ok_or(StorageError::StoreNotFound(name))
    }
}

impl RaftKeyValueStore for RocksDbEngine {
    fn raft_put(&self, key: &[u8], value: &[u8]) -> StorageResult<()> {
        let cf = self.cf(CF_RAFT_LOG)?;
        self.db.put_cf(&cf, key, value)?;
        Ok(())
    }

    fn raft_get(&self, key: &[u8]) -> StorageResult<Option<Vec<u8>>> {
        let cf = self.cf(CF_RAFT_LOG)?;
        Ok(self.db.get_cf(&cf, key)?.map(|v| v.to_vec()))
    }

    fn raft_delete(&self, key: &[u8]) -> StorageResult<()> {
        let cf = self.cf(CF_RAFT_LOG)?;
        self.db.delete_cf(&cf, key)?;
        Ok(())
    }

    fn raft_scan(&self, start: &[u8], limit: usize) -> StorageResult<Vec<(Vec<u8>, Vec<u8>)>> {
        let cf = self.cf(CF_RAFT_LOG)?;
        let iter = self
            .db
            .iterator_cf(&cf, IteratorMode::From(start, rocksdb::Direction::Forward));
        let mut results = Vec::new();
        for item in iter {
            if results.len() >= limit {
                break;
            }
            let (key, value) = item?;
            results.push((key.to_vec(), value.to_vec()));
        }
        Ok(results)
    }

    fn raft_last_with_prefix(&self, prefix: &[u8]) -> StorageResult<Option<(Vec<u8>, Vec<u8>)>> {
        let cf = self.cf(CF_RAFT_LOG)?;

        // Build the successor key of the prefix (prefix with last byte incremented).
        // Seek to that successor in reverse to land on the last key with the prefix.
        let mut upper = prefix.to_vec();
        // Increment the last byte; if it overflows, extend with 0xFF.
        if let Some(last) = upper.last_mut() {
            if *last < 0xFF {
                *last += 1;
            } else {
                upper.push(0xFF);
            }
        } else {
            // Empty prefix — scan from the very end.
            let mut iter = self.db.iterator_cf(&cf, IteratorMode::End);
            return match iter.next() {
                Some(Ok((key, value))) => Ok(Some((key.to_vec(), value.to_vec()))),
                Some(Err(e)) => Err(e.into()),
                None => Ok(None),
            };
        }

        let mut iter = self
            .db
            .iterator_cf(&cf, IteratorMode::From(&upper, rocksdb::Direction::Reverse));
        if let Some(item) = iter.next() {
            let (key, value) = item?;
            if key.starts_with(prefix) {
                return Ok(Some((key.to_vec(), value.to_vec())));
            }
        }
        Ok(None)
    }

    fn raft_delete_range(&self, start: &[u8], end: &[u8]) -> StorageResult<()> {
        let cf = self.cf(CF_RAFT_LOG)?;
        self.db
            .delete_range_cf(&cf, start, end)
            .map_err(|e| StorageError::Engine(e.to_string()))?;
        Ok(())
    }
}

impl StorageEngine for RocksDbEngine {
    fn put_message(&self, key: &[u8], message: &Message) -> StorageResult<()> {
        let cf = self.cf(CF_MESSAGES)?;
        let proto = fila_proto::Message::from(message.clone());
        let value = proto.encode_to_vec();
        self.db.put_cf(&cf, key, &value)?;
        Ok(())
    }

    fn get_message(&self, key: &[u8]) -> StorageResult<Option<Message>> {
        let cf = self.cf(CF_MESSAGES)?;
        match self.db.get_cf(&cf, key)? {
            Some(value) => {
                let proto = fila_proto::Message::decode(&value[..])?;
                let msg = Message::try_from(proto)?;
                Ok(Some(msg))
            }
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
        let mut read_opts = ReadOptions::default();
        let upper = prefix_upper_bound(prefix);
        if !upper.is_empty() {
            read_opts.set_iterate_upper_bound(upper);
        }
        let iter = self.db.iterator_cf_opt(
            &cf,
            read_opts,
            IteratorMode::From(prefix, rocksdb::Direction::Forward),
        );
        let mut results = Vec::new();
        for item in iter {
            let (key, value) = item?;
            let proto = fila_proto::Message::decode(&*value)?;
            let msg = Message::try_from(proto)?;
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
        let proto = fila_proto::ClusterQueueConfig::from(config.clone());
        let value = proto.encode_to_vec();
        self.db.put_cf(&cf, queue_id.as_bytes(), &value)?;
        Ok(())
    }

    fn get_queue(&self, queue_id: &str) -> StorageResult<Option<QueueConfig>> {
        let cf = self.cf(CF_QUEUES)?;
        match self.db.get_cf(&cf, queue_id.as_bytes())? {
            Some(value) => {
                let proto = fila_proto::ClusterQueueConfig::decode(&value[..])?;
                Ok(Some(QueueConfig::from(proto)))
            }
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
            let proto = fila_proto::ClusterQueueConfig::decode(&*value)?;
            results.push(QueueConfig::from(proto));
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

    fn list_state_by_prefix(
        &self,
        prefix: &str,
        limit: usize,
    ) -> StorageResult<Vec<(String, Vec<u8>)>> {
        let cf = self.cf(CF_STATE)?;
        let mut read_opts = ReadOptions::default();
        let upper = prefix_upper_bound(prefix.as_bytes());
        if !upper.is_empty() {
            read_opts.set_iterate_upper_bound(upper);
        }
        let mut result = Vec::new();
        let iter = self.db.iterator_cf_opt(
            &cf,
            read_opts,
            IteratorMode::From(prefix.as_bytes(), rocksdb::Direction::Forward),
        );
        for item in iter {
            if result.len() >= limit {
                break;
            }
            let (key, value) = item?;
            let key_str = std::str::from_utf8(&key)
                .map_err(|e| StorageError::CorruptData(format!("non-UTF8 state key: {e}")))?;
            result.push((key_str.to_string(), value.to_vec()));
        }
        Ok(result)
    }

    fn put_msg_index(&self, key: &[u8], msg_key: &[u8]) -> StorageResult<()> {
        let cf = self.cf(CF_MSG_INDEX)?;
        self.db.put_cf(&cf, key, msg_key)?;
        Ok(())
    }

    fn get_msg_index(&self, key: &[u8]) -> StorageResult<Option<Vec<u8>>> {
        let cf = self.cf(CF_MSG_INDEX)?;
        Ok(self.db.get_cf(&cf, key)?.map(|v| v.to_vec()))
    }

    fn delete_msg_index(&self, key: &[u8]) -> StorageResult<()> {
        let cf = self.cf(CF_MSG_INDEX)?;
        self.db.delete_cf(&cf, key)?;
        Ok(())
    }

    fn apply_mutations(&self, mutations: Vec<Mutation>) -> StorageResult<()> {
        let mut batch = WriteBatch::default();

        for mutation in mutations {
            match mutation {
                Mutation::PutMessage { key, value } => {
                    batch.put_cf(&self.cf(CF_MESSAGES)?, &key, &value);
                }
                Mutation::DeleteMessage { key } => {
                    batch.delete_cf(&self.cf(CF_MESSAGES)?, &key);
                }
                Mutation::PutLease { key, value } => {
                    batch.put_cf(&self.cf(CF_LEASES)?, &key, &value);
                }
                Mutation::DeleteLease { key } => {
                    batch.delete_cf(&self.cf(CF_LEASES)?, &key);
                }
                Mutation::PutLeaseExpiry { key } => {
                    batch.put_cf(&self.cf(CF_LEASE_EXPIRY)?, &key, b"");
                }
                Mutation::DeleteLeaseExpiry { key } => {
                    batch.delete_cf(&self.cf(CF_LEASE_EXPIRY)?, &key);
                }
                Mutation::PutQueue { key, value } => {
                    batch.put_cf(&self.cf(CF_QUEUES)?, &key, &value);
                }
                Mutation::DeleteQueue { key } => {
                    batch.delete_cf(&self.cf(CF_QUEUES)?, &key);
                }
                Mutation::PutState { key, value } => {
                    batch.put_cf(&self.cf(CF_STATE)?, &key, &value);
                }
                Mutation::DeleteState { key } => {
                    batch.delete_cf(&self.cf(CF_STATE)?, &key);
                }
                Mutation::PutMsgIndex { key, value } => {
                    batch.put_cf(&self.cf(CF_MSG_INDEX)?, &key, &value);
                }
                Mutation::DeleteMsgIndex { key } => {
                    batch.delete_cf(&self.cf(CF_MSG_INDEX)?, &key);
                }
            }
        }

        self.db.write(batch)?;
        Ok(())
    }

    fn flush(&self) -> StorageResult<()> {
        self.db
            .flush_wal(true)
            .map_err(|e| StorageError::Engine(e.to_string()))?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::storage::keys;
    use std::collections::HashMap;
    use uuid::Uuid;

    fn test_storage() -> (RocksDbEngine, tempfile::TempDir) {
        let dir = tempfile::tempdir().unwrap();
        let storage = RocksDbEngine::open(dir.path()).unwrap();
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

        let entries = storage
            .list_state_by_prefix("throttle.", usize::MAX)
            .unwrap();
        assert_eq!(entries.len(), 2);
        assert!(entries
            .iter()
            .any(|(k, v)| k == "throttle.a" && v == b"10,100"));
        assert!(entries
            .iter()
            .any(|(k, v)| k == "throttle.b" && v == b"20,200"));

        // Non-matching prefix returns empty
        let entries = storage
            .list_state_by_prefix("nonexistent.", usize::MAX)
            .unwrap();
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
            .apply_mutations(vec![
                Mutation::PutLeaseExpiry { key: ek1.clone() },
                Mutation::PutLeaseExpiry { key: ek2.clone() },
                Mutation::PutLeaseExpiry { key: ek3.clone() },
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
        let msg_value = fila_proto::Message::from(msg.clone()).encode_to_vec();

        // Atomic write: message + lease + lease_expiry
        storage
            .apply_mutations(vec![
                Mutation::PutMessage {
                    key: msg_key.clone(),
                    value: msg_value,
                },
                Mutation::PutLease {
                    key: lease_key.clone(),
                    value: lease_val.clone(),
                },
                Mutation::PutLeaseExpiry {
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
            .apply_mutations(vec![
                Mutation::DeleteMessage {
                    key: msg_key.clone(),
                },
                Mutation::DeleteLease {
                    key: lease_key.clone(),
                },
                Mutation::DeleteLeaseExpiry { key: expiry_key },
            ])
            .unwrap();

        // All three should be gone
        assert!(storage.get_message(&msg_key).unwrap().is_none());
        assert!(storage.get_lease(&lease_key).unwrap().is_none());
        let expired = storage.list_expired_leases(&up_to).unwrap();
        assert!(expired.is_empty());
    }

    #[test]
    fn msg_index_put_get_delete() {
        let (storage, _dir) = test_storage();
        let msg = test_message("q1", "tenant_a");
        let msg_key = keys::message_key(&msg.queue_id, &msg.fairness_key, msg.enqueued_at, &msg.id);
        let idx_key = keys::msg_index_key("q1", &msg.id);

        storage.put_msg_index(&idx_key, &msg_key).unwrap();
        let retrieved = storage.get_msg_index(&idx_key).unwrap().unwrap();
        assert_eq!(retrieved, msg_key);

        storage.delete_msg_index(&idx_key).unwrap();
        assert!(storage.get_msg_index(&idx_key).unwrap().is_none());
    }

    #[test]
    fn msg_index_nonexistent_returns_none() {
        let (storage, _dir) = test_storage();
        let id = Uuid::now_v7();
        let idx_key = keys::msg_index_key("q1", &id);
        assert!(storage.get_msg_index(&idx_key).unwrap().is_none());
    }

    #[test]
    fn msg_index_via_mutations() {
        let (storage, _dir) = test_storage();
        let msg = test_message("q1", "default");
        let msg_key = keys::message_key(&msg.queue_id, &msg.fairness_key, msg.enqueued_at, &msg.id);
        let idx_key = keys::msg_index_key("q1", &msg.id);
        let msg_value = fila_proto::Message::from(msg.clone()).encode_to_vec();

        // Write message and index atomically
        storage
            .apply_mutations(vec![
                Mutation::PutMessage {
                    key: msg_key.clone(),
                    value: msg_value,
                },
                Mutation::PutMsgIndex {
                    key: idx_key.clone(),
                    value: msg_key.clone(),
                },
            ])
            .unwrap();

        assert!(storage.get_message(&msg_key).unwrap().is_some());
        assert_eq!(storage.get_msg_index(&idx_key).unwrap().unwrap(), msg_key);

        // Delete both atomically
        storage
            .apply_mutations(vec![
                Mutation::DeleteMessage {
                    key: msg_key.clone(),
                },
                Mutation::DeleteMsgIndex {
                    key: idx_key.clone(),
                },
            ])
            .unwrap();

        assert!(storage.get_message(&msg_key).unwrap().is_none());
        assert!(storage.get_msg_index(&idx_key).unwrap().is_none());
    }

    #[test]
    fn reopen_preserves_data() {
        let dir = tempfile::tempdir().unwrap();

        // Write data
        {
            let storage = RocksDbEngine::open(dir.path()).unwrap();
            let config = QueueConfig::new("persistent-queue".to_string());
            storage.put_queue("persistent-queue", &config).unwrap();
            storage.put_state("my-key", b"my-value").unwrap();
        }

        // Reopen and verify
        {
            let storage = RocksDbEngine::open(dir.path()).unwrap();
            let config = storage.get_queue("persistent-queue").unwrap().unwrap();
            assert_eq!(config.name, "persistent-queue");
            let val = storage.get_state("my-key").unwrap().unwrap();
            assert_eq!(val, b"my-value");
        }
    }

    #[test]
    fn open_with_config_creates_all_column_families() {
        let dir = tempfile::tempdir().unwrap();
        let config = RocksDbConfig {
            block_cache_mb: 16,
            messages_write_buffer_mb: 4,
            ..Default::default()
        };
        let storage = RocksDbEngine::open_with_config(dir.path(), &config).unwrap();
        for cf_name in COLUMN_FAMILIES {
            assert!(
                storage.db.cf_handle(cf_name).is_some(),
                "column family '{cf_name}' should exist with custom config"
            );
        }
    }

    #[test]
    fn open_with_config_read_write_roundtrip() {
        let dir = tempfile::tempdir().unwrap();
        let config = RocksDbConfig {
            block_cache_mb: 16,
            bloom_filter_bits: 10,
            compact_on_deletion: true,
            ..Default::default()
        };
        let storage = RocksDbEngine::open_with_config(dir.path(), &config).unwrap();
        let msg = test_message("q1", "default");
        let key = keys::message_key(&msg.queue_id, &msg.fairness_key, msg.enqueued_at, &msg.id);
        storage.put_message(&key, &msg).unwrap();
        let retrieved = storage.get_message(&key).unwrap().unwrap();
        assert_eq!(retrieved, msg);
    }

    #[test]
    fn prefix_upper_bound_normal() {
        assert_eq!(prefix_upper_bound(b"abc"), b"abd".to_vec());
    }

    #[test]
    fn prefix_upper_bound_trailing_ff() {
        assert_eq!(prefix_upper_bound(b"ab\xff"), b"ac".to_vec());
    }

    #[test]
    fn prefix_upper_bound_all_ff() {
        assert_eq!(prefix_upper_bound(b"\xff\xff\xff"), Vec::<u8>::new());
    }

    #[test]
    fn prefix_upper_bound_empty() {
        assert_eq!(prefix_upper_bound(b""), Vec::<u8>::new());
    }
}
