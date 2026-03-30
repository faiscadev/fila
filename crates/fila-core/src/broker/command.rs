use std::collections::HashMap;

use uuid::Uuid;

use crate::error::{
    AckError, ConfigError, CreateQueueError, DeleteQueueError, EnqueueError, ListQueuesError,
    NackError, RedriveError, StatsError,
};

/// A message ready for delivery to a consumer.
#[derive(Debug, Clone)]
pub struct ReadyMessage {
    pub msg_id: Uuid,
    pub queue_id: String,
    pub headers: HashMap<String, String>,
    pub payload: Vec<u8>,
    pub fairness_key: String,
    pub weight: u32,
    pub throttle_keys: Vec<String>,
    pub attempt_count: u32,
}

/// Summary info for a single queue, returned by ListQueues.
#[derive(Debug, Clone)]
pub struct QueueSummary {
    pub name: String,
    pub depth: u64,
    pub in_flight: u64,
    pub active_consumers: u32,
    /// Raft leader node ID for this queue (0 = not clustered).
    pub leader_node_id: u64,
}

/// A single ack item in a batch ack command.
#[derive(Debug, Clone)]
pub struct AckItem {
    pub queue_id: String,
    pub msg_id: Uuid,
}

/// A single nack item in a batch nack command.
#[derive(Debug, Clone)]
pub struct NackItem {
    pub queue_id: String,
    pub msg_id: Uuid,
    pub error: String,
}

/// Commands sent from IO threads to the single-threaded scheduler core.
///
/// All hot-path commands (Enqueue, Ack, Nack) are batch-native: they accept
/// a Vec of items and return a Vec of per-item results. Single-message
/// operations use a batch of 1.
///
/// Each variant that expects a response includes a `tokio::sync::oneshot::Sender`
/// for the reply. Fire-and-forget commands omit the reply channel.
pub enum SchedulerCommand {
    Enqueue {
        messages: Vec<crate::message::Message>,
        reply: tokio::sync::oneshot::Sender<Vec<Result<Uuid, EnqueueError>>>,
    },
    Ack {
        items: Vec<AckItem>,
        reply: tokio::sync::oneshot::Sender<Vec<Result<(), AckError>>>,
    },
    Nack {
        items: Vec<NackItem>,
        reply: tokio::sync::oneshot::Sender<Vec<Result<(), NackError>>>,
    },
    RegisterConsumer {
        queue_id: String,
        consumer_id: String,
        tx: tokio::sync::mpsc::Sender<ReadyMessage>,
    },
    UnregisterConsumer {
        consumer_id: String,
    },
    CreateQueue {
        name: String,
        config: crate::queue::QueueConfig,
        reply: tokio::sync::oneshot::Sender<Result<String, CreateQueueError>>,
    },
    DeleteQueue {
        queue_id: String,
        reply: tokio::sync::oneshot::Sender<Result<(), DeleteQueueError>>,
    },
    SetThrottleRate {
        key: String,
        rate_per_second: f64,
        burst: f64,
    },
    RemoveThrottleRate {
        key: String,
    },
    SetConfig {
        key: String,
        value: String,
        reply: tokio::sync::oneshot::Sender<Result<(), ConfigError>>,
    },
    GetConfig {
        key: String,
        reply: tokio::sync::oneshot::Sender<Result<Option<String>, ConfigError>>,
    },
    ListConfig {
        prefix: String,
        reply: tokio::sync::oneshot::Sender<Result<Vec<(String, String)>, ConfigError>>,
    },
    GetStats {
        queue_id: String,
        reply: tokio::sync::oneshot::Sender<Result<crate::broker::stats::QueueStats, StatsError>>,
    },
    Redrive {
        dlq_queue_id: String,
        count: u64,
        reply: tokio::sync::oneshot::Sender<Result<u64, RedriveError>>,
    },
    ListQueues {
        reply: tokio::sync::oneshot::Sender<Result<Vec<QueueSummary>, ListQueuesError>>,
    },
    /// Rebuild in-memory scheduler state (DRR keys, pending index) for a
    /// single queue. Used when this node becomes the Raft leader for a queue
    /// during failover — the new leader must reconstruct its scheduler state
    /// from RocksDB before it can serve consumers.
    RecoverQueue {
        queue_id: String,
        reply: Option<tokio::sync::oneshot::Sender<()>>,
    },
    /// Drop all consumer streams for a queue. Used when this node loses Raft
    /// leadership — consumers must reconnect to the new leader.
    DropQueueConsumers {
        queue_id: String,
    },
    Shutdown,
}
