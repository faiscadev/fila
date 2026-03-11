use std::fmt::Debug;
use std::io::Cursor;
use std::ops::RangeBounds;
use std::sync::Arc;

use openraft::storage::{LogState, RaftLogReader, RaftSnapshotBuilder, RaftStorage, Snapshot};
use openraft::{
    BasicNode, Entry, EntryPayload, LogId, SnapshotMeta, StorageError, StoredMembership, Vote,
};
use serde::{Deserialize, Serialize};

use std::collections::HashMap;

use nonempty::NonEmpty;

use super::types::{ClusterResponse, NodeId, TypeConfig};
use crate::queue::QueueConfig;
use crate::storage::{Mutation, RaftKeyValueStore, StorageEngine};

/// Events emitted by the meta Raft state machine when queue group
/// lifecycle changes are committed. A background task processes these
/// events to create/remove queue Raft groups and local scheduler queues.
#[derive(Debug)]
pub enum MetaStoreEvent {
    QueueGroupCreated {
        queue_id: String,
        members: NonEmpty<(NodeId, String)>,
        config: Box<QueueConfig>,
    },
    QueueGroupDeleted {
        queue_id: String,
    },
}

/// Key segment names for Raft data in the raft_log column family.
const VOTE_SUFFIX: &[u8] = b"vote";
const LOG_SUFFIX: &[u8] = b"log:";
const LAST_PURGED_SUFFIX: &[u8] = b"meta:last_purged";

/// Builds storage keys with an optional group prefix for multi-Raft isolation.
///
/// Meta Raft group uses empty prefix (keys: `vote`, `log:...`, `meta:last_purged`).
/// Queue Raft groups use `q:{queue_id}:` prefix for key-space isolation within
/// the same RocksDB column family.
#[derive(Debug, Clone)]
struct KeyBuilder {
    prefix: Vec<u8>,
}

impl KeyBuilder {
    /// Create a key builder for the meta Raft group (no prefix).
    fn meta() -> Self {
        Self { prefix: Vec::new() }
    }

    /// Create a key builder for a queue Raft group.
    fn for_queue(queue_id: &str) -> Self {
        Self {
            prefix: format!("q:{queue_id}:").into_bytes(),
        }
    }

    fn vote_key(&self) -> Vec<u8> {
        let mut key = self.prefix.clone();
        key.extend_from_slice(VOTE_SUFFIX);
        key
    }

    fn log_key(&self, index: u64) -> Vec<u8> {
        let mut key = self.prefix.clone();
        key.extend_from_slice(LOG_SUFFIX);
        key.extend_from_slice(&index.to_be_bytes());
        key
    }

    fn last_purged_key(&self) -> Vec<u8> {
        let mut key = self.prefix.clone();
        key.extend_from_slice(LAST_PURGED_SUFFIX);
        key
    }

    fn log_prefix(&self) -> Vec<u8> {
        let mut key = self.prefix.clone();
        key.extend_from_slice(LOG_SUFFIX);
        key
    }
}

/// In-memory state machine state, persisted via snapshots.
#[derive(Debug, Default, Clone, Serialize, Deserialize)]
pub struct StateMachineData {
    pub last_applied_log: Option<LogId<NodeId>>,
    pub last_membership: StoredMembership<NodeId, BasicNode>,
    /// Active queue Raft groups: queue_id → member node IDs.
    /// Tracked in the meta Raft state machine so all nodes learn about
    /// queue groups through Raft replication.
    #[serde(default)]
    pub queue_groups: HashMap<String, Vec<u64>>,
}

