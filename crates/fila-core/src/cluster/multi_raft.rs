use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use nonempty::NonEmpty;
use openraft::error::{InitializeError, RaftError};
use openraft::storage::Adaptor;
use openraft::{BasicNode, Config, Raft};
use tokio::sync::RwLock;
use tracing::info;

use super::network::FilaNetworkFactory;
use super::store::FilaRaftStore;
use super::types::{NodeId, TypeConfig};
use crate::storage::RaftKeyValueStore;

/// Errors from `MultiRaftManager::create_group`.
#[derive(Debug, thiserror::Error)]
pub enum CreateGroupError {
    #[error("raft fatal error: {0}")]
    RaftFatal(#[source] Box<openraft::error::Fatal<NodeId>>),

    #[error("node {node_id} not in members list")]
    NotInMembers { node_id: NodeId },
}

/// Manages multiple Raft instances — one per queue — within a single Fila node.
///
/// The meta Raft group handles cluster-wide coordination (membership, queue
/// group creation/deletion). Each queue gets its own independent Raft group
/// for queue-specific operations (enqueue, ack, nack).
pub struct MultiRaftManager {
    node_id: NodeId,
    db: Arc<dyn RaftKeyValueStore>,
    raft_config: Arc<Config>,
    /// Queue ID → Raft instance. Protected by RwLock for concurrent reads
    /// (message routing) with infrequent writes (queue creation/deletion).
    groups: RwLock<HashMap<String, Arc<Raft<TypeConfig>>>>,
    /// Queue IDs that the cluster expects to exist (committed in meta Raft)
    /// but may not yet have a local Raft group ready. Used to distinguish
    /// "queue doesn't exist" (`QueueGroupNotFound`) from "node still catching
    /// up" (`NodeNotReady`).
    expected_queues: RwLock<HashSet<String>>,
    /// Broker storage for queue-level Raft state machines. Committed entries
    /// (enqueue, ack, nack) are applied to this storage on all nodes for replication.
    broker_storage: Arc<dyn crate::storage::StorageEngine>,
}

impl MultiRaftManager {
    pub fn new(
        node_id: NodeId,
        db: Arc<dyn RaftKeyValueStore>,
        raft_config: Arc<Config>,
        broker_storage: Arc<dyn crate::storage::StorageEngine>,
    ) -> Self {
        Self {
            node_id,
            db,
            raft_config,
            groups: RwLock::new(HashMap::new()),
            expected_queues: RwLock::new(HashSet::new()),
            broker_storage,
        }
    }

    /// Create a new Raft group for a queue. All specified member nodes will
    /// participate in this group. The group is initialized (bootstrapped) if
    /// this node is the smallest node ID in the members list.
    pub async fn create_group(
        &self,
        queue_id: &str,
        members: &NonEmpty<(NodeId, String)>,
    ) -> Result<(), CreateGroupError> {
        let store = FilaRaftStore::for_queue(
            Arc::clone(&self.db),
            queue_id,
            Arc::clone(&self.broker_storage),
        );
        let (log_store, state_machine) = Adaptor::new(store);
        let network = FilaNetworkFactory::for_queue(queue_id.to_string());

        let raft = Raft::new(
            self.node_id,
            Arc::clone(&self.raft_config),
            network,
            log_store,
            state_machine,
        )
        .await
        .map_err(|e| CreateGroupError::RaftFatal(Box::new(e)))?;
        let raft = Arc::new(raft);

        // Bootstrap the group if this node is the smallest member
        // (only one node should bootstrap to avoid conflicts).
        let min_member = members.iter().map(|(id, _)| *id).fold(u64::MAX, u64::min);
        if self.node_id == min_member {
            let member_map: std::collections::BTreeMap<_, _> = members
                .iter()
                .map(|(id, addr)| (*id, BasicNode { addr: addr.clone() }))
                .collect();
            match raft.initialize(member_map).await {
                Ok(_) => {}
                Err(RaftError::APIError(InitializeError::NotAllowed(e))) => {
                    // Already initialized — happens on restart when the group
                    // was previously bootstrapped.
                    tracing::debug!(
                        queue_id,
                        error = %e,
                        "queue raft group already initialized"
                    );
                }
                Err(RaftError::Fatal(e)) => return Err(CreateGroupError::RaftFatal(Box::new(e))),
                Err(RaftError::APIError(InitializeError::NotInMembers(_))) => {
                    return Err(CreateGroupError::NotInMembers {
                        node_id: self.node_id,
                    });
                }
            }
        }

        info!(queue_id, node_id = self.node_id, "started queue Raft group");

        self.groups.write().await.insert(queue_id.to_string(), raft);
        Ok(())
    }

