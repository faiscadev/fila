use std::collections::BTreeMap;
use std::sync::RwLock;

use prost::Message as ProstMessage;

use crate::error::StorageResult;
use crate::message::Message;
use crate::queue::QueueConfig;

use super::traits::{Mutation, StorageEngine};

/// In-memory storage engine for profiling and testing.
///
/// All data is stored in sorted BTreeMaps behind a RwLock.
/// This isolates CPU overhead from disk I/O when benchmarking
/// the scheduler, serialization, and DRR paths.
pub struct InMemoryEngine {
    messages: RwLock<BTreeMap<Vec<u8>, Vec<u8>>>,
    leases: RwLock<BTreeMap<Vec<u8>, Vec<u8>>>,
    lease_expiry: RwLock<BTreeMap<Vec<u8>, ()>>,
    queues: RwLock<BTreeMap<String, Vec<u8>>>,
    state: RwLock<BTreeMap<String, Vec<u8>>>,
    msg_index: RwLock<BTreeMap<Vec<u8>, Vec<u8>>>,
}

impl InMemoryEngine {
    pub fn new() -> Self {
        Self {
            messages: RwLock::new(BTreeMap::new()),
            leases: RwLock::new(BTreeMap::new()),
            lease_expiry: RwLock::new(BTreeMap::new()),
            queues: RwLock::new(BTreeMap::new()),
            state: RwLock::new(BTreeMap::new()),
            msg_index: RwLock::new(BTreeMap::new()),
        }
    }
}

impl Default for InMemoryEngine {
    fn default() -> Self {
        Self::new()
    }
}

impl StorageEngine for InMemoryEngine {
    fn put_message(&self, key: &[u8], message: &Message) -> StorageResult<()> {
        let proto = fila_proto::Message::from(message.clone());
        let value = proto.encode_to_vec();
        self.messages.write().unwrap().insert(key.to_vec(), value);
        Ok(())
    }

    fn get_message(&self, key: &[u8]) -> StorageResult<Option<Message>> {
        let guard = self.messages.read().unwrap();
        match guard.get(key) {
            Some(bytes) => {
                let proto = fila_proto::Message::decode(&bytes[..])?;
                let msg = Message::try_from(proto)?;
                Ok(Some(msg))
            }
            None => Ok(None),
        }
    }

    fn delete_message(&self, key: &[u8]) -> StorageResult<()> {
        self.messages.write().unwrap().remove(key);
        Ok(())
    }

    fn list_messages(&self, prefix: &[u8]) -> StorageResult<Vec<(Vec<u8>, Message)>> {
        let guard = self.messages.read().unwrap();
        let mut results = Vec::new();
        for (k, v) in guard.range(prefix.to_vec()..) {
            if !k.starts_with(prefix) {
                break;
            }
            let proto = fila_proto::Message::decode(&v[..])?;
            let msg = Message::try_from(proto)?;
            results.push((k.clone(), msg));
        }
        Ok(results)
    }

    fn put_lease(&self, key: &[u8], value: &[u8]) -> StorageResult<()> {
        self.leases
            .write()
            .unwrap()
            .insert(key.to_vec(), value.to_vec());
        Ok(())
    }

    fn get_lease(&self, key: &[u8]) -> StorageResult<Option<Vec<u8>>> {
        Ok(self.leases.read().unwrap().get(key).cloned())
    }

    fn delete_lease(&self, key: &[u8]) -> StorageResult<()> {
        self.leases.write().unwrap().remove(key);
        Ok(())
    }

    fn list_expired_leases(&self, up_to_key: &[u8]) -> StorageResult<Vec<Vec<u8>>> {
        let guard = self.lease_expiry.read().unwrap();
        let mut results = Vec::new();
        for (k, _) in guard.range(..=up_to_key.to_vec()) {
            results.push(k.clone());
        }
        Ok(results)
    }

    fn put_queue(&self, queue_id: &str, config: &QueueConfig) -> StorageResult<()> {
        let proto = fila_proto::ClusterQueueConfig::from(config.clone());
        let value = proto.encode_to_vec();
        self.queues
            .write()
            .unwrap()
            .insert(queue_id.to_string(), value);
        Ok(())
    }

    fn get_queue(&self, queue_id: &str) -> StorageResult<Option<QueueConfig>> {
        let guard = self.queues.read().unwrap();
        match guard.get(queue_id) {
            Some(bytes) => {
                let proto = fila_proto::ClusterQueueConfig::decode(&bytes[..])?;
                Ok(Some(QueueConfig::from(proto)))
            }
            None => Ok(None),
        }
    }

    fn delete_queue(&self, queue_id: &str) -> StorageResult<()> {
        self.queues.write().unwrap().remove(queue_id);
        Ok(())
    }

    fn list_queues(&self) -> StorageResult<Vec<QueueConfig>> {
        let guard = self.queues.read().unwrap();
        let mut results = Vec::new();
        for bytes in guard.values() {
            let proto = fila_proto::ClusterQueueConfig::decode(&bytes[..])?;
            results.push(QueueConfig::from(proto));
        }
        Ok(results)
    }

    fn put_state(&self, key: &str, value: &[u8]) -> StorageResult<()> {
        self.state
            .write()
            .unwrap()
            .insert(key.to_string(), value.to_vec());
        Ok(())
    }

    fn get_state(&self, key: &str) -> StorageResult<Option<Vec<u8>>> {
        Ok(self.state.read().unwrap().get(key).cloned())
    }

