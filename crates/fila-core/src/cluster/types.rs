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

    // --- Meta Raft group operations for multi-Raft coordination ---
    /// Create a Raft group for a queue. Applied by the meta Raft state machine;
    /// each node then starts a local Raft instance for the queue.
    CreateQueueGroup {
        queue_id: String,
        /// Node IDs that should be members of this queue's Raft group.
        members: Vec<u64>,
        /// Queue configuration so all nodes can create the queue in their
        /// local scheduler when the group is established.
        config: QueueConfig,
    },
    /// Delete a queue's Raft group. Applied by the meta Raft state machine;
    /// each node then shuts down its local Raft instance for the queue.
    DeleteQueueGroup {
        queue_id: String,
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
    CreateQueueGroup { queue_id: String },
    DeleteQueueGroup,
    Error { message: String },
}
