//! FIBP command dispatcher — translates decoded wire payloads into
//! `SchedulerCommand` variants and returns wire-encodable results.
//!
//! The dispatcher holds an `Arc<Broker>` for sending commands.  It does
//! **not** depend on gRPC types or the cluster layer — FIBP connections
//! are anonymous (no auth) and local-only (no Raft forwarding) for now.

use std::sync::Arc;

use uuid::Uuid;

use crate::broker::command::{ReadyMessage, SchedulerCommand};
use crate::broker::Broker;
use crate::error::{AckError, EnqueueError, NackError};
use crate::message::Message;

use super::error::FibpError;
use super::wire;

/// Dispatch FIBP enqueue requests to the scheduler and return wire results.
pub async fn dispatch_enqueue(
    broker: &Arc<Broker>,
    req: wire::EnqueueRequest,
) -> Result<Vec<wire::EnqueueResultItem>, FibpError> {
    if req.queue.is_empty() {
        return Ok(vec![wire::EnqueueResultItem::Err {
            code: wire::enqueue_err::QUEUE_NOT_FOUND,
            message: "queue name must not be empty".to_string(),
        }]);
    }

    let now_ns = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos() as u64;

    let messages: Vec<Message> = req
        .messages
        .into_iter()
        .map(|m| Message {
            id: Uuid::now_v7(),
            queue_id: req.queue.clone(),
            headers: m.headers,
            payload: m.payload,
            fairness_key: "default".to_string(),
            weight: 1,
            throttle_keys: vec![],
            attempt_count: 0,
            enqueued_at: now_ns,
            leased_at: None,
        })
        .collect();

    let (reply_tx, reply_rx) = tokio::sync::oneshot::channel();
    broker
        .send_command(SchedulerCommand::Enqueue {
            messages,
            reply: reply_tx,
        })
        .map_err(FibpError::BrokerUnavailable)?;

    let scheduler_results = reply_rx.await.map_err(|_| FibpError::ReplyDropped)?;

    let results = scheduler_results
        .into_iter()
        .map(|r| match r {
            Ok(msg_id) => wire::EnqueueResultItem::Ok {
                msg_id: msg_id.to_string(),
            },
            Err(err) => match err {
                EnqueueError::QueueNotFound(msg) => wire::EnqueueResultItem::Err {
                    code: wire::enqueue_err::QUEUE_NOT_FOUND,
                    message: msg,
                },
                EnqueueError::Storage(e) => wire::EnqueueResultItem::Err {
                    code: wire::enqueue_err::STORAGE,
                    message: e.to_string(),
                },
            },
        })
        .collect();

    Ok(results)
}

/// Register a consumer for the given queue via the scheduler.
///
/// Returns a `(consumer_id, ready_rx)` pair. The caller is responsible for
/// reading from `ready_rx` and encoding push frames.
pub fn register_consumer(
    broker: &Arc<Broker>,
    queue: &str,
) -> Result<(String, tokio::sync::mpsc::Receiver<ReadyMessage>), FibpError> {
    let consumer_id = Uuid::now_v7().to_string();
    let (ready_tx, ready_rx) = tokio::sync::mpsc::channel::<ReadyMessage>(64);

    broker
        .send_command(SchedulerCommand::RegisterConsumer {
            queue_id: queue.to_string(),
            consumer_id: consumer_id.clone(),
            tx: ready_tx,
        })
        .map_err(FibpError::BrokerUnavailable)?;

    Ok((consumer_id, ready_rx))
}

/// Unregister a consumer from the scheduler.
pub fn unregister_consumer(broker: &Arc<Broker>, consumer_id: &str) {
    let _ = broker.send_command(SchedulerCommand::UnregisterConsumer {
        consumer_id: consumer_id.to_string(),
    });
}

/// Dispatch FIBP ack requests to the scheduler and return wire results.
pub async fn dispatch_ack(
    broker: &Arc<Broker>,
    items: Vec<wire::AckItem>,
) -> Result<Vec<wire::AckNackResultItem>, FibpError> {
    let mut results = Vec::with_capacity(items.len());

    for item in items {
        let msg_id: Uuid = match item.msg_id.parse() {
            Ok(id) => id,
            Err(_) => {
                results.push(wire::AckNackResultItem::Err {
                    code: wire::ack_nack_err::MESSAGE_NOT_FOUND,
                    message: format!("invalid message_id: {}", item.msg_id),
                });
                continue;
            }
        };

        let (reply_tx, reply_rx) = tokio::sync::oneshot::channel();
        broker
            .send_command(SchedulerCommand::Ack {
                queue_id: item.queue,
                msg_id,
                reply: reply_tx,
            })
            .map_err(FibpError::BrokerUnavailable)?;

        let outcome = reply_rx.await.map_err(|_| FibpError::ReplyDropped)?;
        results.push(match outcome {
            Ok(()) => wire::AckNackResultItem::Ok,
            Err(err) => match err {
                AckError::MessageNotFound(msg) => wire::AckNackResultItem::Err {
                    code: wire::ack_nack_err::MESSAGE_NOT_FOUND,
                    message: msg,
                },
                AckError::Storage(e) => wire::AckNackResultItem::Err {
                    code: wire::ack_nack_err::STORAGE,
                    message: e.to_string(),
                },
            },
        });
    }

    Ok(results)
}

/// Dispatch FIBP nack requests to the scheduler and return wire results.
pub async fn dispatch_nack(
    broker: &Arc<Broker>,
    items: Vec<wire::NackItem>,
) -> Result<Vec<wire::AckNackResultItem>, FibpError> {
    let mut results = Vec::with_capacity(items.len());

    for item in items {
        let msg_id: Uuid = match item.msg_id.parse() {
            Ok(id) => id,
            Err(_) => {
                results.push(wire::AckNackResultItem::Err {
                    code: wire::ack_nack_err::MESSAGE_NOT_FOUND,
                    message: format!("invalid message_id: {}", item.msg_id),
                });
                continue;
            }
        };

        let (reply_tx, reply_rx) = tokio::sync::oneshot::channel();
        broker
            .send_command(SchedulerCommand::Nack {
                queue_id: item.queue,
                msg_id,
                error: item.error,
                reply: reply_tx,
            })
            .map_err(FibpError::BrokerUnavailable)?;

        let outcome = reply_rx.await.map_err(|_| FibpError::ReplyDropped)?;
        results.push(match outcome {
            Ok(()) => wire::AckNackResultItem::Ok,
            Err(err) => match err {
                NackError::MessageNotFound(msg) => wire::AckNackResultItem::Err {
                    code: wire::ack_nack_err::MESSAGE_NOT_FOUND,
                    message: msg,
                },
                NackError::Storage(e) => wire::AckNackResultItem::Err {
                    code: wire::ack_nack_err::STORAGE,
                    message: e.to_string(),
                },
            },
        });
    }

    Ok(results)
}