/// Combined Raft storage implementation backed by RocksDB (for logs/vote)
/// and in-memory state (for the state machine).
///
/// This implements the v1 `RaftStorage` trait. The `Adaptor` splits it into
/// separate `RaftLogStorage` + `RaftStateMachine` for the Raft runtime.
pub struct FilaRaftStore {
    db: Arc<dyn RaftKeyValueStore>,
    keys: KeyBuilder,
    /// In-memory state machine state.
    state_machine: StateMachineData,
    /// Current snapshot (if any).
    current_snapshot: Option<StoredSnapshot>,
    /// For the meta store: channel to emit queue group lifecycle events.
    meta_event_tx: Option<tokio::sync::mpsc::UnboundedSender<MetaStoreEvent>>,
    /// For queue-level stores: reference to the broker's storage engine so
    /// committed entries (enqueue, ack, nack) are applied to the local storage
    /// on ALL nodes — not just the leader. This is the core replication
    /// mechanism: followers apply committed entries to their local RocksDB,
    /// so when a follower becomes leader, it already has all the data.
    broker_storage: Option<Arc<dyn StorageEngine>>,
    /// Queue ID (only set for queue-level stores, used for storage key construction).
    queue_id: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct StoredSnapshot {
    meta: SnapshotMeta<NodeId, BasicNode>,
    data: Vec<u8>,
}

impl FilaRaftStore {
    /// Create a store for the meta Raft group (no key prefix).
    pub fn new(
        db: Arc<dyn RaftKeyValueStore>,
        meta_event_tx: Option<tokio::sync::mpsc::UnboundedSender<MetaStoreEvent>>,
    ) -> Self {
        Self {
            db,
            keys: KeyBuilder::meta(),
            state_machine: StateMachineData::default(),
            current_snapshot: None,
            meta_event_tx,
            broker_storage: None,
            queue_id: None,
        }
    }

    /// Create a store for a queue-level Raft group (prefixed key space).
    ///
    /// If `broker_storage` is provided, committed entries (enqueue, ack, nack)
    /// will be applied to the broker's storage on all nodes — enabling
    /// replication for failover.
    pub fn for_queue(
        db: Arc<dyn RaftKeyValueStore>,
        queue_id: &str,
        broker_storage: Option<Arc<dyn StorageEngine>>,
    ) -> Self {
        Self {
            db,
            keys: KeyBuilder::for_queue(queue_id),
            state_machine: StateMachineData::default(),
            current_snapshot: None,
            meta_event_tx: None,
            broker_storage,
            queue_id: Some(queue_id.to_string()),
        }
    }
}

impl RaftLogReader<TypeConfig> for FilaRaftStore {
    async fn try_get_log_entries<RB: RangeBounds<u64> + Clone + Debug + Send>(
        &mut self,
        range: RB,
    ) -> Result<Vec<Entry<TypeConfig>>, StorageError<NodeId>> {
        let start = match range.start_bound() {
            std::ops::Bound::Included(&v) => v,
            std::ops::Bound::Excluded(&v) => v + 1,
            std::ops::Bound::Unbounded => 0,
        };
        let end = match range.end_bound() {
            std::ops::Bound::Included(&v) => v + 1,
            std::ops::Bound::Excluded(&v) => v,
            std::ops::Bound::Unbounded => u64::MAX,
        };

        let start_key = self.keys.log_key(start);
        let log_prefix = self.keys.log_prefix();
        let entries_raw = self
            .db
            .raft_scan(&start_key, (end - start) as usize)
            .map_err(|e| StorageError::IO {
                source: openraft::StorageIOError::read_logs(&e),
            })?;

        let mut entries = Vec::new();
        for (key, value) in entries_raw {
            if !key.starts_with(&log_prefix) {
                break;
            }
            let index_bytes: [u8; 8] =
                key[log_prefix.len()..]
                    .try_into()
                    .map_err(|_| StorageError::IO {
                        source: openraft::StorageIOError::read_logs(&std::io::Error::new(
                            std::io::ErrorKind::InvalidData,
                            "invalid log key length",
                        )),
                    })?;
            let index = u64::from_be_bytes(index_bytes);
            if index >= end {
                break;
            }
            let entry: Entry<TypeConfig> =
                serde_json::from_slice(&value).map_err(|e| StorageError::IO {
                    source: openraft::StorageIOError::read_logs(&e),
                })?;
            entries.push(entry);
        }

        Ok(entries)
    }
}

impl RaftSnapshotBuilder<TypeConfig> for FilaRaftStore {
    async fn build_snapshot(&mut self) -> Result<Snapshot<TypeConfig>, StorageError<NodeId>> {
        let data = serde_json::to_vec(&self.state_machine).map_err(|e| StorageError::IO {
            source: openraft::StorageIOError::read_state_machine(&e),
        })?;

        let last_applied_log = self.state_machine.last_applied_log;
        let last_membership = self.state_machine.last_membership.clone();

        let snapshot_id = if let Some(last) = last_applied_log {
            format!(
                "{}-{}-{}",
                last.leader_id.term, last.leader_id.node_id, last.index
            )
        } else {
            "empty".to_string()
        };

        let meta = SnapshotMeta {
            last_log_id: last_applied_log,
            last_membership,
            snapshot_id,
        };

        let snapshot = StoredSnapshot {
            meta: meta.clone(),
            data: data.clone(),
        };
        self.current_snapshot = Some(snapshot);

        Ok(Snapshot {
            meta,
            snapshot: Box::new(Cursor::new(data)),
        })
    }
}

impl RaftStorage<TypeConfig> for FilaRaftStore {
    type LogReader = Self;
    type SnapshotBuilder = Self;

