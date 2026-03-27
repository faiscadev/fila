//! FIBP command dispatcher -- translates decoded wire payloads into
//! `SchedulerCommand` variants and returns wire-encodable results.
//!
//! The dispatcher holds an `Arc<Broker>` for sending commands. When a
//! `ClusterHandle` is available (cluster mode), admin operations that
//! mutate cluster state (create/delete queue) are routed through the
//! meta Raft for consensus, and read operations (stats, list queues)
//! are enriched with cluster metadata (leader node ID, replication count).
//!
//! All operations (data and admin) use the shared binary wire encoding
//! from the `fila_fibp` crate.

use std::sync::Arc;

use bytes::Bytes;
use tracing::warn;
use uuid::Uuid;

use fila_fibp::wire;

use crate::broker::auth::CallerKey;
use crate::broker::command::{ReadyMessage, SchedulerCommand};
use crate::broker::Broker;
use crate::cluster::{ClusterHandle, ClusterRequest, ClusterResponse, ClusterWriteError};
use crate::error::{AckError, ConfigError, EnqueueError, NackError};
use crate::message::Message;

use super::error::FibpError;

/// Dispatch FIBP enqueue requests to the scheduler and return wire results.
///
/// In single-node mode, messages are sent directly to the local scheduler.
/// In cluster mode, messages are submitted through the queue's Raft group
/// via `ClusterHandle::write_to_queue` for replication. When this node is
/// the leader, the scheduler is also updated so in-memory state (depth, DRR,
/// pending index) stays current.
pub async fn dispatch_enqueue(
    broker: &Arc<Broker>,
    cluster: Option<&Arc<ClusterHandle>>,
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

    if let Some(ch) = cluster {
        let mut results = Vec::with_capacity(messages.len());
        for message in messages {
            let msg_id = message.id;
            let queue_id = message.queue_id.clone();
            match ch
                .write_to_queue(
                    &queue_id,
                    ClusterRequest::Enqueue {
                        message: message.clone(),
                    },
                )
                .await
            {
                Ok(write_result) => {
                    if write_result.handled_locally {
                        // The Raft write succeeded — the message is durably committed.
                        // Apply to the local scheduler for in-memory state consistency
                        // (depth, DRR, pending index).  If the scheduler apply fails
                        // we log a warning but still return success to the client:
                        // returning an error here would cause the client to retry,
                        // producing a duplicate message that IS already committed.
                        let (reply_tx, reply_rx) = tokio::sync::oneshot::channel();
                        if let Err(e) = broker.send_command(SchedulerCommand::Enqueue {
                            messages: vec![message],
                            reply: reply_tx,
                        }) {
                            warn!(
                                msg_id = %msg_id,
                                error = %e,
                                "raft write succeeded but local scheduler apply failed (send); \
                                 returning success to prevent duplicate on client retry"
                            );
                            results.push(wire::EnqueueResultItem::Ok {
                                msg_id: msg_id.to_string(),
                            });
                            continue;
                        }
                        match reply_rx.await {
                            Ok(scheduler_results) => {
                                for r in scheduler_results {
                                    results.push(match r {
                                        Ok(mid) => wire::EnqueueResultItem::Ok {
                                            msg_id: mid.to_string(),
                                        },
                                        Err(err) => {
                                            // Raft write succeeded; the scheduler apply
                                            // error means local state is temporarily
                                            // inconsistent but the message is committed.
                                            // Log and return success.
                                            warn!(
                                                msg_id = %msg_id,
                                                error = ?err,
                                                "raft write succeeded but local scheduler \
                                                 apply returned an error; returning success \
                                                 to prevent duplicate on client retry"
                                            );
                                            wire::EnqueueResultItem::Ok {
                                                msg_id: msg_id.to_string(),
                                            }
                                        }
                                    });
                                }
                            }
                            Err(_) => {
                                results.push(wire::EnqueueResultItem::Ok {
                                    msg_id: msg_id.to_string(),
                                });
                            }
                        }
                    } else {
                        match write_result.response {
                            ClusterResponse::Enqueue { msg_id } => {
                                results.push(wire::EnqueueResultItem::Ok {
                                    msg_id: msg_id.to_string(),
                                });
                            }
                            ClusterResponse::Error { message } => {
                                results.push(wire::EnqueueResultItem::Err {
                                    code: wire::enqueue_err::STORAGE,
                                    message,
                                });
                            }
                            _ => {
                                results.push(wire::EnqueueResultItem::Ok {
                                    msg_id: msg_id.to_string(),
                                });
                            }
                        }
                    }
                }
                Err(e) => {
                    results.push(wire::EnqueueResultItem::Err {
                        code: wire::enqueue_err::STORAGE,
                        message: e.to_string(),
                    });
                }
            }
        }
        Ok(results)
    } else {
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
}

/// Check whether this node is the leader for a queue in cluster mode.
pub async fn check_queue_leadership(
    cluster: Option<&Arc<ClusterHandle>>,
    queue: &str,
) -> Result<Option<String>, FibpError> {
    let ch = match cluster {
        Some(ch) => ch,
        None => return Ok(None),
    };
    match ch.is_queue_leader(queue).await {
        Some(true) => Ok(None),
        Some(false) => match ch.get_queue_leader_client_addr(queue).await {
            Some(addr) => Ok(Some(addr)),
            None => Err(FibpError::InvalidPayload {
                reason: "no leader available for queue".into(),
            }),
        },
        None => {
            if ch.multi_raft.is_queue_expected(queue).await {
                Err(FibpError::InvalidPayload {
                    reason: "node not ready for this queue".into(),
                })
            } else {
                Err(FibpError::InvalidPayload {
                    reason: "queue not found".into(),
                })
            }
        }
    }
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
    cluster: Option<&Arc<ClusterHandle>>,
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

        if let Some(ch) = cluster {
            match ch
                .write_to_queue(
                    &item.queue,
                    ClusterRequest::Ack {
                        queue_id: item.queue.clone(),
                        msg_id,
                    },
                )
                .await
            {
                Ok(write_result) => {
                    if write_result.handled_locally {
                        let (reply_tx, reply_rx) = tokio::sync::oneshot::channel();
                        if let Err(e) = broker.send_command(SchedulerCommand::Ack {
                            queue_id: item.queue,
                            msg_id,
                            reply: reply_tx,
                        }) {
                            results.push(wire::AckNackResultItem::Err {
                                code: wire::ack_nack_err::STORAGE,
                                message: e.to_string(),
                            });
                            continue;
                        }
                        match reply_rx.await {
                            Ok(outcome) => results.push(match outcome {
                                Ok(()) => wire::AckNackResultItem::Ok,
                                Err(err) => match err {
                                    AckError::MessageNotFound(msg) => {
                                        wire::AckNackResultItem::Err {
                                            code: wire::ack_nack_err::MESSAGE_NOT_FOUND,
                                            message: msg,
                                        }
                                    }
                                    AckError::Storage(e) => wire::AckNackResultItem::Err {
                                        code: wire::ack_nack_err::STORAGE,
                                        message: e.to_string(),
                                    },
                                },
                            }),
                            Err(_) => {
                                results.push(wire::AckNackResultItem::Ok);
                            }
                        }
                    } else {
                        match write_result.response {
                            ClusterResponse::Ack => {
                                results.push(wire::AckNackResultItem::Ok);
                            }
                            ClusterResponse::Error { message } => {
                                results.push(wire::AckNackResultItem::Err {
                                    code: wire::ack_nack_err::STORAGE,
                                    message,
                                });
                            }
                            _ => {
                                results.push(wire::AckNackResultItem::Ok);
                            }
                        }
                    }
                }
                Err(e) => {
                    results.push(wire::AckNackResultItem::Err {
                        code: wire::ack_nack_err::STORAGE,
                        message: e.to_string(),
                    });
                }
            }
        } else {
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
    }

    Ok(results)
}

/// Dispatch FIBP nack requests to the scheduler and return wire results.
pub async fn dispatch_nack(
    broker: &Arc<Broker>,
    cluster: Option<&Arc<ClusterHandle>>,
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

        if let Some(ch) = cluster {
            match ch
                .write_to_queue(
                    &item.queue,
                    ClusterRequest::Nack {
                        queue_id: item.queue.clone(),
                        msg_id,
                        error: item.error.clone(),
                    },
                )
                .await
            {
                Ok(write_result) => {
                    if write_result.handled_locally {
                        let (reply_tx, reply_rx) = tokio::sync::oneshot::channel();
                        if let Err(e) = broker.send_command(SchedulerCommand::Nack {
                            queue_id: item.queue,
                            msg_id,
                            error: item.error,
                            reply: reply_tx,
                        }) {
                            results.push(wire::AckNackResultItem::Err {
                                code: wire::ack_nack_err::STORAGE,
                                message: e.to_string(),
                            });
                            continue;
                        }
                        match reply_rx.await {
                            Ok(outcome) => results.push(match outcome {
                                Ok(()) => wire::AckNackResultItem::Ok,
                                Err(err) => match err {
                                    NackError::MessageNotFound(msg) => {
                                        wire::AckNackResultItem::Err {
                                            code: wire::ack_nack_err::MESSAGE_NOT_FOUND,
                                            message: msg,
                                        }
                                    }
                                    NackError::Storage(e) => wire::AckNackResultItem::Err {
                                        code: wire::ack_nack_err::STORAGE,
                                        message: e.to_string(),
                                    },
                                },
                            }),
                            Err(_) => {
                                results.push(wire::AckNackResultItem::Ok);
                            }
                        }
                    } else {
                        match write_result.response {
                            ClusterResponse::Nack => {
                                results.push(wire::AckNackResultItem::Ok);
                            }
                            ClusterResponse::Error { message } => {
                                results.push(wire::AckNackResultItem::Err {
                                    code: wire::ack_nack_err::STORAGE,
                                    message,
                                });
                            }
                            _ => {
                                results.push(wire::AckNackResultItem::Ok);
                            }
                        }
                    }
                }
                Err(e) => {
                    results.push(wire::AckNackResultItem::Err {
                        code: wire::ack_nack_err::STORAGE,
                        message: e.to_string(),
                    });
                }
            }
        } else {
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
    }

    Ok(results)
}

// ---------------------------------------------------------------------------
// Admin operations — protobuf-encoded payloads
// ---------------------------------------------------------------------------

/// Check that `caller` has admin permission covering the given queue.
/// When auth is disabled, `caller` is `None` and the check passes.
fn check_queue_admin(
    broker: &Arc<Broker>,
    caller: &Option<CallerKey>,
    queue: &str,
) -> Result<(), FibpError> {
    if let Some(ref caller) = caller {
        let permitted = broker
            .caller_has_queue_admin(caller, queue)
            .map_err(FibpError::Storage)?;
        if !permitted {
            return Err(FibpError::PermissionDenied {
                reason: format!("key does not have admin permission on queue \"{queue}\""),
            });
        }
    }
    Ok(())
}

/// Convert a `ClusterWriteError` to a `FibpError`.
fn cluster_write_err_to_fibp(e: ClusterWriteError) -> FibpError {
    match e {
        ClusterWriteError::QueueGroupNotFound => FibpError::InvalidPayload {
            reason: "queue raft group not found".into(),
        },
        ClusterWriteError::NodeNotReady => FibpError::InvalidPayload {
            reason: "node not ready for this queue".into(),
        },
        ClusterWriteError::NoLeader => FibpError::InvalidPayload {
            reason: "no leader available".into(),
        },
        ClusterWriteError::Raft(msg) => FibpError::InvalidPayload {
            reason: format!("raft error: {msg}"),
        },
        ClusterWriteError::Forward(msg) => FibpError::InvalidPayload {
            reason: format!("forward error: {msg}"),
        },
    }
}

/// Count current queue leaderships per node across all queue Raft groups.
async fn count_leaderships(
    cluster: &ClusterHandle,
    candidates: &[u64],
) -> std::collections::HashMap<u64, usize> {
    let groups = cluster.multi_raft.snapshot_groups().await;

    let mut counts: std::collections::HashMap<u64, usize> =
        candidates.iter().map(|&id| (id, 0)).collect();

    for (_queue_id, raft) in &groups {
        if let Some(leader) = raft.current_leader().await {
            if let Some(count) = counts.get_mut(&leader) {
                *count += 1;
            }
        }
    }

    counts
}

/// Select which nodes should participate in a new queue's Raft group,
/// and which node should be the preferred leader.
///
/// When the cluster has more nodes than `replication_factor`, only the
/// N least-loaded nodes are selected. The preferred leader is the node
/// with the fewest leaderships among the selected members.
async fn select_members_and_leader(
    all_members: &[u64],
    cluster: &ClusterHandle,
) -> (Vec<u64>, u64) {
    let rf = cluster.replication_factor;
    let counts = count_leaderships(cluster, all_members).await;

    let selected: Vec<u64> = if all_members.len() > rf && rf > 0 {
        let mut sorted: Vec<u64> = all_members.to_vec();
        sorted.sort_by_key(|&id| {
            let count = counts.get(&id).copied().unwrap_or(0);
            (count, id)
        });
        sorted.truncate(rf);
        sorted
    } else {
        all_members.to_vec()
    };

    let preferred = selected
        .iter()
        .copied()
        .min_by_key(|&id| {
            let count = counts.get(&id).copied().unwrap_or(0);
            (count, id)
        })
        .unwrap_or_else(|| selected.first().copied().unwrap_or(1));

    (selected, preferred)
}

/// Dispatch a CreateQueue request. Payload is a binary `CreateQueueRequest`.
pub async fn dispatch_create_queue(
    broker: &Arc<Broker>,
    cluster: Option<&Arc<ClusterHandle>>,
    caller: &Option<CallerKey>,
    payload: Bytes,
) -> Result<Bytes, FibpError> {
    let req = wire::decode_create_queue_request(payload)?;

    if req.name.is_empty() {
        return Err(FibpError::InvalidPayload {
            reason: "queue name must not be empty".into(),
        });
    }

    // Check queue-scoped admin permission.
    check_queue_admin(broker, caller, &req.name)?;

    let visibility_timeout_ms = match req.visibility_timeout_ms {
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
        on_enqueue_script: if req.on_enqueue_script.is_empty() {
            None
        } else {
            Some(req.on_enqueue_script)
        },
        on_failure_script: if req.on_failure_script.is_empty() {
            None
        } else {
            Some(req.on_failure_script)
        },
        visibility_timeout_ms,
        dlq_queue_id: None,
        lua_timeout_ms: None,
        lua_memory_limit_bytes: None,
    };

    if let Some(ch) = cluster {
        // Cluster mode: submit CreateQueueGroup to meta Raft so all nodes
        // create the queue and its per-queue Raft group.
        let (all_members, _) = ch.meta_members();
        let (members, preferred_leader) = select_members_and_leader(&all_members, ch).await;

        let resp = ch
            .write_to_meta(ClusterRequest::CreateQueueGroup {
                queue_id: req.name.clone(),
                members,
                config,
                preferred_leader,
            })
            .await
            .map_err(cluster_write_err_to_fibp)?;

        match resp {
            ClusterResponse::CreateQueueGroup { queue_id } => {
                let resp = wire::CreateQueueResponse { queue_id };
                Ok(wire::encode_create_queue_response(&resp)?)
            }
            ClusterResponse::Error { message } => {
                Err(FibpError::InvalidPayload { reason: message })
            }
            _ => Err(FibpError::InvalidPayload {
                reason: "unexpected cluster response".into(),
            }),
        }
    } else {
        // Single-node mode: direct to scheduler.
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

        let resp = wire::CreateQueueResponse { queue_id };
        Ok(wire::encode_create_queue_response(&resp)?)
    }
}

/// Dispatch a DeleteQueue request. Payload is a binary `DeleteQueueRequest`.
pub async fn dispatch_delete_queue(
    broker: &Arc<Broker>,
    cluster: Option<&Arc<ClusterHandle>>,
    caller: &Option<CallerKey>,
    payload: Bytes,
) -> Result<Bytes, FibpError> {
    let req = wire::decode_delete_queue_request(payload)?;

    if req.queue.is_empty() {
        return Err(FibpError::InvalidPayload {
            reason: "queue name must not be empty".into(),
        });
    }

    // Check queue-scoped admin permission.
    check_queue_admin(broker, caller, &req.queue)?;

    if let Some(ch) = cluster {
        // Cluster mode: submit DeleteQueueGroup to meta Raft.
        let resp = ch
            .write_to_meta(ClusterRequest::DeleteQueueGroup {
                queue_id: req.queue,
            })
            .await
            .map_err(cluster_write_err_to_fibp)?;

        match resp {
            ClusterResponse::DeleteQueueGroup => Ok(Bytes::new()),
            ClusterResponse::Error { message } => {
                Err(FibpError::InvalidPayload { reason: message })
            }
            _ => Err(FibpError::InvalidPayload {
                reason: "unexpected cluster response".into(),
            }),
        }
    } else {
        // Single-node mode: direct to scheduler.
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

        Ok(Bytes::new())
    }
}

/// Dispatch a GetStats request. Payload is a binary `GetStatsRequest`.
pub async fn dispatch_queue_stats(
    broker: &Arc<Broker>,
    cluster: Option<&Arc<ClusterHandle>>,
    caller: &Option<CallerKey>,
    payload: Bytes,
) -> Result<Bytes, FibpError> {
    let req = wire::decode_get_stats_request(payload)?;

    if req.queue.is_empty() {
        return Err(FibpError::InvalidPayload {
            reason: "queue name must not be empty".into(),
        });
    }

    // Check queue-scoped admin permission.
    check_queue_admin(broker, caller, &req.queue)?;

    let (reply_tx, reply_rx) = tokio::sync::oneshot::channel();
    broker
        .send_command(SchedulerCommand::GetStats {
            queue_id: req.queue.clone(),
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
        .map(|s| wire::PerFairnessKeyStats {
            key: s.key,
            pending_count: s.pending_count,
            current_deficit: s.current_deficit,
            weight: s.weight,
        })
        .collect();

    let per_throttle_stats = stats
        .per_throttle_stats
        .into_iter()
        .map(|s| wire::PerThrottleKeyStats {
            key: s.key,
            tokens: s.tokens,
            rate_per_second: s.rate_per_second,
            burst: s.burst,
        })
        .collect();

    // Enrich with cluster metadata when running in cluster mode.
    let (leader_node_id, replication_count) = match cluster {
        Some(ch) => {
            let leader = ch.queue_leader_id(&req.queue).await;
            let replicas = ch.queue_replication_count(&req.queue).await;
            (leader, replicas)
        }
        None => (0, 0),
    };

    let resp = wire::GetStatsResponse {
        depth: stats.depth,
        in_flight: stats.in_flight,
        active_fairness_keys: stats.active_fairness_keys,
        active_consumers: stats.active_consumers,
        quantum: stats.quantum,
        per_key_stats,
        per_throttle_stats,
        leader_node_id,
        replication_count,
    };
    Ok(wire::encode_get_stats_response(&resp)?)
}

/// Dispatch a ListQueues request. Payload is binary (empty or ignored).
pub async fn dispatch_list_queues(
    broker: &Arc<Broker>,
    cluster: Option<&Arc<ClusterHandle>>,
    _payload: Bytes,
) -> Result<Bytes, FibpError> {
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

    let mut queues = Vec::with_capacity(summaries.len());
    for s in summaries {
        let leader_node_id = match cluster {
            Some(ch) => ch.queue_leader_id(&s.name).await,
            None => 0,
        };
        queues.push(wire::QueueInfo {
            name: s.name,
            depth: s.depth,
            in_flight: s.in_flight,
            active_consumers: s.active_consumers,
            leader_node_id,
        });
    }

    let cluster_node_count = match cluster {
        Some(ch) => ch.cluster_node_count(),
        None => 0,
    };

    let resp = wire::ListQueuesResponse {
        queues,
        cluster_node_count,
    };
    Ok(wire::encode_list_queues_response(&resp)?)
}

/// Dispatch a Redrive request. Payload is a binary `RedriveRequest`.
pub async fn dispatch_redrive(
    broker: &Arc<Broker>,
    caller: &Option<CallerKey>,
    payload: Bytes,
) -> Result<Bytes, FibpError> {
    let req = wire::decode_redrive_request(payload)?;

    if req.dlq_queue.is_empty() {
        return Err(FibpError::InvalidPayload {
            reason: "dlq_queue name must not be empty".into(),
        });
    }

    // Check queue-scoped admin permission.
    check_queue_admin(broker, caller, &req.dlq_queue)?;

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

    let resp = wire::RedriveResponse { redriven };
    Ok(wire::encode_redrive_response(&resp))
}

// ---------------------------------------------------------------------------
// Config operations -- binary-encoded payloads
// ---------------------------------------------------------------------------

/// Dispatch a SetConfig request. Payload is a binary `SetConfigRequest`.
pub async fn dispatch_config_set(broker: &Arc<Broker>, payload: Bytes) -> Result<Bytes, FibpError> {
    let req = wire::decode_set_config_request(payload)?;

    if req.key.is_empty() {
        return Err(FibpError::InvalidPayload {
            reason: "config key must not be empty".into(),
        });
    }

    let (reply_tx, reply_rx) = tokio::sync::oneshot::channel();
    broker
        .send_command(SchedulerCommand::SetConfig {
            key: req.key,
            value: req.value,
            reply: reply_tx,
        })
        .map_err(FibpError::BrokerUnavailable)?;

    reply_rx
        .await
        .map_err(|_| FibpError::ReplyDropped)?
        .map_err(|e| match e {
            ConfigError::InvalidValue(msg) => FibpError::InvalidPayload { reason: msg },
            ConfigError::Storage(s) => FibpError::Storage(s),
        })?;

    Ok(Bytes::new())
}

/// Dispatch a GetConfig request. Payload is a binary `GetConfigRequest`.
pub async fn dispatch_config_get(broker: &Arc<Broker>, payload: Bytes) -> Result<Bytes, FibpError> {
    let req = wire::decode_get_config_request(payload)?;

    if req.key.is_empty() {
        return Err(FibpError::InvalidPayload {
            reason: "config key must not be empty".into(),
        });
    }

    let (reply_tx, reply_rx) = tokio::sync::oneshot::channel();
    broker
        .send_command(SchedulerCommand::GetConfig {
            key: req.key,
            reply: reply_tx,
        })
        .map_err(FibpError::BrokerUnavailable)?;

    let value = reply_rx
        .await
        .map_err(|_| FibpError::ReplyDropped)?
        .map_err(|e| match e {
            ConfigError::InvalidValue(msg) => FibpError::InvalidPayload { reason: msg },
            ConfigError::Storage(s) => FibpError::Storage(s),
        })?;

    let resp = wire::GetConfigResponse {
        value: value.unwrap_or_default(),
    };
    Ok(wire::encode_get_config_response(&resp)?)
}

/// Dispatch a ListConfig request. Payload is a binary `ListConfigRequest`.
pub async fn dispatch_config_list(
    broker: &Arc<Broker>,
    payload: Bytes,
) -> Result<Bytes, FibpError> {
    let req = wire::decode_list_config_request(payload)?;

    let (reply_tx, reply_rx) = tokio::sync::oneshot::channel();
    broker
        .send_command(SchedulerCommand::ListConfig {
            prefix: req.prefix,
            reply: reply_tx,
        })
        .map_err(FibpError::BrokerUnavailable)?;

    let entries = reply_rx
        .await
        .map_err(|_| FibpError::ReplyDropped)?
        .map_err(|e| match e {
            ConfigError::InvalidValue(msg) => FibpError::InvalidPayload { reason: msg },
            ConfigError::Storage(s) => FibpError::Storage(s),
        })?;

    let wire_entries = entries
        .iter()
        .map(|(k, v)| wire::ConfigEntry {
            key: k.clone(),
            value: v.clone(),
        })
        .collect();

    let resp = wire::ListConfigResponse {
        entries: wire_entries,
        total_count: entries.len() as u32,
    };
    Ok(wire::encode_list_config_response(&resp)?)
}

// ---------------------------------------------------------------------------
// Auth CRUD operations -- binary-encoded payloads
// ---------------------------------------------------------------------------

/// Dispatch a CreateApiKey request. Payload is a binary `CreateApiKeyRequest`.
pub fn dispatch_auth_create_key(
    broker: &Arc<Broker>,
    caller: &Option<CallerKey>,
    payload: Bytes,
) -> Result<Bytes, FibpError> {
    let req = wire::decode_create_api_key_request(payload)?;

    if req.name.is_empty() {
        return Err(FibpError::InvalidPayload {
            reason: "api key name must not be empty".into(),
        });
    }

    // Only superadmin or admin:* can create keys.
    if let Some(ref caller) = caller {
        let is_admin = broker.has_any_admin(caller).map_err(FibpError::Storage)?;
        if !is_admin {
            return Err(FibpError::PermissionDenied {
                reason: "key does not have admin permission".into(),
            });
        }

        // Only superadmin keys can create other superadmin keys.
        if req.is_superadmin {
            let is_superadmin = broker.is_superadmin(caller).map_err(FibpError::Storage)?;
            if !is_superadmin {
                return Err(FibpError::PermissionDenied {
                    reason: "only superadmin keys can create superadmin keys".into(),
                });
            }
        }
    }

    let expires_at_ms = if req.expires_at_ms == 0 {
        None
    } else {
        Some(req.expires_at_ms)
    };

    let (key_id, token) = broker
        .create_api_key(&req.name, expires_at_ms, req.is_superadmin)
        .map_err(FibpError::Storage)?;

    let resp = wire::CreateApiKeyResponse {
        key_id,
        key: token,
        is_superadmin: req.is_superadmin,
    };
    Ok(wire::encode_create_api_key_response(&resp)?)
}

/// Dispatch a RevokeApiKey request. Payload is a binary `RevokeApiKeyRequest`.
pub fn dispatch_auth_revoke_key(broker: &Arc<Broker>, payload: Bytes) -> Result<Bytes, FibpError> {
    let req = wire::decode_revoke_api_key_request(payload)?;

    if req.key_id.is_empty() {
        return Err(FibpError::InvalidPayload {
            reason: "key_id must not be empty".into(),
        });
    }

    let found = broker
        .revoke_api_key(&req.key_id)
        .map_err(FibpError::Storage)?;

    if !found {
        return Err(FibpError::InvalidPayload {
            reason: format!("api key not found: {}", req.key_id),
        });
    }

    Ok(Bytes::new())
}

/// Dispatch a ListApiKeys request. Payload is binary (empty or ignored).
pub fn dispatch_auth_list_keys(broker: &Arc<Broker>, _payload: Bytes) -> Result<Bytes, FibpError> {
    let entries = broker.list_api_keys().map_err(FibpError::Storage)?;

    let keys = entries
        .into_iter()
        .map(|e| wire::ApiKeyInfo {
            key_id: e.key_id,
            name: e.name,
            created_at_ms: e.created_at_ms,
            expires_at_ms: e.expires_at_ms.unwrap_or(0),
            is_superadmin: e.is_superadmin,
        })
        .collect();

    let resp = wire::ListApiKeysResponse { keys };
    Ok(wire::encode_list_api_keys_response(&resp)?)
}

/// Dispatch a SetAcl request. Payload is a binary `SetAclRequest`.
pub fn dispatch_auth_set_acl(
    broker: &Arc<Broker>,
    caller: &Option<CallerKey>,
    payload: Bytes,
) -> Result<Bytes, FibpError> {
    let req = wire::decode_set_acl_request(payload)?;

    if req.key_id.is_empty() {
        return Err(FibpError::InvalidPayload {
            reason: "key_id must not be empty".into(),
        });
    }

    // Validate permission kinds.
    let mut permissions = Vec::with_capacity(req.permissions.len());
    for p in &req.permissions {
        match p.kind.as_str() {
            "produce" | "consume" | "admin" => {}
            other => {
                return Err(FibpError::InvalidPayload {
                    reason: format!("invalid permission kind: {other}"),
                });
            }
        }
        permissions.push((p.kind.clone(), p.pattern.clone()));
    }

    // Scope check: the caller must be able to grant all requested permissions.
    if let Some(ref caller) = caller {
        let can_grant = broker
            .caller_can_grant_all(caller, &permissions)
            .map_err(FibpError::Storage)?;
        if !can_grant {
            return Err(FibpError::PermissionDenied {
                reason: "cannot grant permissions outside your admin scope".into(),
            });
        }
    }

    let found = broker
        .set_acl(&req.key_id, permissions)
        .map_err(FibpError::Storage)?;

    if !found {
        return Err(FibpError::InvalidPayload {
            reason: format!("api key not found: {}", req.key_id),
        });
    }

    Ok(Bytes::new())
}

/// Dispatch a GetAcl request. Payload is a binary `GetAclRequest`.
pub fn dispatch_auth_get_acl(broker: &Arc<Broker>, payload: Bytes) -> Result<Bytes, FibpError> {
    let req = wire::decode_get_acl_request(payload)?;

    if req.key_id.is_empty() {
        return Err(FibpError::InvalidPayload {
            reason: "key_id must not be empty".into(),
        });
    }

    let entry = broker.get_acl(&req.key_id).map_err(FibpError::Storage)?;

    match entry {
        Some(e) => {
            let permissions = e
                .permissions
                .into_iter()
                .map(|(kind, pattern)| wire::AclPermission { kind, pattern })
                .collect();
            let resp = wire::GetAclResponse {
                key_id: e.key_id,
                permissions,
                is_superadmin: e.is_superadmin,
            };
            Ok(wire::encode_get_acl_response(&resp)?)
        }
        None => Err(FibpError::InvalidPayload {
            reason: format!("api key not found: {}", req.key_id),
        }),
    }
}
