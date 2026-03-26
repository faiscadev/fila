//! FIBP command dispatcher — translates decoded wire payloads into
//! `SchedulerCommand` variants and returns wire-encodable results.
//!
//! The dispatcher holds an `Arc<Broker>` for sending commands.  It does
//! **not** depend on gRPC types or the cluster layer — FIBP connections
//! are local-only (no Raft forwarding) for now.
//!
//! Data operations use custom binary wire encoding (see `wire` module).
//! Admin operations use protobuf encoding (reusing proto message types
//! from `fila_proto`) for schema evolution flexibility.

use std::sync::Arc;

use bytes::Bytes;
use prost::Message as ProstMessage;
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

// ---------------------------------------------------------------------------
// Admin operations — protobuf-encoded payloads
// ---------------------------------------------------------------------------

/// Dispatch a CreateQueue request. Payload is a protobuf `CreateQueueRequest`.
pub async fn dispatch_create_queue(
    broker: &Arc<Broker>,
    payload: Bytes,
) -> Result<Bytes, FibpError> {
    let req = fila_proto::CreateQueueRequest::decode(payload)?;

    if req.name.is_empty() {
        return Err(FibpError::InvalidPayload {
            reason: "queue name must not be empty".into(),
        });
    }

    let proto_config = req.config.unwrap_or_default();
    let visibility_timeout_ms = match proto_config.visibility_timeout_ms {
        0 => crate::queue::QueueConfig::DEFAULT_VISIBILITY_TIMEOUT_MS,
        v if v < 1_000 => {
            return Err(FibpError::InvalidPayload {
                reason: "visibility_timeout_ms must be at least 1000 (1 second)".into(),
            });
        }
        v => v,
    };

    let config = crate::queue::QueueConfig {
        name: req.name.clone(),
        on_enqueue_script: if proto_config.on_enqueue_script.is_empty() {
            None
        } else {
            Some(proto_config.on_enqueue_script)
        },
        on_failure_script: if proto_config.on_failure_script.is_empty() {
            None
        } else {
            Some(proto_config.on_failure_script)
        },
        visibility_timeout_ms,
        dlq_queue_id: None,
        lua_timeout_ms: None,
        lua_memory_limit_bytes: None,
    };

    let (reply_tx, reply_rx) = tokio::sync::oneshot::channel();
    broker
        .send_command(SchedulerCommand::CreateQueue {
            name: req.name,
            config,
            reply: reply_tx,
        })
        .map_err(FibpError::BrokerUnavailable)?;

    let queue_id = reply_rx
        .await
        .map_err(|_| FibpError::ReplyDropped)?
        .map_err(|e| FibpError::InvalidPayload {
            reason: e.to_string(),
        })?;

    let resp = fila_proto::CreateQueueResponse { queue_id };
    Ok(Bytes::from(resp.encode_to_vec()))
}

/// Dispatch a DeleteQueue request. Payload is a protobuf `DeleteQueueRequest`.
pub async fn dispatch_delete_queue(
    broker: &Arc<Broker>,
    payload: Bytes,
) -> Result<Bytes, FibpError> {
    let req = fila_proto::DeleteQueueRequest::decode(payload)?;

    if req.queue.is_empty() {
        return Err(FibpError::InvalidPayload {
            reason: "queue name must not be empty".into(),
        });
    }

    let (reply_tx, reply_rx) = tokio::sync::oneshot::channel();
    broker
        .send_command(SchedulerCommand::DeleteQueue {
            queue_id: req.queue,
            reply: reply_tx,
        })
        .map_err(FibpError::BrokerUnavailable)?;

    reply_rx
        .await
        .map_err(|_| FibpError::ReplyDropped)?
        .map_err(|e| FibpError::InvalidPayload {
            reason: e.to_string(),
        })?;

    let resp = fila_proto::DeleteQueueResponse {};
    Ok(Bytes::from(resp.encode_to_vec()))
}

