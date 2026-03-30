use std::fmt::Debug;
use std::io::Cursor;
use std::ops::RangeBounds;
use std::sync::Arc;

use openraft::storage::{LogState, RaftLogReader, RaftSnapshotBuilder, RaftStorage, Snapshot};
use openraft::{
    BasicNode, Entry, EntryPayload, LogId, SnapshotMeta, StorageError, StoredMembership, Vote,
};
use prost::Message as ProstMessage;
use serde::{Deserialize, Serialize};

use std::collections::HashMap;

use nonempty::NonEmpty;

use super::proto_convert;
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
        /// Which node should bootstrap (become initial leader) for this queue.
        preferred_leader: NodeId,
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

#[derive(Debug, Clone)]
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
    /// Committed entries (enqueue, ack, nack) are applied to `broker_storage`
    /// on all nodes for replication.
    pub fn for_queue(
        db: Arc<dyn RaftKeyValueStore>,
        queue_id: &str,
        broker_storage: Arc<dyn StorageEngine>,
    ) -> Self {
        Self {
            db,
            keys: KeyBuilder::for_queue(queue_id),
            state_machine: StateMachineData::default(),
            current_snapshot: None,
            meta_event_tx: None,
            broker_storage: Some(broker_storage),
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
            let proto_entry =
                fila_proto::RaftEntry::decode(&value[..]).map_err(|e| StorageError::IO {
                    source: openraft::StorageIOError::read_logs(&e),
                })?;
            let entry =
                proto_convert::entry_from_proto(proto_entry).map_err(|e| StorageError::IO {
                    source: openraft::StorageIOError::read_logs(&std::io::Error::other(
                        e.to_string(),
                    )),
                })?;
            entries.push(entry);
        }

        Ok(entries)
    }
}

