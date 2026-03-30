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

/// A single ack item in a batch ack cluster request.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AckItemData {
    pub queue_id: String,
    pub msg_id: uuid::Uuid,
}

/// A single nack item in a batch nack cluster request.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NackItemData {
    pub queue_id: String,
    pub msg_id: uuid::Uuid,
    pub error: String,
}

/// All state-mutating operations that flow through the Raft log.
///
/// Every write operation must be serialized into a `ClusterRequest`,
/// committed via Raft consensus, and then applied to the local state
/// machine. Read-only operations bypass Raft entirely.
///
/// Hot-path operations (Enqueue, Ack, Nack) are batch-native: they
/// carry a Vec of items. Single operations use a batch of 1.
/// New fields use `#[serde(default)]` for backward compatibility with
/// Raft log entries written before batch support was added.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum ClusterRequest {
    Enqueue {
        /// Batch of messages. Pre-batch entries serialized a single `message`
        /// field; `#[serde(default)]` ensures those deserialize as an empty vec,
        /// and the `message` field is kept for backward compat.
        #[serde(default)]
        messages: Vec<Message>,
        /// Legacy single-message field for backward compatibility with Raft log
        /// entries written before batch support.
        #[serde(default)]
        message: Option<Message>,
    },
    Ack {
        /// Batch of ack items.
        #[serde(default)]
        items: Vec<AckItemData>,
        /// Legacy single-item fields for backward compatibility.
        #[serde(default)]
        queue_id: Option<String>,
        #[serde(default)]
        msg_id: Option<uuid::Uuid>,
    },
    Nack {
        /// Batch of nack items.
        #[serde(default)]
        items: Vec<NackItemData>,
        /// Legacy single-item fields for backward compatibility.
        #[serde(default)]
        queue_id: Option<String>,
        #[serde(default)]
        msg_id: Option<uuid::Uuid>,
        #[serde(default)]
        error: Option<String>,
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
        /// Preferred initial leader for this queue's Raft group.
        /// The node with this ID bootstraps the group (becomes initial leader).
        /// Chosen by the assignment strategy to balance leadership across nodes.
        /// A value of 0 means "unset" — fall back to smallest member ID.
        /// This preserves backward compatibility with Raft log entries written
        /// before this field existed (serde defaults missing fields to 0).
        #[serde(default)]
        preferred_leader: u64,
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