    async fn save_vote(&mut self, vote: &Vote<NodeId>) -> Result<(), StorageError<NodeId>> {
        let data = serde_json::to_vec(vote).map_err(|e| StorageError::IO {
            source: openraft::StorageIOError::write_vote(&e),
        })?;
        let vote_key = self.keys.vote_key();
        self.db
            .raft_put(&vote_key, &data)
            .map_err(|e| StorageError::IO {
                source: openraft::StorageIOError::write_vote(&e),
            })?;
        Ok(())
    }

    async fn read_vote(&mut self) -> Result<Option<Vote<NodeId>>, StorageError<NodeId>> {
        let vote_key = self.keys.vote_key();
        match self.db.raft_get(&vote_key).map_err(|e| StorageError::IO {
            source: openraft::StorageIOError::read_vote(&e),
        })? {
            Some(data) => {
                let vote: Vote<NodeId> =
                    serde_json::from_slice(&data).map_err(|e| StorageError::IO {
                        source: openraft::StorageIOError::read_vote(&e),
                    })?;
                Ok(Some(vote))
            }
            None => Ok(None),
        }
    }

    async fn get_log_state(&mut self) -> Result<LogState<TypeConfig>, StorageError<NodeId>> {
        // Get last purged log id
        let purged_key = self.keys.last_purged_key();
        let last_purged_log_id =
            match self
                .db
                .raft_get(&purged_key)
                .map_err(|e| StorageError::IO {
                    source: openraft::StorageIOError::read_logs(&e),
                })? {
                Some(data) => serde_json::from_slice(&data).map_err(|e| StorageError::IO {
                    source: openraft::StorageIOError::read_logs(&e),
                })?,
                None => None,
            };

        // Scan backwards to find last log entry
        let last_log_id = self.find_last_log_id()?;

        Ok(LogState {
            last_purged_log_id,
            last_log_id: last_log_id.or(last_purged_log_id),
        })
    }

    async fn get_log_reader(&mut self) -> Self::LogReader {
        FilaRaftStore {
            db: Arc::clone(&self.db),
            keys: self.keys.clone(),
            state_machine: self.state_machine.clone(),
            current_snapshot: self.current_snapshot.clone(),
            meta_event_tx: self.meta_event_tx.clone(),
            broker_storage: self.broker_storage.clone(),
            queue_id: self.queue_id.clone(),
        }
    }

    async fn append_to_log<I>(&mut self, entries: I) -> Result<(), StorageError<NodeId>>
    where
        I: IntoIterator<Item = Entry<TypeConfig>> + Send,
    {
        for entry in entries {
            let key = self.keys.log_key(entry.log_id.index);
            let value = serde_json::to_vec(&entry).map_err(|e| StorageError::IO {
                source: openraft::StorageIOError::write_logs(&e),
            })?;
            self.db
                .raft_put(&key, &value)
                .map_err(|e| StorageError::IO {
                    source: openraft::StorageIOError::write_logs(&e),
                })?;
        }
        Ok(())
    }

    async fn delete_conflict_logs_since(
        &mut self,
        log_id: LogId<NodeId>,
    ) -> Result<(), StorageError<NodeId>> {
        let start = self.keys.log_key(log_id.index);
        let end = self.keys.log_key(u64::MAX);
        self.db
            .raft_delete_range(&start, &end)
            .map_err(|e| StorageError::IO {
                source: openraft::StorageIOError::write_logs(&e),
            })?;
        Ok(())
    }

