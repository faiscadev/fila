//! Bidirectional conversions between openraft types and protobuf types.
//!
//! These conversions serve as an **openraft upgrade guardrail**: if openraft
//! adds, removes, or renames a field, the conversion code breaks at compile
//! time (struct field mismatch), and the roundtrip tests catch any semantic
//! drift.
//!
//! Many conversions use standalone functions instead of `From`/`TryFrom` to
//! satisfy Rust's orphan rule (both openraft and proto types are external).

use std::collections::{BTreeMap, BTreeSet, HashMap};

use openraft::raft::{
    AppendEntriesRequest, AppendEntriesResponse, InstallSnapshotRequest, InstallSnapshotResponse,
    VoteRequest, VoteResponse,
};
use openraft::{BasicNode, Entry, EntryPayload, LogId, Membership, SnapshotMeta, StoredMembership};

use super::store::StateMachineData;
use super::types::{ClusterRequest, ClusterResponse, NodeId, TypeConfig};
use crate::message::Message;
use crate::queue::QueueConfig;

/// Error type for proto → domain conversions.
#[derive(Debug, thiserror::Error)]
pub enum ConvertError {
    #[error("missing required field: {0}")]
    MissingField(&'static str),
    #[error("invalid uuid: {0}")]
    InvalidUuid(#[from] uuid::Error),
    #[error("invalid timestamp: out of range")]
    InvalidTimestamp,
}

/// Convert a protobuf `Timestamp` to nanoseconds since epoch.
/// Returns an error if seconds or nanos are out of valid range.
fn timestamp_to_nanos(ts: &prost_types::Timestamp) -> Result<u64, ConvertError> {
    if ts.seconds < 0 || !(0..1_000_000_000).contains(&ts.nanos) {
        return Err(ConvertError::InvalidTimestamp);
    }
    (ts.seconds as u64)
        .checked_mul(1_000_000_000)
        .and_then(|s| s.checked_add(ts.nanos as u64))
        .ok_or(ConvertError::InvalidTimestamp)
}

// ---------------------------------------------------------------------------
// Primitive openraft types (standalone functions — orphan rule)
// ---------------------------------------------------------------------------

pub(crate) fn leader_id_to_proto(l: openraft::LeaderId<NodeId>) -> fila_proto::RaftLeaderId {
    fila_proto::RaftLeaderId {
        term: l.term,
        node_id: l.node_id,
    }
}

pub(crate) fn leader_id_from_proto(p: fila_proto::RaftLeaderId) -> openraft::LeaderId<NodeId> {
    openraft::LeaderId::new(p.term, p.node_id)
}

pub(crate) fn log_id_to_proto(l: LogId<NodeId>) -> fila_proto::RaftLogId {
    // In adv mode, CommittedLeaderId is the same as LeaderId.
    fila_proto::RaftLogId {
        leader_id: Some(leader_id_to_proto(l.leader_id)),
        index: l.index,
    }
}

pub(crate) fn log_id_from_proto(p: fila_proto::RaftLogId) -> Result<LogId<NodeId>, ConvertError> {
    let leader_id = p
        .leader_id
        .ok_or(ConvertError::MissingField("log_id.leader_id"))?;
    Ok(LogId {
        leader_id: leader_id_from_proto(leader_id),
        index: p.index,
    })
}

pub(crate) fn vote_to_proto(v: openraft::Vote<NodeId>) -> fila_proto::RaftVote {
    fila_proto::RaftVote {
        leader_id: Some(leader_id_to_proto(v.leader_id)),
        committed: v.committed,
    }
}

pub(crate) fn vote_from_proto(
    p: fila_proto::RaftVote,
) -> Result<openraft::Vote<NodeId>, ConvertError> {
    let leader_id = p
        .leader_id
        .ok_or(ConvertError::MissingField("vote.leader_id"))?;
    Ok(openraft::Vote {
        leader_id: leader_id_from_proto(leader_id),
        committed: p.committed,
    })
}

pub(crate) fn membership_to_proto(m: Membership<NodeId, BasicNode>) -> fila_proto::RaftMembership {
    let configs = m
        .get_joint_config()
        .iter()
        .map(|set| fila_proto::RaftNodeIdSet {
            node_ids: set.iter().copied().collect(),
        })
        .collect();
    let nodes = m
        .nodes()
        .map(|(&id, n)| {
            (
                id,
                fila_proto::RaftBasicNode {
                    addr: n.addr.clone(),
                },
            )
        })
        .collect();
    fila_proto::RaftMembership { configs, nodes }
}

pub(crate) fn membership_from_proto(
    p: fila_proto::RaftMembership,
) -> Membership<NodeId, BasicNode> {
    let configs: Vec<BTreeSet<NodeId>> = p
        .configs
        .into_iter()
        .map(|set| set.node_ids.into_iter().collect())
        .collect();
    let nodes: BTreeMap<NodeId, BasicNode> = p
        .nodes
        .into_iter()
        .map(|(id, n)| (id, BasicNode { addr: n.addr }))
        .collect();
    Membership::new(configs, nodes)
}

pub(crate) fn stored_membership_to_proto(
    s: StoredMembership<NodeId, BasicNode>,
) -> fila_proto::RaftStoredMembership {
    fila_proto::RaftStoredMembership {
        log_id: s.log_id().as_ref().map(|l| log_id_to_proto(*l)),
        membership: Some(membership_to_proto(s.membership().clone())),
    }
}

pub(crate) fn stored_membership_from_proto(
    p: fila_proto::RaftStoredMembership,
) -> Result<StoredMembership<NodeId, BasicNode>, ConvertError> {
    let log_id = p.log_id.map(log_id_from_proto).transpose()?;
    let membership = p
        .membership
        .ok_or(ConvertError::MissingField("stored_membership.membership"))?;
    Ok(StoredMembership::new(
        log_id,
        membership_from_proto(membership),
    ))
}

pub(crate) fn snapshot_meta_to_proto(
    m: SnapshotMeta<NodeId, BasicNode>,
) -> fila_proto::RaftSnapshotMeta {
    fila_proto::RaftSnapshotMeta {
        last_log_id: m.last_log_id.map(log_id_to_proto),
        last_membership: Some(stored_membership_to_proto(m.last_membership)),
        snapshot_id: m.snapshot_id,
    }
}

pub(crate) fn snapshot_meta_from_proto(
    p: fila_proto::RaftSnapshotMeta,
) -> Result<SnapshotMeta<NodeId, BasicNode>, ConvertError> {
    let last_log_id = p.last_log_id.map(log_id_from_proto).transpose()?;
    let last_membership = p
        .last_membership
        .ok_or(ConvertError::MissingField("snapshot_meta.last_membership"))?;
    Ok(SnapshotMeta {
        last_log_id,
        last_membership: stored_membership_from_proto(last_membership)?,
        snapshot_id: p.snapshot_id,
    })
}

// ---------------------------------------------------------------------------
// Entry & EntryPayload
// ---------------------------------------------------------------------------

pub(crate) fn entry_to_proto(e: Entry<TypeConfig>) -> fila_proto::RaftEntry {
    fila_proto::RaftEntry {
        log_id: Some(log_id_to_proto(e.log_id)),
        payload: Some(entry_payload_to_proto(e.payload)),
    }
}

pub(crate) fn entry_from_proto(
    p: fila_proto::RaftEntry,
) -> Result<Entry<TypeConfig>, ConvertError> {
    let log_id = p.log_id.ok_or(ConvertError::MissingField("entry.log_id"))?;
    let payload = p
        .payload
        .ok_or(ConvertError::MissingField("entry.payload"))?;
    Ok(Entry {
        log_id: log_id_from_proto(log_id)?,
        payload: entry_payload_from_proto(payload)?,
    })
}

fn entry_payload_to_proto(p: EntryPayload<TypeConfig>) -> fila_proto::RaftEntryPayload {
    use fila_proto::raft_entry_payload::Payload;
    let payload = match p {
        EntryPayload::Blank => Some(Payload::Blank(true)),
        EntryPayload::Normal(req) => {
            Some(Payload::Normal(fila_proto::ClusterRequestProto::from(req)))
        }
        EntryPayload::Membership(m) => Some(Payload::Membership(membership_to_proto(m))),
    };
    fila_proto::RaftEntryPayload { payload }
}

fn entry_payload_from_proto(
    p: fila_proto::RaftEntryPayload,
) -> Result<EntryPayload<TypeConfig>, ConvertError> {
    use fila_proto::raft_entry_payload::Payload;
    match p
        .payload
        .ok_or(ConvertError::MissingField("entry_payload.payload"))?
    {
        Payload::Blank(_) => Ok(EntryPayload::Blank),
        Payload::Normal(req) => Ok(EntryPayload::Normal(req.try_into()?)),
        Payload::Membership(m) => Ok(EntryPayload::Membership(membership_from_proto(m))),
    }
}

// ---------------------------------------------------------------------------
// AppendEntriesRequest / Response
// ---------------------------------------------------------------------------

pub(crate) fn append_entries_request_to_proto(
    r: AppendEntriesRequest<TypeConfig>,
    group_id: String,
) -> fila_proto::RaftAppendEntriesRequest {
    fila_proto::RaftAppendEntriesRequest {
        vote: Some(vote_to_proto(r.vote)),
        prev_log_id: r.prev_log_id.map(log_id_to_proto),
        entries: r.entries.into_iter().map(entry_to_proto).collect(),
        leader_commit: r.leader_commit.map(log_id_to_proto),
        group_id,
    }
}

pub(crate) fn append_entries_request_from_proto(
    p: fila_proto::RaftAppendEntriesRequest,
) -> Result<AppendEntriesRequest<TypeConfig>, ConvertError> {
    let vote = p
        .vote
        .ok_or(ConvertError::MissingField("append_entries.vote"))?;
    let prev_log_id = p.prev_log_id.map(log_id_from_proto).transpose()?;
    let entries: Result<Vec<_>, _> = p.entries.into_iter().map(entry_from_proto).collect();
    let leader_commit = p.leader_commit.map(log_id_from_proto).transpose()?;
    Ok(AppendEntriesRequest {
        vote: vote_from_proto(vote)?,
        prev_log_id,
        entries: entries?,
        leader_commit,
    })
}

pub(crate) fn append_entries_response_to_proto(
    r: AppendEntriesResponse<NodeId>,
) -> fila_proto::RaftAppendEntriesResponse {
    use fila_proto::raft_append_entries_response::Result as R;
    let result = match r {
        AppendEntriesResponse::Success => Some(R::Success(true)),
        AppendEntriesResponse::PartialSuccess(log_id) => Some(R::PartialSuccess(match log_id {
            Some(l) => log_id_to_proto(l),
            None => fila_proto::RaftLogId {
                leader_id: Some(fila_proto::RaftLeaderId {
                    term: 0,
                    node_id: 0,
                }),
                index: 0,
            },
        })),
        AppendEntriesResponse::Conflict => Some(R::Conflict(true)),
        AppendEntriesResponse::HigherVote(v) => Some(R::HigherVote(vote_to_proto(v))),
    };
    fila_proto::RaftAppendEntriesResponse { result }
}

pub(crate) fn append_entries_response_from_proto(
    p: fila_proto::RaftAppendEntriesResponse,
) -> Result<AppendEntriesResponse<NodeId>, ConvertError> {
    use fila_proto::raft_append_entries_response::Result as R;
    match p
        .result
        .ok_or(ConvertError::MissingField("append_entries_response.result"))?
    {
        R::Success(_) => Ok(AppendEntriesResponse::Success),
        R::PartialSuccess(log_id) => {
            let l = log_id_from_proto(log_id)?;
            // A zero-valued LogId represents None (from the encoding side).
            if l.leader_id.term == 0 && l.index == 0 {
                Ok(AppendEntriesResponse::PartialSuccess(None))
            } else {
                Ok(AppendEntriesResponse::PartialSuccess(Some(l)))
            }
        }
        R::Conflict(_) => Ok(AppendEntriesResponse::Conflict),
        R::HigherVote(v) => Ok(AppendEntriesResponse::HigherVote(vote_from_proto(v)?)),
    }
}

// ---------------------------------------------------------------------------
// VoteRequest / Response
// ---------------------------------------------------------------------------

pub(crate) fn vote_request_to_proto(
    r: VoteRequest<NodeId>,
    group_id: String,
) -> fila_proto::RaftVoteRequest {
    fila_proto::RaftVoteRequest {
        vote: Some(vote_to_proto(r.vote)),
        last_log_id: r.last_log_id.map(log_id_to_proto),
        group_id,
    }
}

pub(crate) fn vote_request_from_proto(
    p: fila_proto::RaftVoteRequest,
) -> Result<VoteRequest<NodeId>, ConvertError> {
    let vote = p
        .vote
        .ok_or(ConvertError::MissingField("vote_request.vote"))?;
    let last_log_id = p.last_log_id.map(log_id_from_proto).transpose()?;
    Ok(VoteRequest {
        vote: vote_from_proto(vote)?,
        last_log_id,
    })
}

pub(crate) fn vote_response_to_proto(r: VoteResponse<NodeId>) -> fila_proto::RaftVoteResponse {
    fila_proto::RaftVoteResponse {
        vote: Some(vote_to_proto(r.vote)),
        vote_granted: r.vote_granted,
        last_log_id: r.last_log_id.map(log_id_to_proto),
    }
}

pub(crate) fn vote_response_from_proto(
    p: fila_proto::RaftVoteResponse,
) -> Result<VoteResponse<NodeId>, ConvertError> {
    let vote = p
        .vote
        .ok_or(ConvertError::MissingField("vote_response.vote"))?;
    let last_log_id = p.last_log_id.map(log_id_from_proto).transpose()?;
    Ok(VoteResponse {
        vote: vote_from_proto(vote)?,
        vote_granted: p.vote_granted,
        last_log_id,
    })
}

// ---------------------------------------------------------------------------
// InstallSnapshotRequest / Response
// ---------------------------------------------------------------------------

pub(crate) fn install_snapshot_request_to_proto(
    r: InstallSnapshotRequest<TypeConfig>,
    group_id: String,
) -> fila_proto::RaftInstallSnapshotRequest {
    fila_proto::RaftInstallSnapshotRequest {
        vote: Some(vote_to_proto(r.vote)),
        meta: Some(snapshot_meta_to_proto(r.meta)),
        offset: r.offset,
        data: r.data,
        done: r.done,
        group_id,
    }
}

pub(crate) fn install_snapshot_request_from_proto(
    p: fila_proto::RaftInstallSnapshotRequest,
) -> Result<InstallSnapshotRequest<TypeConfig>, ConvertError> {
    let vote = p
        .vote
        .ok_or(ConvertError::MissingField("install_snapshot.vote"))?;
    let meta = p
        .meta
        .ok_or(ConvertError::MissingField("install_snapshot.meta"))?;
    Ok(InstallSnapshotRequest {
        vote: vote_from_proto(vote)?,
        meta: snapshot_meta_from_proto(meta)?,
        offset: p.offset,
        data: p.data,
        done: p.done,
    })
}

pub(crate) fn install_snapshot_response_to_proto(
    r: InstallSnapshotResponse<NodeId>,
) -> fila_proto::RaftInstallSnapshotResponse {
    fila_proto::RaftInstallSnapshotResponse {
        vote: Some(vote_to_proto(r.vote)),
    }
}

pub(crate) fn install_snapshot_response_from_proto(
    p: fila_proto::RaftInstallSnapshotResponse,
) -> Result<InstallSnapshotResponse<NodeId>, ConvertError> {
    let vote = p
        .vote
        .ok_or(ConvertError::MissingField("install_snapshot_response.vote"))?;
    Ok(InstallSnapshotResponse {
        vote: vote_from_proto(vote)?,
    })
}

// ---------------------------------------------------------------------------
// Message (domain ↔ proto) — local type, From/TryFrom OK
// ---------------------------------------------------------------------------

impl From<Message> for fila_proto::Message {
    fn from(m: Message) -> Self {
        Self {
            id: m.id.to_string(),
            headers: m.headers,
            payload: m.payload,
            metadata: Some(fila_proto::MessageMetadata {
                fairness_key: m.fairness_key,
                weight: m.weight,
                throttle_keys: m.throttle_keys,
                attempt_count: m.attempt_count,
                queue_id: m.queue_id,
            }),
            timestamps: Some(fila_proto::MessageTimestamps {
                enqueued_at: Some(prost_types::Timestamp {
                    seconds: (m.enqueued_at / 1_000_000_000) as i64,
                    nanos: (m.enqueued_at % 1_000_000_000) as i32,
                }),
                leased_at: m.leased_at.map(|ts| prost_types::Timestamp {
                    seconds: (ts / 1_000_000_000) as i64,
                    nanos: (ts % 1_000_000_000) as i32,
                }),
            }),
        }
    }
}

impl TryFrom<fila_proto::Message> for Message {
    type Error = ConvertError;

    fn try_from(p: fila_proto::Message) -> Result<Self, Self::Error> {
        let id: uuid::Uuid = p.id.parse().map_err(ConvertError::InvalidUuid)?;
        let metadata = p
            .metadata
            .ok_or(ConvertError::MissingField("message.metadata"))?;
        let timestamps = p.timestamps.unwrap_or_default();

        let enqueued_at = timestamps
            .enqueued_at
            .as_ref()
            .map(timestamp_to_nanos)
            .transpose()?
            .unwrap_or(0);

        let leased_at = timestamps
            .leased_at
            .as_ref()
            .map(timestamp_to_nanos)
            .transpose()?;

        Ok(Message {
            id,
            queue_id: metadata.queue_id,
            headers: p.headers,
            payload: p.payload,
            fairness_key: metadata.fairness_key,
            weight: metadata.weight,
            throttle_keys: metadata.throttle_keys,
            attempt_count: metadata.attempt_count,
            enqueued_at,
            leased_at,
        })
    }
}

// ---------------------------------------------------------------------------
// QueueConfig — local type, From OK
// ---------------------------------------------------------------------------

impl From<QueueConfig> for fila_proto::ClusterQueueConfig {
    fn from(c: QueueConfig) -> Self {
        Self {
            name: c.name,
            on_enqueue_script: c.on_enqueue_script,
            on_failure_script: c.on_failure_script,
            visibility_timeout_ms: c.visibility_timeout_ms,
            dlq_queue_id: c.dlq_queue_id,
            lua_timeout_ms: c.lua_timeout_ms,
            lua_memory_limit_bytes: c.lua_memory_limit_bytes.map(|v| v as u64),
        }
    }
}

impl From<fila_proto::ClusterQueueConfig> for QueueConfig {
    fn from(p: fila_proto::ClusterQueueConfig) -> Self {
        Self {
            name: p.name,
            on_enqueue_script: p.on_enqueue_script,
            on_failure_script: p.on_failure_script,
            visibility_timeout_ms: p.visibility_timeout_ms,
            dlq_queue_id: p.dlq_queue_id,
            lua_timeout_ms: p.lua_timeout_ms,
            lua_memory_limit_bytes: p.lua_memory_limit_bytes.map(|v| v as usize),
        }
    }
}

// ---------------------------------------------------------------------------
// ClusterRequest — local type, From/TryFrom OK
// ---------------------------------------------------------------------------

impl From<ClusterRequest> for fila_proto::ClusterRequestProto {
    fn from(r: ClusterRequest) -> Self {
        use fila_proto::cluster_request_proto::Request;
        let request = match r {
            ClusterRequest::Enqueue { message } => {
                Some(Request::Enqueue(fila_proto::ClusterEnqueue {
                    message: Some(fila_proto::Message::from(message)),
                }))
            }
            ClusterRequest::Ack { queue_id, msg_id } => {
                Some(Request::Ack(fila_proto::ClusterAck {
                    queue_id,
                    msg_id: msg_id.to_string(),
                }))
            }
            ClusterRequest::Nack {
                queue_id,
                msg_id,
                error,
            } => Some(Request::Nack(fila_proto::ClusterNack {
                queue_id,
                msg_id: msg_id.to_string(),
                error,
            })),
            ClusterRequest::CreateQueue { name, config } => {
                Some(Request::CreateQueue(fila_proto::ClusterCreateQueue {
                    name,
                    config: Some(fila_proto::ClusterQueueConfig::from(config)),
                }))
            }
            ClusterRequest::DeleteQueue { queue_id } => {
                Some(Request::DeleteQueue(fila_proto::ClusterDeleteQueue {
                    queue_id,
                }))
            }
            ClusterRequest::SetConfig { key, value } => {
                Some(Request::SetConfig(fila_proto::ClusterSetConfig {
                    key,
                    value,
                }))
            }
            ClusterRequest::SetThrottleRate {
                key,
                rate_per_second,
                burst,
            } => Some(Request::SetThrottleRate(
                fila_proto::ClusterSetThrottleRate {
                    key,
                    rate_per_second,
                    burst,
                },
            )),
            ClusterRequest::RemoveThrottleRate { key } => Some(Request::RemoveThrottleRate(
                fila_proto::ClusterRemoveThrottleRate { key },
            )),
            ClusterRequest::Redrive {
                dlq_queue_id,
                count,
            } => Some(Request::Redrive(fila_proto::ClusterRedrive {
                dlq_queue_id,
                count,
            })),
            ClusterRequest::CreateQueueGroup {
                queue_id,
                members,
                config,
                preferred_leader,
            } => Some(Request::CreateQueueGroup(
                fila_proto::ClusterCreateQueueGroup {
                    queue_id,
                    members,
                    config: Some(fila_proto::ClusterQueueConfig::from(config)),
                    preferred_leader,
                },
            )),
            ClusterRequest::DeleteQueueGroup { queue_id } => Some(Request::DeleteQueueGroup(
                fila_proto::ClusterDeleteQueueGroup { queue_id },
            )),
        };
        Self { request }
    }
}

impl TryFrom<fila_proto::ClusterRequestProto> for ClusterRequest {
    type Error = ConvertError;

    fn try_from(p: fila_proto::ClusterRequestProto) -> Result<Self, Self::Error> {
        use fila_proto::cluster_request_proto::Request;
        match p
            .request
            .ok_or(ConvertError::MissingField("cluster_request.request"))?
        {
            Request::Enqueue(e) => {
                let message = e
                    .message
                    .ok_or(ConvertError::MissingField("enqueue.message"))?
                    .try_into()?;
                Ok(ClusterRequest::Enqueue { message })
            }
            Request::Ack(a) => Ok(ClusterRequest::Ack {
                queue_id: a.queue_id,
                msg_id: a.msg_id.parse().map_err(ConvertError::InvalidUuid)?,
            }),
            Request::Nack(n) => Ok(ClusterRequest::Nack {
                queue_id: n.queue_id,
                msg_id: n.msg_id.parse().map_err(ConvertError::InvalidUuid)?,
                error: n.error,
            }),
            Request::CreateQueue(cq) => Ok(ClusterRequest::CreateQueue {
                name: cq.name,
                config: cq
                    .config
                    .ok_or(ConvertError::MissingField("create_queue.config"))?
                    .into(),
            }),
            Request::DeleteQueue(dq) => Ok(ClusterRequest::DeleteQueue {
                queue_id: dq.queue_id,
            }),
            Request::SetConfig(sc) => Ok(ClusterRequest::SetConfig {
                key: sc.key,
                value: sc.value,
            }),
            Request::SetThrottleRate(st) => Ok(ClusterRequest::SetThrottleRate {
                key: st.key,
                rate_per_second: st.rate_per_second,
                burst: st.burst,
            }),
            Request::RemoveThrottleRate(rt) => {
                Ok(ClusterRequest::RemoveThrottleRate { key: rt.key })
            }
            Request::Redrive(rd) => Ok(ClusterRequest::Redrive {
                dlq_queue_id: rd.dlq_queue_id,
                count: rd.count,
            }),
            Request::CreateQueueGroup(cqg) => {
                let preferred_leader = cqg.preferred_leader;
                Ok(ClusterRequest::CreateQueueGroup {
                    queue_id: cqg.queue_id,
                    members: cqg.members,
                    config: cqg
                        .config
                        .ok_or(ConvertError::MissingField("create_queue_group.config"))?
                        .into(),
                    preferred_leader,
                })
            }
            Request::DeleteQueueGroup(dqg) => Ok(ClusterRequest::DeleteQueueGroup {
                queue_id: dqg.queue_id,
            }),
        }
    }
}

// ---------------------------------------------------------------------------
// ClusterResponse — local type, From/TryFrom OK
// ---------------------------------------------------------------------------

impl From<ClusterResponse> for fila_proto::ClusterResponseProto {
    fn from(r: ClusterResponse) -> Self {
        use fila_proto::cluster_response_proto::Response;
        let response = match r {
            ClusterResponse::Enqueue { msg_id } => {
                Some(Response::Enqueue(fila_proto::ClusterEnqueueResponse {
                    msg_id: msg_id.to_string(),
                }))
            }
            ClusterResponse::Ack => Some(Response::Ack(true)),
            ClusterResponse::Nack => Some(Response::Nack(true)),
            ClusterResponse::CreateQueue { queue_id } => Some(Response::CreateQueue(
                fila_proto::ClusterCreateQueueResponse { queue_id },
            )),
            ClusterResponse::DeleteQueue => Some(Response::DeleteQueue(true)),
            ClusterResponse::SetConfig => Some(Response::SetConfig(true)),
            ClusterResponse::SetThrottleRate => Some(Response::SetThrottleRate(true)),
            ClusterResponse::RemoveThrottleRate => Some(Response::RemoveThrottleRate(true)),
            ClusterResponse::Redrive { count } => {
                Some(Response::Redrive(fila_proto::ClusterRedriveResponse {
                    count,
                }))
            }
            ClusterResponse::CreateQueueGroup { queue_id } => Some(Response::CreateQueueGroup(
                fila_proto::ClusterCreateQueueGroupResponse { queue_id },
            )),
            ClusterResponse::DeleteQueueGroup => Some(Response::DeleteQueueGroup(true)),
            ClusterResponse::Error { message } => Some(Response::Error(message)),
        };
        Self { response }
    }
}

impl TryFrom<fila_proto::ClusterResponseProto> for ClusterResponse {
    type Error = ConvertError;

    fn try_from(p: fila_proto::ClusterResponseProto) -> Result<Self, ConvertError> {
        use fila_proto::cluster_response_proto::Response;
        match p
            .response
            .ok_or(ConvertError::MissingField("cluster_response.response"))?
        {
            Response::Enqueue(e) => Ok(ClusterResponse::Enqueue {
                msg_id: e.msg_id.parse().map_err(ConvertError::InvalidUuid)?,
            }),
            Response::Ack(_) => Ok(ClusterResponse::Ack),
            Response::Nack(_) => Ok(ClusterResponse::Nack),
            Response::CreateQueue(cq) => Ok(ClusterResponse::CreateQueue {
                queue_id: cq.queue_id,
            }),
            Response::DeleteQueue(_) => Ok(ClusterResponse::DeleteQueue),
            Response::SetConfig(_) => Ok(ClusterResponse::SetConfig),
            Response::SetThrottleRate(_) => Ok(ClusterResponse::SetThrottleRate),
            Response::RemoveThrottleRate(_) => Ok(ClusterResponse::RemoveThrottleRate),
            Response::Redrive(rd) => Ok(ClusterResponse::Redrive { count: rd.count }),
            Response::CreateQueueGroup(cqg) => Ok(ClusterResponse::CreateQueueGroup {
                queue_id: cqg.queue_id,
            }),
            Response::DeleteQueueGroup(_) => Ok(ClusterResponse::DeleteQueueGroup),
            Response::Error(msg) => Ok(ClusterResponse::Error { message: msg }),
        }
    }
}

// ---------------------------------------------------------------------------
// StateMachineData — local type, From/TryFrom OK
// ---------------------------------------------------------------------------

impl From<&StateMachineData> for fila_proto::StateMachineDataProto {
    fn from(s: &StateMachineData) -> Self {
        Self {
            last_applied_log: s.last_applied_log.map(log_id_to_proto),
            last_membership: Some(stored_membership_to_proto(s.last_membership.clone())),
            queue_groups: s
                .queue_groups
                .iter()
                .map(|(k, v)| {
                    (
                        k.clone(),
                        fila_proto::QueueGroupMembers { members: v.clone() },
                    )
                })
                .collect(),
        }
    }
}

impl TryFrom<fila_proto::StateMachineDataProto> for StateMachineData {
    type Error = ConvertError;

    fn try_from(p: fila_proto::StateMachineDataProto) -> Result<Self, Self::Error> {
        let last_applied_log = p.last_applied_log.map(log_id_from_proto).transpose()?;
        let last_membership = p.last_membership.ok_or(ConvertError::MissingField(
            "state_machine_data.last_membership",
        ))?;
        let queue_groups: HashMap<String, Vec<u64>> = p
            .queue_groups
            .into_iter()
            .map(|(k, v)| (k, v.members))
            .collect();
        Ok(StateMachineData {
            last_applied_log,
            last_membership: stored_membership_from_proto(last_membership)?,
            queue_groups,
        })
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use prost::Message as ProstMessage;

    fn make_message() -> Message {
        Message {
            id: uuid::Uuid::now_v7(),
            queue_id: "test-queue".to_string(),
            headers: HashMap::from([("key".to_string(), "value".to_string())]),
            payload: b"hello world".to_vec(),
            fairness_key: "default".to_string(),
            weight: 1,
            throttle_keys: vec!["rate-1".to_string()],
            attempt_count: 3,
            enqueued_at: 1_700_000_000_000_000_000,
            leased_at: Some(1_700_000_001_000_000_000),
        }
    }

    fn make_log_id(term: u64, index: u64) -> LogId<NodeId> {
        LogId {
            leader_id: openraft::LeaderId::new(term, 0),
            index,
        }
    }

    fn make_vote(term: u64, node_id: u64, committed: bool) -> openraft::Vote<NodeId> {
        if committed {
            openraft::Vote::new_committed(term, node_id)
        } else {
            openraft::Vote::new(term, node_id)
        }
    }

    fn make_membership() -> Membership<NodeId, BasicNode> {
        let configs = vec![[1u64, 2, 3].into_iter().collect::<BTreeSet<_>>()];
        let nodes: BTreeMap<NodeId, BasicNode> = [
            (
                1,
                BasicNode {
                    addr: "addr1".into(),
                },
            ),
            (
                2,
                BasicNode {
                    addr: "addr2".into(),
                },
            ),
            (
                3,
                BasicNode {
                    addr: "addr3".into(),
                },
            ),
        ]
        .into();
        Membership::new(configs, nodes)
    }

    #[test]
    fn roundtrip_log_id() {
        let original = make_log_id(5, 42);
        let proto = log_id_to_proto(original);
        let bytes = proto.encode_to_vec();
        let decoded = fila_proto::RaftLogId::decode(bytes.as_slice()).unwrap();
        let roundtripped = log_id_from_proto(decoded).unwrap();
        assert_eq!(original, roundtripped);
    }

    #[test]
    fn roundtrip_vote_committed() {
        let original = make_vote(3, 7, true);
        let proto = vote_to_proto(original);
        let bytes = proto.encode_to_vec();
        let decoded = fila_proto::RaftVote::decode(bytes.as_slice()).unwrap();
        let roundtripped = vote_from_proto(decoded).unwrap();
        assert_eq!(original, roundtripped);
    }

    #[test]
    fn roundtrip_vote_uncommitted() {
        let original = make_vote(1, 0, false);
        let proto = vote_to_proto(original);
        let bytes = proto.encode_to_vec();
        let decoded = fila_proto::RaftVote::decode(bytes.as_slice()).unwrap();
        let roundtripped = vote_from_proto(decoded).unwrap();
        assert_eq!(original, roundtripped);
    }

    #[test]
    fn roundtrip_entry_blank() {
        let original = Entry::<TypeConfig> {
            log_id: make_log_id(1, 0),
            payload: EntryPayload::Blank,
        };
        let proto = entry_to_proto(original.clone());
        let bytes = proto.encode_to_vec();
        let decoded = fila_proto::RaftEntry::decode(bytes.as_slice()).unwrap();
        let roundtripped = entry_from_proto(decoded).unwrap();
        assert_eq!(original.log_id, roundtripped.log_id);
        assert!(matches!(roundtripped.payload, EntryPayload::Blank));
    }

    #[test]
    fn roundtrip_entry_normal() {
        let msg = make_message();
        let original = Entry::<TypeConfig> {
            log_id: make_log_id(2, 5),
            payload: EntryPayload::Normal(ClusterRequest::Enqueue {
                message: msg.clone(),
            }),
        };
        let proto = entry_to_proto(original.clone());
        let bytes = proto.encode_to_vec();
        let decoded = fila_proto::RaftEntry::decode(bytes.as_slice()).unwrap();
        let roundtripped = entry_from_proto(decoded).unwrap();
        assert_eq!(original.log_id, roundtripped.log_id);
        match roundtripped.payload {
            EntryPayload::Normal(ClusterRequest::Enqueue { message }) => {
                assert_eq!(msg, message);
            }
            _ => panic!("expected Normal(Enqueue)"),
        }
    }

    #[test]
    fn roundtrip_entry_membership() {
        let membership = make_membership();
        let original = Entry::<TypeConfig> {
            log_id: make_log_id(3, 10),
            payload: EntryPayload::Membership(membership.clone()),
        };
        let proto = entry_to_proto(original.clone());
        let bytes = proto.encode_to_vec();
        let decoded = fila_proto::RaftEntry::decode(bytes.as_slice()).unwrap();
        let roundtripped = entry_from_proto(decoded).unwrap();
        assert_eq!(original.log_id, roundtripped.log_id);
        match roundtripped.payload {
            EntryPayload::Membership(m) => assert_eq!(membership, m),
            _ => panic!("expected Membership"),
        }
    }

    #[test]
    fn roundtrip_append_entries_request() {
        let msg = make_message();
        let original = AppendEntriesRequest::<TypeConfig> {
            vote: make_vote(5, 1, true),
            prev_log_id: Some(make_log_id(4, 99)),
            entries: vec![
                Entry {
                    log_id: make_log_id(5, 100),
                    payload: EntryPayload::Normal(ClusterRequest::Enqueue {
                        message: msg.clone(),
                    }),
                },
                Entry {
                    log_id: make_log_id(5, 101),
                    payload: EntryPayload::Blank,
                },
            ],
            leader_commit: Some(make_log_id(5, 99)),
        };
        let proto = append_entries_request_to_proto(original.clone(), String::new());
        let bytes = proto.encode_to_vec();
        let decoded = fila_proto::RaftAppendEntriesRequest::decode(bytes.as_slice()).unwrap();
        let roundtripped = append_entries_request_from_proto(decoded).unwrap();
        assert_eq!(original.vote, roundtripped.vote);
        assert_eq!(original.prev_log_id, roundtripped.prev_log_id);
        assert_eq!(original.leader_commit, roundtripped.leader_commit);
        assert_eq!(original.entries.len(), roundtripped.entries.len());
    }

    #[test]
    fn roundtrip_append_entries_response_success() {
        let proto = append_entries_response_to_proto(AppendEntriesResponse::Success);
        let bytes = proto.encode_to_vec();
        let decoded = fila_proto::RaftAppendEntriesResponse::decode(bytes.as_slice()).unwrap();
        let roundtripped = append_entries_response_from_proto(decoded).unwrap();
        assert_eq!(AppendEntriesResponse::<NodeId>::Success, roundtripped);
    }

    #[test]
    fn roundtrip_append_entries_response_partial_success() {
        let log_id = make_log_id(3, 50);
        let proto =
            append_entries_response_to_proto(AppendEntriesResponse::PartialSuccess(Some(log_id)));
        let bytes = proto.encode_to_vec();
        let decoded = fila_proto::RaftAppendEntriesResponse::decode(bytes.as_slice()).unwrap();
        let roundtripped = append_entries_response_from_proto(decoded).unwrap();
        assert_eq!(
            AppendEntriesResponse::<NodeId>::PartialSuccess(Some(log_id)),
            roundtripped
        );
    }

    #[test]
    fn roundtrip_append_entries_response_conflict() {
        let proto = append_entries_response_to_proto(AppendEntriesResponse::Conflict);
        let bytes = proto.encode_to_vec();
        let decoded = fila_proto::RaftAppendEntriesResponse::decode(bytes.as_slice()).unwrap();
        let roundtripped = append_entries_response_from_proto(decoded).unwrap();
        assert_eq!(AppendEntriesResponse::<NodeId>::Conflict, roundtripped);
    }

    #[test]
    fn roundtrip_append_entries_response_higher_vote() {
        let vote = make_vote(10, 5, true);
        let proto = append_entries_response_to_proto(AppendEntriesResponse::HigherVote(vote));
        let bytes = proto.encode_to_vec();
        let decoded = fila_proto::RaftAppendEntriesResponse::decode(bytes.as_slice()).unwrap();
        let roundtripped = append_entries_response_from_proto(decoded).unwrap();
        assert_eq!(
            AppendEntriesResponse::<NodeId>::HigherVote(vote),
            roundtripped
        );
    }

    #[test]
    fn roundtrip_vote_request() {
        let original = VoteRequest::<NodeId> {
            vote: make_vote(3, 1, false),
            last_log_id: Some(make_log_id(2, 15)),
        };
        let proto = vote_request_to_proto(original.clone(), String::new());
        let bytes = proto.encode_to_vec();
        let decoded = fila_proto::RaftVoteRequest::decode(bytes.as_slice()).unwrap();
        let roundtripped = vote_request_from_proto(decoded).unwrap();
        assert_eq!(original, roundtripped);
    }

    #[test]
    fn roundtrip_vote_response() {
        let original = VoteResponse::<NodeId> {
            vote: make_vote(3, 2, true),
            vote_granted: true,
            last_log_id: Some(make_log_id(3, 20)),
        };
        let proto = vote_response_to_proto(original.clone());
        let bytes = proto.encode_to_vec();
        let decoded = fila_proto::RaftVoteResponse::decode(bytes.as_slice()).unwrap();
        let roundtripped = vote_response_from_proto(decoded).unwrap();
        assert_eq!(original, roundtripped);
    }

    #[test]
    fn roundtrip_install_snapshot_request() {
        let original = InstallSnapshotRequest::<TypeConfig> {
            vote: make_vote(5, 1, true),
            meta: SnapshotMeta {
                last_log_id: Some(make_log_id(5, 100)),
                last_membership: StoredMembership::new(Some(make_log_id(4, 50)), make_membership()),
                snapshot_id: "snap-5-1-100".to_string(),
            },
            offset: 0,
            data: vec![1, 2, 3, 4],
            done: true,
        };
        let proto = install_snapshot_request_to_proto(original.clone(), String::new());
        let bytes = proto.encode_to_vec();
        let decoded = fila_proto::RaftInstallSnapshotRequest::decode(bytes.as_slice()).unwrap();
        let roundtripped = install_snapshot_request_from_proto(decoded).unwrap();
        assert_eq!(original, roundtripped);
    }

    #[test]
    fn roundtrip_install_snapshot_response() {
        let vote = make_vote(5, 1, true);
        let proto = install_snapshot_response_to_proto(InstallSnapshotResponse { vote });
        let bytes = proto.encode_to_vec();
        let decoded = fila_proto::RaftInstallSnapshotResponse::decode(bytes.as_slice()).unwrap();
        let roundtripped = install_snapshot_response_from_proto(decoded).unwrap();
        assert_eq!(InstallSnapshotResponse::<NodeId> { vote }, roundtripped);
    }

    #[test]
    fn roundtrip_cluster_request_enqueue() {
        let msg = make_message();
        let original = ClusterRequest::Enqueue {
            message: msg.clone(),
        };
        let proto = fila_proto::ClusterRequestProto::from(original);
        let bytes = proto.encode_to_vec();
        let decoded = fila_proto::ClusterRequestProto::decode(bytes.as_slice()).unwrap();
        let roundtripped: ClusterRequest = decoded.try_into().unwrap();
        match roundtripped {
            ClusterRequest::Enqueue { message } => assert_eq!(msg, message),
            _ => panic!("expected Enqueue"),
        }
    }

    #[test]
    fn roundtrip_cluster_request_ack() {
        let id = uuid::Uuid::now_v7();
        let original = ClusterRequest::Ack {
            queue_id: "q1".into(),
            msg_id: id,
        };
        let proto = fila_proto::ClusterRequestProto::from(original);
        let roundtripped: ClusterRequest = proto.try_into().unwrap();
        match roundtripped {
            ClusterRequest::Ack { queue_id, msg_id } => {
                assert_eq!(queue_id, "q1");
                assert_eq!(msg_id, id);
            }
            _ => panic!("expected Ack"),
        }
    }

    #[test]
    fn roundtrip_cluster_request_nack() {
        let id = uuid::Uuid::now_v7();
        let original = ClusterRequest::Nack {
            queue_id: "q1".into(),
            msg_id: id,
            error: "test error".into(),
        };
        let proto = fila_proto::ClusterRequestProto::from(original);
        let roundtripped: ClusterRequest = proto.try_into().unwrap();
        match roundtripped {
            ClusterRequest::Nack {
                queue_id,
                msg_id,
                error,
            } => {
                assert_eq!(queue_id, "q1");
                assert_eq!(msg_id, id);
                assert_eq!(error, "test error");
            }
            _ => panic!("expected Nack"),
        }
    }

    #[test]
    fn roundtrip_cluster_request_create_queue() {
        let original = ClusterRequest::CreateQueue {
            name: "my-queue".into(),
            config: QueueConfig {
                name: "my-queue".into(),
                on_enqueue_script: Some("script".into()),
                on_failure_script: None,
                visibility_timeout_ms: 60_000,
                dlq_queue_id: Some("dlq".into()),
                lua_timeout_ms: Some(5000),
                lua_memory_limit_bytes: Some(1024 * 1024),
            },
        };
        let proto = fila_proto::ClusterRequestProto::from(original);
        let roundtripped: ClusterRequest = proto.try_into().unwrap();
        match roundtripped {
            ClusterRequest::CreateQueue { name, config } => {
                assert_eq!(name, "my-queue");
                assert_eq!(config.on_enqueue_script, Some("script".into()));
                assert_eq!(config.dlq_queue_id, Some("dlq".into()));
                assert_eq!(config.lua_timeout_ms, Some(5000));
                assert_eq!(config.lua_memory_limit_bytes, Some(1024 * 1024));
            }
            _ => panic!("expected CreateQueue"),
        }
    }

    #[test]
    fn roundtrip_cluster_request_delete_queue() {
        let original = ClusterRequest::DeleteQueue {
            queue_id: "q1".into(),
        };
        let proto = fila_proto::ClusterRequestProto::from(original);
        let roundtripped: ClusterRequest = proto.try_into().unwrap();
        assert!(
            matches!(roundtripped, ClusterRequest::DeleteQueue { queue_id } if queue_id == "q1")
        );
    }

    #[test]
    fn roundtrip_cluster_request_set_config() {
        let original = ClusterRequest::SetConfig {
            key: "k".into(),
            value: "v".into(),
        };
        let proto = fila_proto::ClusterRequestProto::from(original);
        let roundtripped: ClusterRequest = proto.try_into().unwrap();
        assert!(
            matches!(roundtripped, ClusterRequest::SetConfig { key, value } if key == "k" && value == "v")
        );
    }

    #[test]
    fn roundtrip_cluster_request_set_throttle_rate() {
        let original = ClusterRequest::SetThrottleRate {
            key: "k".into(),
            rate_per_second: 10.5,
            burst: 20.0,
        };
        let proto = fila_proto::ClusterRequestProto::from(original);
        let roundtripped: ClusterRequest = proto.try_into().unwrap();
        match roundtripped {
            ClusterRequest::SetThrottleRate {
                key,
                rate_per_second,
                burst,
            } => {
                assert_eq!(key, "k");
                assert!((rate_per_second - 10.5).abs() < f64::EPSILON);
                assert!((burst - 20.0).abs() < f64::EPSILON);
            }
            _ => panic!("expected SetThrottleRate"),
        }
    }

    #[test]
    fn roundtrip_cluster_request_remove_throttle_rate() {
        let original = ClusterRequest::RemoveThrottleRate { key: "k".into() };
        let proto = fila_proto::ClusterRequestProto::from(original);
        let roundtripped: ClusterRequest = proto.try_into().unwrap();
        assert!(matches!(roundtripped, ClusterRequest::RemoveThrottleRate { key } if key == "k"));
    }

    #[test]
    fn roundtrip_cluster_request_redrive() {
        let original = ClusterRequest::Redrive {
            dlq_queue_id: "dlq".into(),
            count: 100,
        };
        let proto = fila_proto::ClusterRequestProto::from(original);
        let roundtripped: ClusterRequest = proto.try_into().unwrap();
        assert!(
            matches!(roundtripped, ClusterRequest::Redrive { dlq_queue_id, count } if dlq_queue_id == "dlq" && count == 100)
        );
    }

    #[test]
    fn roundtrip_cluster_request_create_queue_group() {
        let original = ClusterRequest::CreateQueueGroup {
            queue_id: "q1".into(),
            members: vec![1, 2, 3],
            config: QueueConfig::new("q1".into()),
            preferred_leader: 2,
        };
        let proto = fila_proto::ClusterRequestProto::from(original);
        let roundtripped: ClusterRequest = proto.try_into().unwrap();
        match roundtripped {
            ClusterRequest::CreateQueueGroup {
                queue_id,
                members,
                config,
                preferred_leader,
            } => {
                assert_eq!(queue_id, "q1");
                assert_eq!(members, vec![1, 2, 3]);
                assert_eq!(config.name, "q1");
                assert_eq!(preferred_leader, 2);
            }
            _ => panic!("expected CreateQueueGroup"),
        }
    }

    #[test]
    fn roundtrip_cluster_request_delete_queue_group() {
        let original = ClusterRequest::DeleteQueueGroup {
            queue_id: "q1".into(),
        };
        let proto = fila_proto::ClusterRequestProto::from(original);
        let roundtripped: ClusterRequest = proto.try_into().unwrap();
        assert!(
            matches!(roundtripped, ClusterRequest::DeleteQueueGroup { queue_id } if queue_id == "q1")
        );
    }

    #[test]
    fn roundtrip_cluster_response_enqueue() {
        let id = uuid::Uuid::now_v7();
        let original = ClusterResponse::Enqueue { msg_id: id };
        let proto = fila_proto::ClusterResponseProto::from(original);
        let roundtripped: ClusterResponse = proto.try_into().unwrap();
        assert!(matches!(roundtripped, ClusterResponse::Enqueue { msg_id } if msg_id == id));
    }

    #[test]
    fn roundtrip_cluster_response_ack() {
        let original = ClusterResponse::Ack;
        let proto = fila_proto::ClusterResponseProto::from(original);
        let roundtripped: ClusterResponse = proto.try_into().unwrap();
        assert!(matches!(roundtripped, ClusterResponse::Ack));
    }

    #[test]
    fn roundtrip_cluster_response_create_queue() {
        let original = ClusterResponse::CreateQueue {
            queue_id: "q1".into(),
        };
        let proto = fila_proto::ClusterResponseProto::from(original);
        let roundtripped: ClusterResponse = proto.try_into().unwrap();
        assert!(
            matches!(roundtripped, ClusterResponse::CreateQueue { queue_id } if queue_id == "q1")
        );
    }

    #[test]
    fn roundtrip_cluster_response_redrive() {
        let original = ClusterResponse::Redrive { count: 42 };
        let proto = fila_proto::ClusterResponseProto::from(original);
        let roundtripped: ClusterResponse = proto.try_into().unwrap();
        assert!(matches!(roundtripped, ClusterResponse::Redrive { count } if count == 42));
    }

    #[test]
    fn roundtrip_cluster_response_error() {
        let original = ClusterResponse::Error {
            message: "oops".into(),
        };
        let proto = fila_proto::ClusterResponseProto::from(original);
        let roundtripped: ClusterResponse = proto.try_into().unwrap();
        assert!(matches!(roundtripped, ClusterResponse::Error { message } if message == "oops"));
    }

    #[test]
    fn roundtrip_state_machine_data() {
        let original = StateMachineData {
            last_applied_log: Some(make_log_id(5, 100)),
            last_membership: StoredMembership::new(Some(make_log_id(4, 50)), make_membership()),
            queue_groups: HashMap::from([
                ("q1".to_string(), vec![1, 2, 3]),
                ("q2".to_string(), vec![2, 3]),
            ]),
        };
        let proto = fila_proto::StateMachineDataProto::from(&original);
        let bytes = proto.encode_to_vec();
        let decoded = fila_proto::StateMachineDataProto::decode(bytes.as_slice()).unwrap();
        let roundtripped: StateMachineData = decoded.try_into().unwrap();
        assert_eq!(original.last_applied_log, roundtripped.last_applied_log);
        assert_eq!(original.last_membership, roundtripped.last_membership);
        assert_eq!(original.queue_groups, roundtripped.queue_groups);
    }

    #[test]
    fn roundtrip_message() {
        let original = make_message();
        let proto = fila_proto::Message::from(original.clone());
        let bytes = proto.encode_to_vec();
        let decoded = fila_proto::Message::decode(bytes.as_slice()).unwrap();
        let roundtripped: Message = decoded.try_into().unwrap();
        assert_eq!(original, roundtripped);
    }

    #[test]
    fn roundtrip_membership() {
        let original = make_membership();
        let proto = membership_to_proto(original.clone());
        let bytes = proto.encode_to_vec();
        let decoded = fila_proto::RaftMembership::decode(bytes.as_slice()).unwrap();
        let roundtripped = membership_from_proto(decoded);
        assert_eq!(original, roundtripped);
    }

    #[test]
    fn roundtrip_stored_membership() {
        let original = StoredMembership::new(Some(make_log_id(3, 10)), make_membership());
        let proto = stored_membership_to_proto(original.clone());
        let bytes = proto.encode_to_vec();
        let decoded = fila_proto::RaftStoredMembership::decode(bytes.as_slice()).unwrap();
        let roundtripped = stored_membership_from_proto(decoded).unwrap();
        assert_eq!(original, roundtripped);
    }

    #[test]
    fn roundtrip_snapshot_meta() {
        let original = SnapshotMeta::<NodeId, BasicNode> {
            last_log_id: Some(make_log_id(5, 100)),
            last_membership: StoredMembership::new(Some(make_log_id(4, 50)), make_membership()),
            snapshot_id: "snap-5-0-100".to_string(),
        };
        let proto = snapshot_meta_to_proto(original.clone());
        let bytes = proto.encode_to_vec();
        let decoded = fila_proto::RaftSnapshotMeta::decode(bytes.as_slice()).unwrap();
        let roundtripped = snapshot_meta_from_proto(decoded).unwrap();
        assert_eq!(original, roundtripped);
    }

    #[test]
    fn roundtrip_queue_config() {
        let original = QueueConfig {
            name: "test".into(),
            on_enqueue_script: Some("script1".into()),
            on_failure_script: Some("script2".into()),
            visibility_timeout_ms: 45_000,
            dlq_queue_id: Some("dlq-1".into()),
            lua_timeout_ms: Some(3000),
            lua_memory_limit_bytes: Some(2 * 1024 * 1024),
        };
        let proto = fila_proto::ClusterQueueConfig::from(original.clone());
        let roundtripped: QueueConfig = proto.into();
        assert_eq!(original, roundtripped);
    }
}
