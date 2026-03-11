use std::io::Cursor;

use openraft::BasicNode;
use serde::{Deserialize, Serialize};

use crate::message::Message;
use crate::queue::QueueConfig;

pub type NodeId = u64;

openraft::declare_raft_types!(
    pub TypeConfig:
        D = ClusterRequest,
        R = ClusterResponse,
        NodeId = NodeId,
        Node = BasicNode,
        Entry = openraft::Entry<TypeConfig>,
        SnapshotData = Cursor<Vec<u8>>,
);

/// All state-mutating operations that flow through the Raft log.
///
/// Every write operation must be serialized into a `ClusterRequest`,
/// committed via Raft consensus, and then applied to the local state
/// machine. Read-only operations bypass Raft entirely.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum ClusterRequest {
    Enqueue {
        message: Message,
    },
    Ack {
        queue_id: String,
        msg_id: uuid::Uuid,
    },
    Nack {
        queue_id: String,
        msg_id: uuid::Uuid,
        error: String,
    },
    CreateQueue {
        name: String,
        config: QueueConfig,
    },
    DeleteQueue {
        queue_id: String,
    },
    SetConfig {
        key: String,
        value: String,
    },
    SetThrottleRate {
        key: String,
        rate_per_second: f64,
        burst: f64,
    },
    RemoveThrottleRate {
        key: String,
    },
    Redrive {
        dlq_queue_id: String,
        count: u64,
    },
}

/// Response from applying a committed `ClusterRequest` to the state machine.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum ClusterResponse {
    Enqueue { msg_id: uuid::Uuid },
    Ack,
    Nack,
    CreateQueue { queue_id: String },
    DeleteQueue,
    SetConfig,
    SetThrottleRate,
    RemoveThrottleRate,
    Redrive { count: u64 },
    Error { message: String },
}