    async fn purge_logs_upto(&mut self, log_id: LogId<NodeId>) -> Result<(), StorageError<NodeId>> {
        // Save the purge point
        let data = serde_json::to_vec(&Some(log_id)).map_err(|e| StorageError::IO {
            source: openraft::StorageIOError::write_logs(&e),
        })?;
        let purged_key = self.keys.last_purged_key();
        self.db
            .raft_put(&purged_key, &data)
            .map_err(|e| StorageError::IO {
                source: openraft::StorageIOError::write_logs(&e),
            })?;

        // Delete log entries up to and including log_id.index
        let start = self.keys.log_key(0);
        let end = self.keys.log_key(log_id.index + 1);
        self.db
            .raft_delete_range(&start, &end)
            .map_err(|e| StorageError::IO {
                source: openraft::StorageIOError::write_logs(&e),
            })?;

        Ok(())
    }

    async fn last_applied_state(
        &mut self,
    ) -> Result<(Option<LogId<NodeId>>, StoredMembership<NodeId, BasicNode>), StorageError<NodeId>>
    {
        Ok((
            self.state_machine.last_applied_log,
            self.state_machine.last_membership.clone(),
        ))
    }

    async fn apply_to_state_machine(
        &mut self,
        entries: &[Entry<TypeConfig>],
    ) -> Result<Vec<ClusterResponse>, StorageError<NodeId>> {
        let mut responses = Vec::with_capacity(entries.len());

        for entry in entries {
            self.state_machine.last_applied_log = Some(entry.log_id);

            match &entry.payload {
                EntryPayload::Blank => {
                    responses.push(ClusterResponse::Ack);
                }
                EntryPayload::Normal(request) => {
                    // Queue-level stores: apply committed operations to the
                    // broker's storage so all nodes have the data. This is the
                    // replication mechanism — followers apply entries too.
                    if let Some(ref storage) = self.broker_storage {
                        self.apply_to_broker_storage(storage.as_ref(), request);
                    }

                    let response = match request {
                        super::types::ClusterRequest::Enqueue { message } => {
                            ClusterResponse::Enqueue { msg_id: message.id }
                        }
                        super::types::ClusterRequest::Ack { .. } => ClusterResponse::Ack,
                        super::types::ClusterRequest::Nack { .. } => ClusterResponse::Nack,
                        super::types::ClusterRequest::CreateQueue { name, .. } => {
                            ClusterResponse::CreateQueue {
                                queue_id: name.clone(),
                            }
                        }
                        super::types::ClusterRequest::DeleteQueue { .. } => {
                            ClusterResponse::DeleteQueue
                        }
                        super::types::ClusterRequest::SetConfig { .. } => {
                            ClusterResponse::SetConfig
                        }
                        super::types::ClusterRequest::SetThrottleRate { .. } => {
                            ClusterResponse::SetThrottleRate
                        }
                        super::types::ClusterRequest::RemoveThrottleRate { .. } => {
                            ClusterResponse::RemoveThrottleRate
                        }
                        super::types::ClusterRequest::Redrive { .. } => {
                            ClusterResponse::Redrive { count: 0 }
                        }
                        super::types::ClusterRequest::CreateQueueGroup {
                            queue_id,
                            members,
                            config,
                        } => {
                            self.state_machine
                                .queue_groups
                                .insert(queue_id.clone(), members.clone());

                            // Extract member (id, addr) pairs from Raft membership.
                            let member_pairs: Vec<(NodeId, String)> = self
                                .state_machine
                                .last_membership
                                .membership()
                                .nodes()
                                .filter(|(id, _)| members.contains(id))
                                .map(|(&id, n)| (id, n.addr.clone()))
                                .collect();

                            if let Some(ref tx) = self.meta_event_tx {
                                if let Some(member_pairs) = NonEmpty::from_vec(member_pairs) {
                                    let _ = tx.send(MetaStoreEvent::QueueGroupCreated {
                                        queue_id: queue_id.clone(),
                                        members: member_pairs,
                                        config: Box::new(config.clone()),
                                    });
                                } else {
                                    tracing::error!(
                                        queue_id,
                                        "no member addresses found in raft membership"
                                    );
                                }
                            }

                            ClusterResponse::CreateQueueGroup {
                                queue_id: queue_id.clone(),
                            }
                        }
                        super::types::ClusterRequest::DeleteQueueGroup { queue_id } => {
                            self.state_machine.queue_groups.remove(queue_id);

                            if let Some(ref tx) = self.meta_event_tx {
                                let _ = tx.send(MetaStoreEvent::QueueGroupDeleted {
                                    queue_id: queue_id.clone(),
                                });
                            }

                            ClusterResponse::DeleteQueueGroup
                        }
                    };
                    responses.push(response);
                }
                EntryPayload::Membership(membership) => {
                    self.state_machine.last_membership =
                        StoredMembership::new(Some(entry.log_id), membership.clone());
                    responses.push(ClusterResponse::Ack);
                }
            }
        }

        Ok(responses)
    }

