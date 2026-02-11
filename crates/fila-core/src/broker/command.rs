use std::collections::HashMap;

use uuid::Uuid;

use crate::error::{AckError, EnqueueError, NackError};

/// A message ready for delivery to a consumer.
#[derive(Debug, Clone)]
pub struct ReadyMessage {
    pub msg_id: Uuid,
    pub queue_id: String,
    pub headers: HashMap<String, String>,
    pub payload: Vec<u8>,
    pub fairness_key: String,
    pub attempt_count: u32,
}

/// Commands sent from IO threads to the single-threaded scheduler core.
///
/// Each variant that expects a response includes a `tokio::sync::oneshot::Sender`
/// for the reply. Fire-and-forget commands omit the reply channel.
pub enum SchedulerCommand {
    Enqueue {
        message: crate::message::Message,
        reply: tokio::sync::oneshot::Sender<Result<Uuid, EnqueueError>>,
    },
    Ack {
        queue_id: String,
        msg_id: Uuid,
        reply: tokio::sync::oneshot::Sender<Result<(), AckError>>,
    },
    Nack {
        queue_id: String,
        msg_id: Uuid,
        error: String,
        reply: tokio::sync::oneshot::Sender<Result<(), NackError>>,
    },
    RegisterConsumer {
        queue_id: String,
        consumer_id: String,
        tx: crossbeam_channel::Sender<ReadyMessage>,
    },
    UnregisterConsumer {
        consumer_id: String,
    },
    CreateQueue {
        name: String,
        config: crate::queue::QueueConfig,
        reply: tokio::sync::oneshot::Sender<Result<String>>,
    },
    DeleteQueue {
        queue_id: String,
        reply: tokio::sync::oneshot::Sender<Result<()>>,
    },
    Shutdown,
}