    /// Shut down and remove a queue's Raft group, cleaning up its storage.
    pub async fn remove_group(&self, queue_id: &str) {
        if let Some(raft) = self.groups.write().await.remove(queue_id) {
            if let Err(e) = raft.shutdown().await {
                tracing::error!(queue_id, error = ?e, "error shutting down queue Raft group");
            }

            // Clean up the queue's key-prefixed data from RocksDB so that
            // re-creating a queue with the same name starts fresh.
            let prefix = format!("q:{queue_id}:");
            let mut upper = prefix.as_bytes().to_vec();
            // Increment the last byte to form an exclusive upper bound.
            if let Some(last) = upper.last_mut() {
                *last += 1; // ':' (0x3A) → ';' (0x3B)
            }
            if let Err(e) = self.db.raft_delete_range(prefix.as_bytes(), &upper) {
                tracing::error!(queue_id, error = %e, "error cleaning up queue Raft storage");
            }

            info!(queue_id, "removed queue Raft group");
        }
    }

    /// Get the Raft instance for a queue, if it exists.
    pub async fn get_raft(&self, queue_id: &str) -> Option<Arc<Raft<TypeConfig>>> {
        self.groups.read().await.get(queue_id).cloned()
    }

    /// Shut down all queue Raft groups.
    pub async fn shutdown_all(&self) {
        let groups: Vec<(String, Arc<Raft<TypeConfig>>)> =
            self.groups.write().await.drain().collect();
        for (queue_id, raft) in groups {
            if let Err(e) = raft.shutdown().await {
                tracing::error!(queue_id, error = ?e, "error shutting down queue Raft group");
            }
        }
        info!("all queue Raft groups shut down");
    }

    /// List all active queue group IDs.
    pub async fn list_groups(&self) -> Vec<String> {
        self.groups.read().await.keys().cloned().collect()
    }

    /// Get a snapshot of all queue group Raft instances.
    /// Used by `LeaderChangeWatcher` to monitor leadership changes.
    pub async fn snapshot_groups(&self) -> Vec<(String, Arc<Raft<TypeConfig>>)> {
        self.groups
            .read()
            .await
            .iter()
            .map(|(k, v)| (k.clone(), Arc::clone(v)))
            .collect()
    }

    /// Mark a queue as expected by the cluster (committed in meta Raft).
    /// Called before `create_group` so that `is_queue_expected` returns true
    /// during the window between meta commit and local Raft group readiness.
    pub async fn mark_queue_expected(&self, queue_id: &str) {
        self.expected_queues
            .write()
            .await
            .insert(queue_id.to_string());
    }

    /// Remove a queue from the expected set (after its Raft group is deleted).
    pub async fn unmark_queue_expected(&self, queue_id: &str) {
        self.expected_queues.write().await.remove(queue_id);
    }

    /// Check if a queue is expected to exist in the cluster (committed in
    /// meta Raft) even if the local Raft group is not ready yet.
    pub async fn is_queue_expected(&self, queue_id: &str) -> bool {
        self.expected_queues.read().await.contains(queue_id)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn expected_queues_lifecycle() {
        let dir = tempfile::tempdir().unwrap();
        let rocksdb =
            Arc::new(crate::storage::RocksDbEngine::open(dir.path()).unwrap());
        let db: Arc<dyn crate::storage::RaftKeyValueStore> = Arc::clone(&rocksdb) as _;
        let storage: Arc<dyn crate::storage::StorageEngine> = Arc::clone(&rocksdb) as _;

        let config = Config {
            cluster_name: "test".to_string(),
            heartbeat_interval: 100,
            election_timeout_min: 200,
            election_timeout_max: 400,
            ..Default::default()
        };
        let config = Arc::new(config.validate().unwrap());

        let mgr = MultiRaftManager::new(1, db, config, storage);

        // Initially no queues expected.
        assert!(!mgr.is_queue_expected("q1").await);

        // Mark as expected.
        mgr.mark_queue_expected("q1").await;
        assert!(mgr.is_queue_expected("q1").await);
        assert!(!mgr.is_queue_expected("q2").await);

        // Unmark.
        mgr.unmark_queue_expected("q1").await;
        assert!(!mgr.is_queue_expected("q1").await);
    }

    #[tokio::test]
    async fn get_raft_returns_none_for_unknown_queue() {
        let dir = tempfile::tempdir().unwrap();
        let rocksdb =
            Arc::new(crate::storage::RocksDbEngine::open(dir.path()).unwrap());
        let db: Arc<dyn crate::storage::RaftKeyValueStore> = Arc::clone(&rocksdb) as _;
        let storage: Arc<dyn crate::storage::StorageEngine> = Arc::clone(&rocksdb) as _;

        let config = Config {
            cluster_name: "test".to_string(),
            heartbeat_interval: 100,
            election_timeout_min: 200,
            election_timeout_max: 400,
            ..Default::default()
        };
        let config = Arc::new(config.validate().unwrap());

        let mgr = MultiRaftManager::new(1, db, config, storage);

        // Unknown queue returns None.
        assert!(mgr.get_raft("nonexistent").await.is_none());
    }
}