/// Dispatch a GetStats request. Payload is a protobuf `GetStatsRequest`.
pub async fn dispatch_queue_stats(
    broker: &Arc<Broker>,
    payload: Bytes,
) -> Result<Bytes, FibpError> {
    let req = fila_proto::GetStatsRequest::decode(payload)?;

    if req.queue.is_empty() {
        return Err(FibpError::InvalidPayload {
            reason: "queue name must not be empty".into(),
        });
    }

    let (reply_tx, reply_rx) = tokio::sync::oneshot::channel();
    broker
        .send_command(SchedulerCommand::GetStats {
            queue_id: req.queue,
            reply: reply_tx,
        })
        .map_err(FibpError::BrokerUnavailable)?;

    let stats = reply_rx
        .await
        .map_err(|_| FibpError::ReplyDropped)?
        .map_err(|e| FibpError::InvalidPayload {
            reason: e.to_string(),
        })?;

    let per_key_stats = stats
        .per_key_stats
        .into_iter()
        .map(|s| fila_proto::PerFairnessKeyStats {
            key: s.key,
            pending_count: s.pending_count,
            current_deficit: s.current_deficit,
            weight: s.weight,
        })
        .collect();

    let per_throttle_stats = stats
        .per_throttle_stats
        .into_iter()
        .map(|s| fila_proto::PerThrottleKeyStats {
            key: s.key,
            tokens: s.tokens,
            rate_per_second: s.rate_per_second,
            burst: s.burst,
        })
        .collect();

    let resp = fila_proto::GetStatsResponse {
        depth: stats.depth,
        in_flight: stats.in_flight,
        active_fairness_keys: stats.active_fairness_keys,
        active_consumers: stats.active_consumers,
        quantum: stats.quantum,
        per_key_stats,
        per_throttle_stats,
        leader_node_id: 0,
        replication_count: 0,
    };
    Ok(Bytes::from(resp.encode_to_vec()))
}

/// Dispatch a ListQueues request. Payload is a protobuf `ListQueuesRequest`.
pub async fn dispatch_list_queues(
    broker: &Arc<Broker>,
    payload: Bytes,
) -> Result<Bytes, FibpError> {
    // Decode for forward compatibility even though the message is currently empty.
    let _req = fila_proto::ListQueuesRequest::decode(payload)?;

    let (reply_tx, reply_rx) = tokio::sync::oneshot::channel();
    broker
        .send_command(SchedulerCommand::ListQueues { reply: reply_tx })
        .map_err(FibpError::BrokerUnavailable)?;

    let summaries = reply_rx
        .await
        .map_err(|_| FibpError::ReplyDropped)?
        .map_err(|e| FibpError::InvalidPayload {
            reason: e.to_string(),
        })?;

    let queues = summaries
        .into_iter()
        .map(|s| fila_proto::QueueInfo {
            name: s.name,
            depth: s.depth,
            in_flight: s.in_flight,
            active_consumers: s.active_consumers,
            leader_node_id: 0,
        })
        .collect();

    let resp = fila_proto::ListQueuesResponse {
        queues,
        cluster_node_count: 0,
    };
    Ok(Bytes::from(resp.encode_to_vec()))
}

/// Dispatch a Redrive request. Payload is a protobuf `RedriveRequest`.
pub async fn dispatch_redrive(broker: &Arc<Broker>, payload: Bytes) -> Result<Bytes, FibpError> {
    let req = fila_proto::RedriveRequest::decode(payload)?;

    if req.dlq_queue.is_empty() {
        return Err(FibpError::InvalidPayload {
            reason: "dlq_queue name must not be empty".into(),
        });
    }

    let (reply_tx, reply_rx) = tokio::sync::oneshot::channel();
    broker
        .send_command(SchedulerCommand::Redrive {
            dlq_queue_id: req.dlq_queue,
            count: req.count,
            reply: reply_tx,
        })
        .map_err(FibpError::BrokerUnavailable)?;

    let redriven = reply_rx
        .await
        .map_err(|_| FibpError::ReplyDropped)?
        .map_err(|e| FibpError::InvalidPayload {
            reason: e.to_string(),
        })?;

    let resp = fila_proto::RedriveResponse { redriven };
    Ok(Bytes::from(resp.encode_to_vec()))
}