    async fn get_snapshot_builder(&mut self) -> Self::SnapshotBuilder {
        FilaRaftStore {
            db: Arc::clone(&self.db),
            keys: self.keys.clone(),
            state_machine: self.state_machine.clone(),
            current_snapshot: self.current_snapshot.clone(),
            meta_event_tx: self.meta_event_tx.clone(),
            broker_storage: self.broker_storage.clone(),
            queue_id: self.queue_id.clone(),
        }
    }

    async fn begin_receiving_snapshot(
        &mut self,
    ) -> Result<Box<Cursor<Vec<u8>>>, StorageError<NodeId>> {
        Ok(Box::new(Cursor::new(Vec::new())))
    }

    async fn install_snapshot(
        &mut self,
        meta: &SnapshotMeta<NodeId, BasicNode>,
        snapshot: Box<Cursor<Vec<u8>>>,
    ) -> Result<(), StorageError<NodeId>> {
        let data = snapshot.into_inner();

        let new_state: StateMachineData =
            serde_json::from_slice(&data).map_err(|e| StorageError::IO {
                source: openraft::StorageIOError::read_state_machine(&e),
            })?;

        self.state_machine = new_state;
        self.current_snapshot = Some(StoredSnapshot {
            meta: meta.clone(),
            data,
        });

        // After restoring from snapshot, emit events for all queue groups
        // so the background handler creates local queues and Raft groups.
        // Without this, nodes that start from a snapshot would miss setup.
        if let Some(ref tx) = self.meta_event_tx {
            let all_nodes: Vec<(NodeId, String)> = self
                .state_machine
                .last_membership
                .membership()
                .nodes()
                .map(|(&id, n)| (id, n.addr.clone()))
                .collect();

            for (queue_id, members) in &self.state_machine.queue_groups {
                let member_pairs: Vec<(NodeId, String)> = all_nodes
                    .iter()
                    .filter(|(id, _)| members.contains(id))
                    .cloned()
                    .collect();
                if let Some(member_pairs) = NonEmpty::from_vec(member_pairs) {
                    let _ = tx.send(MetaStoreEvent::QueueGroupCreated {
                        queue_id: queue_id.clone(),
                        members: member_pairs,
                        config: Box::new(QueueConfig::new(queue_id.clone())),
                    });
                } else {
                    tracing::error!(queue_id, "no member addresses found in snapshot membership");
                }
            }
        }

        Ok(())
    }

