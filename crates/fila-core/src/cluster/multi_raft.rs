use std::collections::HashMap;
use std::sync::Arc;

use openraft::storage::Adaptor;
use openraft::{BasicNode, Config, Raft};
use tokio::sync::RwLock;
use tracing::info;

use super::network::FilaNetworkFactory;
use super::store::FilaRaftStore;
use super::types::{NodeId, TypeConfig};
use crate::storage::RocksDbEngine;

/// Manages multiple Raft instances — one per queue — within a single Fila node.
///
/// The meta Raft group handles cluster-wide coordination (membership, queue
/// group creation/deletion). Each queue gets its own independent Raft group
/// for queue-specific operations (enqueue, ack, nack).
pub struct MultiRaftManager {
    node_id: NodeId,
    db: Arc<RocksDbEngine>,
    raft_config: Arc<Config>,
    /// Queue ID → Raft instance. Protected by RwLock for concurrent reads
    /// (message routing) with infrequent writes (queue creation/deletion).
    groups: RwLock<HashMap<String, Arc<Raft<TypeConfig>>>>,
}

impl MultiRaftManager {
    pub fn new(node_id: NodeId, db: Arc<RocksDbEngine>, raft_config: Arc<Config>) -> Self {
        Self {
            node_id,
            db,
            raft_config,
            groups: RwLock::new(HashMap::new()),
        }
    }

    /// Create a new Raft group for a queue. All specified member nodes will
    /// participate in this group. The group is initialized (bootstrapped) if
    /// this node is the smallest node ID in the members list.
    pub async fn create_group(
        &self,
        queue_id: &str,
        members: &[NodeId],
        member_addrs: &HashMap<NodeId, String>,
    ) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        let store = FilaRaftStore::for_queue(Arc::clone(&self.db), queue_id);
        let (log_store, state_machine) = Adaptor::new(store);
        let network = FilaNetworkFactory::for_queue(queue_id.to_string());

        let raft = Raft::new(
            self.node_id,
            Arc::clone(&self.raft_config),
            network,
            log_store,
            state_machine,
        )
        .await?;
        let raft = Arc::new(raft);

        // Bootstrap the group if this node is the smallest member
        // (only one node should bootstrap to avoid conflicts).
        let min_member = members.iter().copied().min().unwrap_or(0);
        if self.node_id == min_member {
            let mut member_map = std::collections::BTreeMap::new();
            for &node_id in members {
                if let Some(addr) = member_addrs.get(&node_id) {
                    member_map.insert(node_id, BasicNode { addr: addr.clone() });
                }
            }
            if let Err(e) = raft.initialize(member_map).await {
                // Ignore "already initialized" — can happen if we restart and
                // the group was previously bootstrapped.
                tracing::debug!(
                    queue_id,
                    error = %e,
                    "initialize queue raft group (may already be initialized)"
                );
            }
        }

        info!(queue_id, node_id = self.node_id, "started queue Raft group");

        self.groups.write().await.insert(queue_id.to_string(), raft);
        Ok(())
    }

    /// Shut down and remove a queue's Raft group.
    pub async fn remove_group(&self, queue_id: &str) {
        if let Some(raft) = self.groups.write().await.remove(queue_id) {
            if let Err(e) = raft.shutdown().await {
                tracing::error!(queue_id, error = ?e, "error shutting down queue Raft group");
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
}