impl RaftSnapshotBuilder<TypeConfig> for FilaRaftStore {
    async fn build_snapshot(&mut self) -> Result<Snapshot<TypeConfig>, StorageError<NodeId>> {
        let proto = fila_proto::StateMachineDataProto::from(&self.state_machine);
        let data = proto.encode_to_vec();

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
        let proto = proto_convert::vote_to_proto(*vote);
        let data = proto.encode_to_vec();
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
                let proto =
                    fila_proto::RaftVote::decode(&data[..]).map_err(|e| StorageError::IO {
                        source: openraft::StorageIOError::read_vote(&e),
                    })?;
                let vote = proto_convert::vote_from_proto(proto).map_err(|e| StorageError::IO {
                    source: openraft::StorageIOError::read_vote(&std::io::Error::other(
                        e.to_string(),
                    )),
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
                Some(data) if !data.is_empty() => {
                    let proto =
                        fila_proto::RaftLogId::decode(&data[..]).map_err(|e| StorageError::IO {
                            source: openraft::StorageIOError::read_logs(&e),
                        })?;
                    Some(
                        proto_convert::log_id_from_proto(proto).map_err(|e| StorageError::IO {
                            source: openraft::StorageIOError::read_logs(&std::io::Error::other(
                                e.to_string(),
                            )),
                        })?,
                    )
                }
                _ => None,
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
            let proto = proto_convert::entry_to_proto(entry);
            let value = proto.encode_to_vec();
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
        let proto = proto_convert::log_id_to_proto(log_id);
        let data = proto.encode_to_vec();
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
                        self.apply_to_broker_storage(storage.as_ref(), request, entry.log_id)?;
                    }

                    let response = match request {
                        super::types::ClusterRequest::Enqueue {
                            messages, message, ..
                        } => {
                            // Return the first message's ID for backward compatibility.
                            let msg_id = messages
                                .first()
                                .map(|m| m.id)
                                .or_else(|| message.as_ref().map(|m| m.id))
                                .unwrap_or(uuid::Uuid::nil());
                            ClusterResponse::Enqueue { msg_id }
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
                            preferred_leader,
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
                                        preferred_leader: *preferred_leader,
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

        let proto =
            fila_proto::StateMachineDataProto::decode(&data[..]).map_err(|e| StorageError::IO {
                source: openraft::StorageIOError::read_state_machine(&e),
            })?;
        let new_state: StateMachineData =
            proto
                .try_into()
                .map_err(|e: proto_convert::ConvertError| StorageError::IO {
                    source: openraft::StorageIOError::read_state_machine(&std::io::Error::other(
                        e.to_string(),
                    )),
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
                    // Snapshot restores don't need preferred_leader — groups are
                    // already initialized and Raft won't re-bootstrap.
                    let first_member = member_pairs.first().0;
                    let _ = tx.send(MetaStoreEvent::QueueGroupCreated {
                        queue_id: queue_id.clone(),
                        members: member_pairs,
                        config: Box::new(QueueConfig::new(queue_id.clone())),
                        preferred_leader: first_member,
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
    ///
    /// Returns `Err` if a storage mutation fails — the caller should propagate
    /// this as a `StorageError` so Raft can handle the failure appropriately.
    #[allow(clippy::result_large_err)] // StorageError<NodeId> is an openraft type we can't shrink; the caller apply_to_state_machine is an openraft trait method with a fixed signature — we cannot change its return type
    fn apply_to_broker_storage(
        &self,
        storage: &dyn StorageEngine,
        request: &super::types::ClusterRequest,
        log_id: LogId<NodeId>,
    ) -> Result<(), StorageError<NodeId>> {
        match request {
            super::types::ClusterRequest::Enqueue {
                messages, message, ..
            } => {
                // Resolve batch: new-format uses `messages`, legacy uses `message`.
                let msgs: Vec<&crate::message::Message> = if !messages.is_empty() {
                    messages.iter().collect()
                } else if let Some(m) = message {
                    vec![m]
                } else {
                    vec![]
                };
                for msg in msgs {
                    let queue_id = self.queue_id.as_deref().unwrap_or(&msg.queue_id);
                    let msg_key = crate::storage::keys::message_key(
                        queue_id,
                        &msg.fairness_key,
                        msg.enqueued_at,
                        &msg.id,
                    );
                    let idx_key = crate::storage::keys::msg_index_key(queue_id, &msg.id);
                    let proto = fila_proto::Message::from(msg.clone());
                    let msg_value = proto.encode_to_vec();
                    storage
                        .apply_mutations(vec![
                            Mutation::PutMessage {
                                key: msg_key.clone(),
                                value: msg_value,
                            },
                            Mutation::PutMsgIndex {
                                key: idx_key,
                                value: msg_key,
                            },
                        ])
                        .map_err(|e| StorageError::IO {
                            source: openraft::StorageIOError::apply(log_id, &e),
                        })?;
                }
            }
            super::types::ClusterRequest::Ack {
                items,
                queue_id: legacy_queue_id,
                msg_id: legacy_msg_id,
            } => {
                // Resolve batch: new-format uses `items`, legacy uses queue_id + msg_id.
                let ack_pairs: Vec<(&str, &uuid::Uuid)> = if !items.is_empty() {
                    items.iter().map(|i| (i.queue_id.as_str(), &i.msg_id)).collect()
                } else if let (Some(qid), Some(mid)) = (legacy_queue_id, legacy_msg_id) {
                    vec![(qid.as_str(), mid)]
                } else {
                    vec![]
                };
                for (queue_id, msg_id) in ack_pairs {
                    Self::apply_ack_to_storage(storage, queue_id, msg_id, log_id)?;
                }
            }
            super::types::ClusterRequest::Nack {
                items,
                queue_id: legacy_queue_id,
                msg_id: legacy_msg_id,
                ..
            } => {
                // Resolve batch: new-format uses `items`, legacy uses queue_id + msg_id.
                let nack_pairs: Vec<(&str, &uuid::Uuid)> = if !items.is_empty() {
                    items.iter().map(|i| (i.queue_id.as_str(), &i.msg_id)).collect()
                } else if let (Some(qid), Some(mid)) = (legacy_queue_id, legacy_msg_id) {
                    vec![(qid.as_str(), mid)]
                } else {
                    vec![]
                };
                for (queue_id, msg_id) in nack_pairs {
                    Self::apply_nack_to_storage(storage, queue_id, msg_id, log_id)?;
                }
            }
            // Queue-level data operations are Enqueue, Ack, Nack only.
            // Meta operations (CreateQueue, DeleteQueue, etc.) and group
            // operations (CreateQueueGroup, DeleteQueueGroup) are handled
            // by the meta Raft state machine, not the queue store.
            super::types::ClusterRequest::CreateQueue { .. }
            | super::types::ClusterRequest::DeleteQueue { .. }
            | super::types::ClusterRequest::SetConfig { .. }
            | super::types::ClusterRequest::SetThrottleRate { .. }
            | super::types::ClusterRequest::RemoveThrottleRate { .. }
            | super::types::ClusterRequest::Redrive { .. }
            | super::types::ClusterRequest::CreateQueueGroup { .. }
            | super::types::ClusterRequest::DeleteQueueGroup { .. } => {}
        }
        Ok(())
    }

    /// Apply a single ack to broker storage (extracted for reuse with batch).
    #[allow(clippy::result_large_err)]
    fn apply_ack_to_storage(
        storage: &dyn StorageEngine,
        queue_id: &str,
        msg_id: &uuid::Uuid,
        log_id: LogId<NodeId>,
    ) -> Result<(), StorageError<NodeId>> {
        let idx_key = crate::storage::keys::msg_index_key(queue_id, msg_id);
        let msg_key_opt =
            storage
                .get_msg_index(&idx_key)
                .map_err(|e| StorageError::IO {
                    source: openraft::StorageIOError::apply(log_id, &e),
                })?;

        let msg_key = if let Some(key) = msg_key_opt {
            if storage
                .get_message(&key)
                .map_err(|e| StorageError::IO {
                    source: openraft::StorageIOError::apply(log_id, &e),
                })?
                .is_some()
            {
                Some(key)
            } else {
                let msg_prefix = crate::storage::keys::message_prefix(queue_id);
                let messages =
                    storage
                        .list_messages(&msg_prefix)
                        .map_err(|e| StorageError::IO {
                            source: openraft::StorageIOError::apply(log_id, &e),
                        })?;
                messages
                    .into_iter()
                    .find(|(_, msg)| msg.id == *msg_id)
                    .map(|(key, _)| key)
            }
        } else {
            let msg_prefix = crate::storage::keys::message_prefix(queue_id);
            let messages =
                storage
                    .list_messages(&msg_prefix)
                    .map_err(|e| StorageError::IO {
                        source: openraft::StorageIOError::apply(log_id, &e),
                    })?;
            messages
                .into_iter()
                .find(|(_, msg)| msg.id == *msg_id)
                .map(|(key, _)| key)
        };

        if let Some(key) = msg_key {
            let lease_key = crate::storage::keys::lease_key(queue_id, msg_id);
            let mut mutations = vec![
                Mutation::DeleteMessage { key },
                Mutation::DeleteMsgIndex { key: idx_key },
            ];
            if let Some(lease_value) = storage.get_lease(&lease_key).ok().flatten() {
                mutations.push(Mutation::DeleteLease { key: lease_key });
                if let Some(expiry_ts) =
                    crate::storage::keys::parse_expiry_from_lease_value(&lease_value)
                {
                    let expiry_key =
                        crate::storage::keys::lease_expiry_key(expiry_ts, queue_id, msg_id);
                    mutations.push(Mutation::DeleteLeaseExpiry { key: expiry_key });
                }
            }
            storage
                .apply_mutations(mutations)
                .map_err(|e| StorageError::IO {
                    source: openraft::StorageIOError::apply(log_id, &e),
                })?;
        }
        Ok(())
    }

    /// Apply a single nack to broker storage (extracted for reuse with batch).
    #[allow(clippy::result_large_err)]
    fn apply_nack_to_storage(
        storage: &dyn StorageEngine,
        queue_id: &str,
        msg_id: &uuid::Uuid,
        log_id: LogId<NodeId>,
    ) -> Result<(), StorageError<NodeId>> {
        let idx_key = crate::storage::keys::msg_index_key(queue_id, msg_id);
        let msg_key_opt =
            storage
                .get_msg_index(&idx_key)
                .map_err(|e| StorageError::IO {
                    source: openraft::StorageIOError::apply(log_id, &e),
                })?;

        let found = if let Some(key) = msg_key_opt {
            match storage.get_message(&key).map_err(|e| StorageError::IO {
                source: openraft::StorageIOError::apply(log_id, &e),
            })? {
                Some(msg) => Some((key, msg)),
                None => {
                    let msg_prefix = crate::storage::keys::message_prefix(queue_id);
                    let messages = storage.list_messages(&msg_prefix).map_err(|e| {
                        StorageError::IO {
                            source: openraft::StorageIOError::apply(log_id, &e),
                        }
                    })?;
                    messages.into_iter().find(|(_, msg)| msg.id == *msg_id)
                }
            }
        } else {
            let msg_prefix = crate::storage::keys::message_prefix(queue_id);
            let messages =
                storage
                    .list_messages(&msg_prefix)
                    .map_err(|e| StorageError::IO {
                        source: openraft::StorageIOError::apply(log_id, &e),
                    })?;
            messages.into_iter().find(|(_, msg)| msg.id == *msg_id)
        };

        if let Some((key, msg)) = found {
            let mut updated = msg;
            updated.attempt_count += 1;
            updated.leased_at = None;
            let proto = fila_proto::Message::from(updated);
            let msg_value = proto.encode_to_vec();
            let lease_key = crate::storage::keys::lease_key(queue_id, msg_id);
            let mut mutations = vec![Mutation::PutMessage {
                key,
                value: msg_value,
            }];
            if let Some(lease_value) = storage.get_lease(&lease_key).ok().flatten() {
                mutations.push(Mutation::DeleteLease { key: lease_key });
                if let Some(expiry_ts) =
                    crate::storage::keys::parse_expiry_from_lease_value(&lease_value)
                {
                    let expiry_key =
                        crate::storage::keys::lease_expiry_key(expiry_ts, queue_id, msg_id);
                    mutations.push(Mutation::DeleteLeaseExpiry { key: expiry_key });
                }
            }
            storage
                .apply_mutations(mutations)
                .map_err(|e| StorageError::IO {
                    source: openraft::StorageIOError::apply(log_id, &e),
                })?;
        }
        Ok(())
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
                let proto =
                    fila_proto::RaftEntry::decode(&value[..]).map_err(|e| StorageError::IO {
                        source: openraft::StorageIOError::read_logs(&e),
                    })?;
                let entry =
                    proto_convert::entry_from_proto(proto).map_err(|e| StorageError::IO {
                        source: openraft::StorageIOError::read_logs(&std::io::Error::other(
                            e.to_string(),
                        )),
                    })?;
                Ok(Some(entry.log_id))
            }
            None => Ok(None),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::message::Message;
    use crate::storage::{keys, RocksDbEngine, StorageEngine};
    use std::collections::HashMap;
    use std::sync::Arc;

    fn test_storage() -> (Arc<RocksDbEngine>, tempfile::TempDir) {
        let dir = tempfile::tempdir().unwrap();
        let storage = Arc::new(RocksDbEngine::open(dir.path()).unwrap());
        (storage, dir)
    }

    fn test_message(queue_id: &str, fairness_key: &str) -> Message {
        Message {
            id: uuid::Uuid::now_v7(),
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

    fn make_store(
        db: Arc<RocksDbEngine>,
        storage: Arc<RocksDbEngine>,
        queue_id: &str,
    ) -> FilaRaftStore {
        FilaRaftStore::for_queue(
            db as Arc<dyn RaftKeyValueStore>,
            queue_id,
            storage as Arc<dyn StorageEngine>,
        )
    }

    fn test_log_id(index: u64) -> LogId<NodeId> {
        LogId::new(openraft::CommittedLeaderId::new(1, 1), index)
    }

    #[test]
    fn enqueue_writes_msg_index() {
        let (rocksdb, _dir) = test_storage();
        let store = make_store(Arc::clone(&rocksdb), Arc::clone(&rocksdb), "q1");
        let msg = test_message("q1", "tenant_a");

        let request = super::super::types::ClusterRequest::Enqueue {
            messages: vec![msg.clone()],
            message: None,
        };
        store
            .apply_to_broker_storage(rocksdb.as_ref(), &request, test_log_id(1))
            .unwrap();

        // Verify message was written.
        let prefix = keys::message_prefix("q1");
        let messages = rocksdb.list_messages(&prefix).unwrap();
        assert_eq!(messages.len(), 1);
        assert_eq!(messages[0].1.id, msg.id);

        // Verify msg_index was written.
        let idx_key = keys::msg_index_key("q1", &msg.id);
        let stored_key = rocksdb.get_msg_index(&idx_key).unwrap().unwrap();
        assert_eq!(stored_key, messages[0].0);
    }

    #[test]
    fn ack_uses_index_for_o1_lookup() {
        let (rocksdb, _dir) = test_storage();
        let store = make_store(Arc::clone(&rocksdb), Arc::clone(&rocksdb), "q1");

        // Enqueue a message (creates both message and index).
        let msg = test_message("q1", "tenant_a");
        let enqueue_req = super::super::types::ClusterRequest::Enqueue {
            messages: vec![msg.clone()],
            message: None,
        };
        store
            .apply_to_broker_storage(rocksdb.as_ref(), &enqueue_req, test_log_id(1))
            .unwrap();

        // Verify message and index exist.
        let prefix = keys::message_prefix("q1");
        assert_eq!(rocksdb.list_messages(&prefix).unwrap().len(), 1);
        let idx_key = keys::msg_index_key("q1", &msg.id);
        assert!(rocksdb.get_msg_index(&idx_key).unwrap().is_some());

        // Ack the message (should use index for O(1) lookup).
        let ack_req = super::super::types::ClusterRequest::Ack {
            items: vec![super::super::types::AckItemData {
                queue_id: "q1".to_string(),
                msg_id: msg.id,
            }],
            queue_id: None,
            msg_id: None,
        };
        store
            .apply_to_broker_storage(rocksdb.as_ref(), &ack_req, test_log_id(2))
            .unwrap();

        // Message and index should both be gone.
        assert_eq!(rocksdb.list_messages(&prefix).unwrap().len(), 0);
        assert!(rocksdb.get_msg_index(&idx_key).unwrap().is_none());
    }

    #[test]
    fn nack_uses_index_for_o1_lookup() {
        let (rocksdb, _dir) = test_storage();
        let store = make_store(Arc::clone(&rocksdb), Arc::clone(&rocksdb), "q1");

        // Enqueue a message.
        let msg = test_message("q1", "tenant_a");
        let enqueue_req = super::super::types::ClusterRequest::Enqueue {
            messages: vec![msg.clone()],
            message: None,
        };
        store
            .apply_to_broker_storage(rocksdb.as_ref(), &enqueue_req, test_log_id(1))
            .unwrap();

        // Nack the message (should use index for O(1) lookup).
        let nack_req = super::super::types::ClusterRequest::Nack {
            items: vec![super::super::types::NackItemData {
                queue_id: "q1".to_string(),
                msg_id: msg.id,
                error: "test error".to_string(),
            }],
            queue_id: None,
            msg_id: None,
            error: None,
        };
        store
            .apply_to_broker_storage(rocksdb.as_ref(), &nack_req, test_log_id(2))
            .unwrap();

        // Message should still exist with incremented attempt_count.
        let prefix = keys::message_prefix("q1");
        let messages = rocksdb.list_messages(&prefix).unwrap();
        assert_eq!(messages.len(), 1);
        assert_eq!(messages[0].1.attempt_count, 1);
        assert!(messages[0].1.leased_at.is_none());
    }

    #[test]
    fn ack_falls_back_to_scan_without_index() {
        let (rocksdb, _dir) = test_storage();
        let store = make_store(Arc::clone(&rocksdb), Arc::clone(&rocksdb), "q1");

        // Manually write a message WITHOUT an index entry (simulates
        // pre-index Raft log entries for backward compatibility).
        let msg = test_message("q1", "tenant_a");
        let msg_key = keys::message_key("q1", &msg.fairness_key, msg.enqueued_at, &msg.id);
        rocksdb.put_message(&msg_key, &msg).unwrap();

        // Verify no index exists.
        let idx_key = keys::msg_index_key("q1", &msg.id);
        assert!(rocksdb.get_msg_index(&idx_key).unwrap().is_none());

        // Ack should fall back to O(n) scan and still succeed.
        let ack_req = super::super::types::ClusterRequest::Ack {
            items: vec![super::super::types::AckItemData {
                queue_id: "q1".to_string(),
                msg_id: msg.id,
            }],
            queue_id: None,
            msg_id: None,
        };
        store
            .apply_to_broker_storage(rocksdb.as_ref(), &ack_req, test_log_id(1))
            .unwrap();

        let prefix = keys::message_prefix("q1");
        assert_eq!(rocksdb.list_messages(&prefix).unwrap().len(), 0);
    }

    #[test]
    fn nack_falls_back_to_scan_without_index() {
        let (rocksdb, _dir) = test_storage();
        let store = make_store(Arc::clone(&rocksdb), Arc::clone(&rocksdb), "q1");

        // Manually write a message without an index entry.
        let msg = test_message("q1", "tenant_a");
        let msg_key = keys::message_key("q1", &msg.fairness_key, msg.enqueued_at, &msg.id);
        rocksdb.put_message(&msg_key, &msg).unwrap();

        // Nack should fall back to scan and still succeed.
        let nack_req = super::super::types::ClusterRequest::Nack {
            items: vec![super::super::types::NackItemData {
                queue_id: "q1".to_string(),
                msg_id: msg.id,
                error: "test".to_string(),
            }],
            queue_id: None,
            msg_id: None,
            error: None,
        };
        store
            .apply_to_broker_storage(rocksdb.as_ref(), &nack_req, test_log_id(1))
            .unwrap();

        let prefix = keys::message_prefix("q1");
        let messages = rocksdb.list_messages(&prefix).unwrap();
        assert_eq!(messages.len(), 1);
        assert_eq!(messages[0].1.attempt_count, 1);
    }

    #[test]
    fn ack_with_many_messages_uses_index() {
        let (rocksdb, _dir) = test_storage();
        let store = make_store(Arc::clone(&rocksdb), Arc::clone(&rocksdb), "q1");

        // Enqueue 100 messages.
        let mut target_id = uuid::Uuid::nil();
        for i in 0..100 {
            let mut msg = test_message("q1", "tenant_a");
            msg.enqueued_at = (i + 1) * 1_000_000_000;
            if i == 50 {
                target_id = msg.id;
            }
            let req = super::super::types::ClusterRequest::Enqueue { messages: vec![msg], message: None };
            store
                .apply_to_broker_storage(rocksdb.as_ref(), &req, test_log_id(i + 1))
                .unwrap();
        }

        // Verify 100 messages exist.
        let prefix = keys::message_prefix("q1");
        assert_eq!(rocksdb.list_messages(&prefix).unwrap().len(), 100);

        // Ack the 51st message — with index this is O(1), not O(100).
        let ack_req = super::super::types::ClusterRequest::Ack {
            items: vec![super::super::types::AckItemData {
                queue_id: "q1".to_string(),
                msg_id: target_id,
            }],
            queue_id: None,
            msg_id: None,
        };
        store
            .apply_to_broker_storage(rocksdb.as_ref(), &ack_req, test_log_id(101))
            .unwrap();

        // 99 messages should remain.
        assert_eq!(rocksdb.list_messages(&prefix).unwrap().len(), 99);
        // Index for the acked message should be gone.
        let idx_key = keys::msg_index_key("q1", &target_id);
        assert!(rocksdb.get_msg_index(&idx_key).unwrap().is_none());
    }

    #[test]
    fn double_ack_is_idempotent() {
        let (rocksdb, _dir) = test_storage();
        let store = make_store(Arc::clone(&rocksdb), Arc::clone(&rocksdb), "q1");

        let msg = test_message("q1", "tenant_a");
        let enqueue_req = super::super::types::ClusterRequest::Enqueue {
            messages: vec![msg.clone()],
            message: None,
        };
        store
            .apply_to_broker_storage(rocksdb.as_ref(), &enqueue_req, test_log_id(1))
            .unwrap();

        let ack_req = super::super::types::ClusterRequest::Ack {
            items: vec![super::super::types::AckItemData {
                queue_id: "q1".to_string(),
                msg_id: msg.id,
            }],
            queue_id: None,
            msg_id: None,
        };
        // First ack — deletes message.
        store
            .apply_to_broker_storage(rocksdb.as_ref(), &ack_req, test_log_id(2))
            .unwrap();
        // Second ack — message already gone, should be a no-op.
        store
            .apply_to_broker_storage(rocksdb.as_ref(), &ack_req, test_log_id(3))
            .unwrap();

        let prefix = keys::message_prefix("q1");
        assert_eq!(rocksdb.list_messages(&prefix).unwrap().len(), 0);
    }

    #[test]
    fn ack_nonexistent_message_is_noop() {
        let (rocksdb, _dir) = test_storage();
        let store = make_store(Arc::clone(&rocksdb), Arc::clone(&rocksdb), "q1");

        let ack_req = super::super::types::ClusterRequest::Ack {
            items: vec![super::super::types::AckItemData {
                queue_id: "q1".to_string(),
                msg_id: uuid::Uuid::now_v7(),
            }],
            queue_id: None,
            msg_id: None,
        };
        // Ack a message that was never enqueued — should not error.
        store
            .apply_to_broker_storage(rocksdb.as_ref(), &ack_req, test_log_id(1))
            .unwrap();
    }
}