    fn delete_state(&self, key: &str) -> StorageResult<()> {
        self.state.write().unwrap().remove(key);
        Ok(())
    }

    fn list_state_by_prefix(
        &self,
        prefix: &str,
        limit: usize,
    ) -> StorageResult<Vec<(String, Vec<u8>)>> {
        let guard = self.state.read().unwrap();
        let mut results = Vec::new();
        for (k, v) in guard.range(prefix.to_string()..) {
            if !k.starts_with(prefix) {
                break;
            }
            results.push((k.clone(), v.clone()));
            if results.len() >= limit {
                break;
            }
        }
        Ok(results)
    }

    fn put_msg_index(&self, key: &[u8], msg_key: &[u8]) -> StorageResult<()> {
        self.msg_index
            .write()
            .unwrap()
            .insert(key.to_vec(), msg_key.to_vec());
        Ok(())
    }

    fn get_msg_index(&self, key: &[u8]) -> StorageResult<Option<Vec<u8>>> {
        Ok(self.msg_index.read().unwrap().get(key).cloned())
    }

    fn delete_msg_index(&self, key: &[u8]) -> StorageResult<()> {
        self.msg_index.write().unwrap().remove(key);
        Ok(())
    }

    fn apply_mutations(&self, mutations: Vec<Mutation>) -> StorageResult<()> {
        for mutation in mutations {
            match mutation {
                Mutation::PutMessage { key, value } => {
                    self.messages.write().unwrap().insert(key, value);
                }
                Mutation::DeleteMessage { key } => {
                    self.messages.write().unwrap().remove(&key);
                }
                Mutation::PutLease { key, value } => {
                    self.leases.write().unwrap().insert(key, value);
                }
                Mutation::DeleteLease { key } => {
                    self.leases.write().unwrap().remove(&key);
                }
                Mutation::PutLeaseExpiry { key } => {
                    self.lease_expiry.write().unwrap().insert(key, ());
                }
                Mutation::DeleteLeaseExpiry { key } => {
                    self.lease_expiry.write().unwrap().remove(&key);
                }
                Mutation::PutQueue { key, value } => {
                    self.queues
                        .write()
                        .unwrap()
                        .insert(String::from_utf8_lossy(&key).into_owned(), value);
                }
                Mutation::DeleteQueue { key } => {
                    self.queues
                        .write()
                        .unwrap()
                        .remove(&String::from_utf8_lossy(&key).into_owned());
                }
                Mutation::PutState { key, value } => {
                    self.state
                        .write()
                        .unwrap()
                        .insert(String::from_utf8_lossy(&key).into_owned(), value);
                }
                Mutation::DeleteState { key } => {
                    self.state
                        .write()
                        .unwrap()
                        .remove(&String::from_utf8_lossy(&key).into_owned());
                }
                Mutation::PutMsgIndex { key, value } => {
                    self.msg_index.write().unwrap().insert(key, value);
                }
                Mutation::DeleteMsgIndex { key } => {
                    self.msg_index.write().unwrap().remove(&key);
                }
            }
        }
        Ok(())
    }

    fn flush(&self) -> StorageResult<()> {
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use bytes::Bytes;
    use std::collections::HashMap;
    use uuid::Uuid;

    fn test_message() -> Message {
        Message {
            id: Uuid::new_v4(),
            queue_id: "test-queue".to_string(),
            headers: HashMap::new(),
            payload: Bytes::from(vec![1, 2, 3]),
            fairness_key: "default".to_string(),
            weight: 1,
            throttle_keys: vec![],
            attempt_count: 0,
            enqueued_at: 1000,
            leased_at: None,
        }
    }

    #[test]
    fn message_put_get_delete() {
        let engine = InMemoryEngine::new();
        let msg = test_message();
        let key = b"test-key";

        engine.put_message(key, &msg).unwrap();
        let retrieved = engine.get_message(key).unwrap().unwrap();
        assert_eq!(retrieved.id, msg.id);
        assert_eq!(retrieved.queue_id, msg.queue_id);

        engine.delete_message(key).unwrap();
        assert!(engine.get_message(key).unwrap().is_none());
    }

    #[test]
    fn apply_mutations_works() {
        let engine = InMemoryEngine::new();
        let msg = test_message();
        let proto = fila_proto::Message::from(msg.clone());
        let msg_value = proto.encode_to_vec();

        let mutations = vec![
            Mutation::PutMessage {
                key: b"msg-1".to_vec(),
                value: msg_value,
            },
            Mutation::PutLease {
                key: b"lease-1".to_vec(),
                value: b"lease-val".to_vec(),
            },
            Mutation::PutMsgIndex {
                key: b"idx-1".to_vec(),
                value: b"msg-1".to_vec(),
            },
        ];

        engine.apply_mutations(mutations).unwrap();

        assert!(engine.get_message(b"msg-1").unwrap().is_some());
        assert!(engine.get_lease(b"lease-1").unwrap().is_some());
        assert!(engine.get_msg_index(b"idx-1").unwrap().is_some());
    }

    #[test]
    fn queue_config_roundtrip() {
        let engine = InMemoryEngine::new();
        let config = QueueConfig {
            name: "test".to_string(),
            on_enqueue_script: None,
            on_failure_script: None,
            visibility_timeout_ms: 30_000,
            dlq_queue_id: None,
            lua_timeout_ms: None,
            lua_memory_limit_bytes: None,
        };

        engine.put_queue("test", &config).unwrap();
        let retrieved = engine.get_queue("test").unwrap().unwrap();
        assert_eq!(retrieved.name, "test");
    }
}