    async fn get_current_snapshot(
        &mut self,
    ) -> Result<Option<Snapshot<TypeConfig>>, StorageError<NodeId>> {
        Ok(self.current_snapshot.as_ref().map(|s| Snapshot {
            meta: s.meta.clone(),
            snapshot: Box::new(Cursor::new(s.data.clone())),
        }))
    }
}

impl FilaRaftStore {
    /// Apply a committed queue-level operation to the broker's storage.
    ///
    /// This runs on ALL nodes (leader and followers) when Raft commits an entry,
    /// ensuring every replica has the data in its local RocksDB. When a follower
    /// becomes leader, it can rebuild its in-memory scheduler state from storage.
    fn apply_to_broker_storage(
        &self,
        storage: &dyn StorageEngine,
        request: &super::types::ClusterRequest,
    ) {
        match request {
            super::types::ClusterRequest::Enqueue { message } => {
                let queue_id = self.queue_id.as_deref().unwrap_or(&message.queue_id);
                let msg_key = crate::storage::keys::message_key(
                    queue_id,
                    &message.fairness_key,
                    message.enqueued_at,
                    &message.id,
                );
                let msg_value = match serde_json::to_vec(message) {
                    Ok(v) => v,
                    Err(e) => {
                        tracing::error!(error = %e, "failed to serialize message for storage replication");
                        return;
                    }
                };
                if let Err(e) = storage.apply_mutations(vec![Mutation::PutMessage {
                    key: msg_key,
                    value: msg_value,
                }]) {
                    tracing::error!(error = %e, "failed to replicate enqueue to broker storage");
                }
            }
            super::types::ClusterRequest::Ack { queue_id, msg_id } => {
                // Find the message and its lease, then delete them.
                let msg_prefix = crate::storage::keys::message_prefix(queue_id);
                match storage.list_messages(&msg_prefix) {
                    Ok(messages) => {
                        for (key, msg) in messages {
                            if msg.id == *msg_id {
                                let lease_key = crate::storage::keys::lease_key(queue_id, msg_id);
                                let mut mutations = vec![Mutation::DeleteMessage { key }];
                                // Also clean up any lease/lease_expiry entries.
                                if storage.get_lease(&lease_key).ok().flatten().is_some() {
                                    mutations.push(Mutation::DeleteLease { key: lease_key });
                                }
                                if let Err(e) = storage.apply_mutations(mutations) {
                                    tracing::error!(error = %e, "failed to replicate ack to broker storage");
                                }
                                return;
                            }
                        }
                    }
                    Err(e) => {
                        tracing::error!(error = %e, "failed to scan messages for ack replication");
                    }
                }
            }
            super::types::ClusterRequest::Nack {
                queue_id, msg_id, ..
            } => {
                // Nack: increment attempt count, clear leased_at, delete lease entries.
                let msg_prefix = crate::storage::keys::message_prefix(queue_id);
                match storage.list_messages(&msg_prefix) {
                    Ok(messages) => {
                        for (key, msg) in messages {
                            if msg.id == *msg_id {
                                let mut updated = msg;
                                updated.attempt_count += 1;
                                updated.leased_at = None;
                                let msg_value = match serde_json::to_vec(&updated) {
                                    Ok(v) => v,
                                    Err(e) => {
                                        tracing::error!(error = %e, "failed to serialize nack message");
                                        return;
                                    }
                                };
                                let lease_key = crate::storage::keys::lease_key(queue_id, msg_id);
                                let mut mutations = vec![Mutation::PutMessage {
                                    key,
                                    value: msg_value,
                                }];
                                if storage.get_lease(&lease_key).ok().flatten().is_some() {
                                    mutations.push(Mutation::DeleteLease { key: lease_key });
                                }
                                if let Err(e) = storage.apply_mutations(mutations) {
                                    tracing::error!(error = %e, "failed to replicate nack to broker storage");
                                }
                                return;
                            }
                        }
                    }
                    Err(e) => {
                        tracing::error!(error = %e, "failed to scan messages for nack replication");
                    }
                }
            }
            // Other request types are not queue-level data operations.
            _ => {}
        }
    }

    #[allow(clippy::result_large_err)]
    fn find_last_log_id(&self) -> Result<Option<LogId<NodeId>>, StorageError<NodeId>> {
        // Reverse-seek to the last key with the log prefix — O(1) instead of
        // scanning all log entries forward.
        let log_prefix = self.keys.log_prefix();
        let entry = self
            .db
            .raft_last_with_prefix(&log_prefix)
            .map_err(|e| StorageError::IO {
                source: openraft::StorageIOError::read_logs(&e),
            })?;

        match entry {
            Some((_key, value)) => {
                let entry: Entry<TypeConfig> =
                    serde_json::from_slice(&value).map_err(|e| StorageError::IO {
                        source: openraft::StorageIOError::read_logs(&e),
                    })?;
                Ok(Some(entry.log_id))
            }
            None => Ok(None),
        }
    }
}
