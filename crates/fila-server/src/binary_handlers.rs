//! Binary protocol operation handlers — translate FIBP typed requests into
//! `SchedulerCommand` batch variants and convert results back to FIBP responses.

use std::collections::HashMap;

use fila_core::cluster::{AckItemData, NackItemData};
use fila_core::{Broker, ClusterHandle, ClusterRequest, ClusterResponse, Message};
use fila_fibp::{
    AckResponse, AckResultItem, EnqueueResponse, EnqueueResultItem, ErrorCode, NackResponse,
    NackResultItem,
};
use uuid::Uuid;

/// Handle a batch enqueue request.
///
/// The binary protocol enqueue is batched — messages may target different
/// queues. In cluster mode each queue gets its own Raft write; results are
/// collected back into the original message order. In single-node mode the
/// entire batch goes directly to the scheduler.
pub async fn handle_enqueue(
    broker: &Broker,
    cluster: Option<&ClusterHandle>,
    req: fila_fibp::EnqueueRequest,
) -> Result<EnqueueResponse, (ErrorCode, String)> {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos() as u64;

    let messages: Vec<Message> = req
        .messages
        .into_iter()
        .map(|m| Message {
            id: Uuid::now_v7(),
            queue_id: m.queue,
            headers: m.headers,
            payload: m.payload,
            fairness_key: "default".to_string(),
            weight: 1,
            throttle_keys: vec![],
            attempt_count: 0,
            enqueued_at: now,
            leased_at: None,
        })
        .collect();

    if let Some(cluster) = cluster {
        // Cluster mode: group messages by queue, one Raft write per queue.
        // Track original index so results come back in request order.
        let mut by_queue: HashMap<String, Vec<(usize, Message)>> = HashMap::new();
        for (idx, msg) in messages.iter().enumerate() {
            by_queue
                .entry(msg.queue_id.clone())
                .or_default()
                .push((idx, msg.clone()));
        }

        let mut results: Vec<EnqueueResultItem> = vec![
            EnqueueResultItem {
                error_code: ErrorCode::Ok,
                message_id: String::new(),
            };
            messages.len()
        ];

        for (queue_id, items) in &by_queue {
            let batch_msgs: Vec<Message> = items.iter().map(|(_, m)| m.clone()).collect();

            let write_result = cluster
                .write_to_queue(
                    queue_id,
                    ClusterRequest::Enqueue {
                        messages: batch_msgs.clone(),
                    },
                )
                .await;

            match write_result {
                Ok(result) => {
                    match result.response {
                        ClusterResponse::Enqueue { msg_id } => {
                            // Raft returns a single msg_id (the first). For batches
                            // within a single queue, we use the IDs we assigned.
                            // Apply to local scheduler if handled locally.
                            if result.handled_locally {
                                let (reply_tx, reply_rx) = tokio::sync::oneshot::channel();
                                broker
                                    .send_command(fila_core::SchedulerCommand::Enqueue {
                                        messages: batch_msgs,
                                        reply: reply_tx,
                                    })
                                    .map_err(broker_error_to_pair)?;

                                let sched_results = reply_rx.await.map_err(|_| {
                                    (
                                        ErrorCode::InternalError,
                                        "scheduler reply dropped".to_string(),
                                    )
                                })?;

                                for ((orig_idx, _), sched_result) in
                                    items.iter().zip(sched_results.into_iter())
                                {
                                    results[*orig_idx] = match sched_result {
                                        Ok(id) => EnqueueResultItem {
                                            error_code: ErrorCode::Ok,
                                            message_id: id.to_string(),
                                        },
                                        Err(fila_core::EnqueueError::QueueNotFound(_)) => {
                                            EnqueueResultItem {
                                                error_code: ErrorCode::QueueNotFound,
                                                message_id: String::new(),
                                            }
                                        }
                                        Err(fila_core::EnqueueError::Storage(_)) => {
                                            EnqueueResultItem {
                                                error_code: ErrorCode::StorageError,
                                                message_id: String::new(),
                                            }
                                        }
                                    };
                                }
                            } else {
                                // Forwarded — leader applied. Use the IDs we assigned.
                                // For a batch of 1, Raft returns the msg_id directly.
                                // For larger batches, use the pre-assigned UUIDs.
                                if items.len() == 1 {
                                    results[items[0].0] = EnqueueResultItem {
                                        error_code: ErrorCode::Ok,
                                        message_id: msg_id.to_string(),
                                    };
                                } else {
                                    for (orig_idx, msg) in items {
                                        results[*orig_idx] = EnqueueResultItem {
                                            error_code: ErrorCode::Ok,
                                            message_id: msg.id.to_string(),
                                        };
                                    }
                                }
                            }
                        }
                        ClusterResponse::Error { message: _ } => {
                            for (orig_idx, _) in items {
                                results[*orig_idx] = EnqueueResultItem {
                                    error_code: ErrorCode::InternalError,
                                    message_id: String::new(),
                                };
                            }
                        }
                        _ => {
                            for (orig_idx, _) in items {
                                results[*orig_idx] = EnqueueResultItem {
                                    error_code: ErrorCode::InternalError,
                                    message_id: String::new(),
                                };
                            }
                        }
                    }
                }
                Err(e) => {
                    let (code, msg) = cluster_write_err_to_pair(e);
                    for (orig_idx, _) in items {
                        results[*orig_idx] = EnqueueResultItem {
                            error_code: code,
                            message_id: msg.clone(),
                        };
                    }
                }
            }
        }

        Ok(EnqueueResponse { results })
    } else {
        // Single-node mode: direct to scheduler.
        let (reply_tx, reply_rx) = tokio::sync::oneshot::channel();
        broker
            .send_command(fila_core::SchedulerCommand::Enqueue {
                messages,
                reply: reply_tx,
            })
            .map_err(broker_error_to_pair)?;

        let results = reply_rx.await.map_err(|_| {
            (
                ErrorCode::InternalError,
                "scheduler reply dropped".to_string(),
            )
        })?;

        let items: Vec<EnqueueResultItem> = results
            .into_iter()
            .map(|r| match r {
                Ok(id) => EnqueueResultItem {
                    error_code: ErrorCode::Ok,
                    message_id: id.to_string(),
                },
                Err(fila_core::EnqueueError::QueueNotFound(_)) => EnqueueResultItem {
                    error_code: ErrorCode::QueueNotFound,
                    message_id: String::new(),
                },
                Err(fila_core::EnqueueError::Storage(_)) => EnqueueResultItem {
                    error_code: ErrorCode::StorageError,
                    message_id: String::new(),
                },
            })
            .collect();

        Ok(EnqueueResponse { results: items })
    }
}

/// Handle a batch ack request.
///
/// In cluster mode, items are grouped by queue and each queue gets its own
/// Raft write. Results are collected back into original request order.
pub async fn handle_ack(
    broker: &Broker,
    cluster: Option<&ClusterHandle>,
    req: fila_fibp::AckRequest,
) -> Result<AckResponse, (ErrorCode, String)> {
    let items: Vec<(String, Uuid)> = req
        .items
        .into_iter()
        .map(|item| {
            let msg_id = item.message_id.parse::<Uuid>().unwrap_or(Uuid::nil());
            (item.queue, msg_id)
        })
        .collect();

    if let Some(cluster) = cluster {
        // Cluster mode: group by queue, one Raft write per queue.
        let mut by_queue: HashMap<String, Vec<(usize, Uuid)>> = HashMap::new();
        for (idx, (queue_id, msg_id)) in items.iter().enumerate() {
            by_queue
                .entry(queue_id.clone())
                .or_default()
                .push((idx, *msg_id));
        }

        let mut results: Vec<AckResultItem> = vec![
            AckResultItem {
                error_code: ErrorCode::Ok,
            };
            items.len()
        ];

        for (queue_id, queue_items) in &by_queue {
            let cluster_items: Vec<AckItemData> = queue_items
                .iter()
                .map(|(_, msg_id)| AckItemData {
                    queue_id: queue_id.clone(),
                    msg_id: *msg_id,
                })
                .collect();

            let write_result = cluster
                .write_to_queue(
                    queue_id,
                    ClusterRequest::Ack {
                        items: cluster_items,
                    },
                )
                .await;

            match write_result {
                Ok(result) => {
                    if let ClusterResponse::Error { message } = result.response {
                        for (orig_idx, _) in queue_items {
                            results[*orig_idx] = AckResultItem {
                                error_code: ErrorCode::InternalError,
                            };
                        }
                        tracing::warn!(queue = %queue_id, %message, "cluster ack error");
                        continue;
                    }

                    if result.handled_locally {
                        let sched_items: Vec<fila_core::broker::command::AckItem> = queue_items
                            .iter()
                            .map(|(_, msg_id)| fila_core::broker::command::AckItem {
                                queue_id: queue_id.clone(),
                                msg_id: *msg_id,
                            })
                            .collect();

                        let (reply_tx, reply_rx) = tokio::sync::oneshot::channel();
                        broker
                            .send_command(fila_core::SchedulerCommand::Ack {
                                items: sched_items,
                                reply: reply_tx,
                            })
                            .map_err(broker_error_to_pair)?;

                        let sched_results = reply_rx.await.map_err(|_| {
                            (
                                ErrorCode::InternalError,
                                "scheduler reply dropped".to_string(),
                            )
                        })?;

                        for ((orig_idx, _), sched_result) in
                            queue_items.iter().zip(sched_results.into_iter())
                        {
                            results[*orig_idx] = match sched_result {
                                Ok(()) => AckResultItem {
                                    error_code: ErrorCode::Ok,
                                },
                                Err(fila_core::AckError::MessageNotFound(_)) => AckResultItem {
                                    error_code: ErrorCode::MessageNotFound,
                                },
                                Err(fila_core::AckError::Storage(_)) => AckResultItem {
                                    error_code: ErrorCode::StorageError,
                                },
                            };
                        }
                    }
                    // If forwarded, the leader already applied — results are Ok.
                }
                Err(e) => {
                    let (code, _msg) = cluster_write_err_to_pair(e);
                    for (orig_idx, _) in queue_items {
                        results[*orig_idx] = AckResultItem { error_code: code };
                    }
                }
            }
        }

        Ok(AckResponse { results })
    } else {
        // Single-node mode: direct to scheduler.
        let sched_items: Vec<fila_core::broker::command::AckItem> = items
            .into_iter()
            .map(|(queue_id, msg_id)| fila_core::broker::command::AckItem { queue_id, msg_id })
            .collect();

        let (reply_tx, reply_rx) = tokio::sync::oneshot::channel();
        broker
            .send_command(fila_core::SchedulerCommand::Ack {
                items: sched_items,
                reply: reply_tx,
            })
            .map_err(broker_error_to_pair)?;

        let results = reply_rx.await.map_err(|_| {
            (
                ErrorCode::InternalError,
                "scheduler reply dropped".to_string(),
            )
        })?;

        let items: Vec<AckResultItem> = results
            .into_iter()
            .map(|r| match r {
                Ok(()) => AckResultItem {
                    error_code: ErrorCode::Ok,
                },
                Err(fila_core::AckError::MessageNotFound(_)) => AckResultItem {
                    error_code: ErrorCode::MessageNotFound,
                },
                Err(fila_core::AckError::Storage(_)) => AckResultItem {
                    error_code: ErrorCode::StorageError,
                },
            })
            .collect();

        Ok(AckResponse { results: items })
    }
}

/// Handle a batch nack request.
///
/// In cluster mode, items are grouped by queue and each queue gets its own
/// Raft write. Results are collected back into original request order.
pub async fn handle_nack(
    broker: &Broker,
    cluster: Option<&ClusterHandle>,
    req: fila_fibp::NackRequest,
) -> Result<NackResponse, (ErrorCode, String)> {
    let items: Vec<(String, Uuid, String)> = req
        .items
        .into_iter()
        .map(|item| {
            let msg_id = item.message_id.parse::<Uuid>().unwrap_or(Uuid::nil());
            (item.queue, msg_id, item.error)
        })
        .collect();

    if let Some(cluster) = cluster {
        // Cluster mode: group by queue, one Raft write per queue.
        let mut by_queue: HashMap<String, Vec<(usize, Uuid, String)>> = HashMap::new();
        for (idx, (queue_id, msg_id, error)) in items.iter().enumerate() {
            by_queue
                .entry(queue_id.clone())
                .or_default()
                .push((idx, *msg_id, error.clone()));
        }

        let mut results: Vec<NackResultItem> = vec![
            NackResultItem {
                error_code: ErrorCode::Ok,
            };
            items.len()
        ];

        for (queue_id, queue_items) in &by_queue {
            let cluster_items: Vec<NackItemData> = queue_items
                .iter()
                .map(|(_, msg_id, error)| NackItemData {
                    queue_id: queue_id.clone(),
                    msg_id: *msg_id,
                    error: error.clone(),
                })
                .collect();

            let write_result = cluster
                .write_to_queue(
                    queue_id,
                    ClusterRequest::Nack {
                        items: cluster_items,
                    },
                )
                .await;

            match write_result {
                Ok(result) => {
                    if let ClusterResponse::Error { message } = result.response {
                        for (orig_idx, _, _) in queue_items {
                            results[*orig_idx] = NackResultItem {
                                error_code: ErrorCode::InternalError,
                            };
                        }
                        tracing::warn!(queue = %queue_id, %message, "cluster nack error");
                        continue;
                    }

                    if result.handled_locally {
                        let sched_items: Vec<fila_core::broker::command::NackItem> = queue_items
                            .iter()
                            .map(|(_, msg_id, error)| fila_core::broker::command::NackItem {
                                queue_id: queue_id.clone(),
                                msg_id: *msg_id,
                                error: error.clone(),
                            })
                            .collect();

                        let (reply_tx, reply_rx) = tokio::sync::oneshot::channel();
                        broker
                            .send_command(fila_core::SchedulerCommand::Nack {
                                items: sched_items,
                                reply: reply_tx,
                            })
                            .map_err(broker_error_to_pair)?;

                        let sched_results = reply_rx.await.map_err(|_| {
                            (
                                ErrorCode::InternalError,
                                "scheduler reply dropped".to_string(),
                            )
                        })?;

                        for ((orig_idx, _, _), sched_result) in
                            queue_items.iter().zip(sched_results.into_iter())
                        {
                            results[*orig_idx] = match sched_result {
                                Ok(()) => NackResultItem {
                                    error_code: ErrorCode::Ok,
                                },
                                Err(fila_core::NackError::MessageNotFound(_)) => NackResultItem {
                                    error_code: ErrorCode::MessageNotFound,
                                },
                                Err(fila_core::NackError::Storage(_)) => NackResultItem {
                                    error_code: ErrorCode::StorageError,
                                },
                            };
                        }
                    }
                    // If forwarded, the leader already applied — results are Ok.
                }
                Err(e) => {
                    let (code, _msg) = cluster_write_err_to_pair(e);
                    for (orig_idx, _, _) in queue_items {
                        results[*orig_idx] = NackResultItem { error_code: code };
                    }
                }
            }
        }

        Ok(NackResponse { results })
    } else {
        // Single-node mode: direct to scheduler.
        let sched_items: Vec<fila_core::broker::command::NackItem> = items
            .into_iter()
            .map(
                |(queue_id, msg_id, error)| fila_core::broker::command::NackItem {
                    queue_id,
                    msg_id,
                    error,
                },
            )
            .collect();

        let (reply_tx, reply_rx) = tokio::sync::oneshot::channel();
        broker
            .send_command(fila_core::SchedulerCommand::Nack {
                items: sched_items,
                reply: reply_tx,
            })
            .map_err(broker_error_to_pair)?;

        let results = reply_rx.await.map_err(|_| {
            (
                ErrorCode::InternalError,
                "scheduler reply dropped".to_string(),
            )
        })?;

        let items: Vec<NackResultItem> = results
            .into_iter()
            .map(|r| match r {
                Ok(()) => NackResultItem {
                    error_code: ErrorCode::Ok,
                },
                Err(fila_core::NackError::MessageNotFound(_)) => NackResultItem {
                    error_code: ErrorCode::MessageNotFound,
                },
                Err(fila_core::NackError::Storage(_)) => NackResultItem {
                    error_code: ErrorCode::StorageError,
                },
            })
            .collect();

        Ok(NackResponse { results: items })
    }
}

/// Handle a create queue request.
///
/// In cluster mode (`cluster` is `Some`), the queue is created via the meta
/// Raft group so all nodes get the queue's Raft group. In single-node mode
/// the request goes directly to the local scheduler.
pub async fn handle_create_queue(
    broker: &Broker,
    cluster: Option<&ClusterHandle>,
    req: fila_fibp::CreateQueueRequest,
) -> Result<fila_fibp::CreateQueueResponse, (ErrorCode, String)> {
    let visibility_timeout_ms = match req.visibility_timeout_ms {
        0 => fila_core::QueueConfig::DEFAULT_VISIBILITY_TIMEOUT_MS,
        v => v,
    };

    let config = fila_core::QueueConfig {
        name: req.name.clone(),
        on_enqueue_script: req.on_enqueue_script,
        on_failure_script: req.on_failure_script,
        visibility_timeout_ms,
        dlq_queue_id: None,
        lua_timeout_ms: None,
        lua_memory_limit_bytes: None,
    };

    if let Some(cluster) = cluster {
        // Cluster mode: submit CreateQueueGroup to meta Raft.
        let (all_members, _member_addrs) = cluster.meta_members();
        let (members, preferred_leader) = select_members_and_leader(&all_members, cluster).await;

        let resp = cluster
            .write_to_meta(ClusterRequest::CreateQueueGroup {
                queue_id: req.name.clone(),
                members,
                config,
                preferred_leader,
            })
            .await
            .map_err(cluster_write_err_to_pair)?;

        match resp {
            ClusterResponse::CreateQueueGroup { queue_id } => Ok(fila_fibp::CreateQueueResponse {
                error_code: ErrorCode::Ok,
                queue_id,
            }),
            ClusterResponse::Error { message } => Err((ErrorCode::InternalError, message)),
            _ => Err((
                ErrorCode::InternalError,
                "unexpected cluster response".to_string(),
            )),
        }
    } else {
        // Single-node mode: direct to scheduler.
        let (reply_tx, reply_rx) = tokio::sync::oneshot::channel();
        broker
            .send_command(fila_core::SchedulerCommand::CreateQueue {
                name: req.name,
                config,
                reply: reply_tx,
            })
            .map_err(broker_error_to_pair)?;

        let result = reply_rx.await.map_err(|_| {
            (
                ErrorCode::InternalError,
                "scheduler reply dropped".to_string(),
            )
        })?;

        match result {
            Ok(queue_id) => Ok(fila_fibp::CreateQueueResponse {
                error_code: ErrorCode::Ok,
                queue_id,
            }),
            Err(fila_core::CreateQueueError::QueueAlreadyExists(name)) => {
                Ok(fila_fibp::CreateQueueResponse {
                    error_code: ErrorCode::QueueAlreadyExists,
                    queue_id: name,
                })
            }
            Err(fila_core::CreateQueueError::LuaCompilation(msg)) => {
                Ok(fila_fibp::CreateQueueResponse {
                    error_code: ErrorCode::LuaCompilationError,
                    queue_id: msg,
                })
            }
            Err(fila_core::CreateQueueError::Storage(_)) => Ok(fila_fibp::CreateQueueResponse {
                error_code: ErrorCode::StorageError,
                queue_id: String::new(),
            }),
        }
    }
}

/// Handle a delete queue request.
///
/// In cluster mode (`cluster` is `Some`), the queue is deleted via the meta
/// Raft group so all nodes remove the queue's Raft group. In single-node mode
/// the request goes directly to the local scheduler.
pub async fn handle_delete_queue(
    broker: &Broker,
    cluster: Option<&ClusterHandle>,
    req: fila_fibp::DeleteQueueRequest,
) -> Result<fila_fibp::DeleteQueueResponse, (ErrorCode, String)> {
    if let Some(cluster) = cluster {
        // Cluster mode: submit DeleteQueueGroup to meta Raft.
        let resp = cluster
            .write_to_meta(ClusterRequest::DeleteQueueGroup {
                queue_id: req.queue,
            })
            .await
            .map_err(cluster_write_err_to_pair)?;

        match resp {
            ClusterResponse::DeleteQueueGroup => Ok(fila_fibp::DeleteQueueResponse {
                error_code: ErrorCode::Ok,
            }),
            ClusterResponse::Error { message } => Err((ErrorCode::InternalError, message)),
            _ => Err((
                ErrorCode::InternalError,
                "unexpected cluster response".to_string(),
            )),
        }
    } else {
        // Single-node mode: direct to scheduler.
        let (reply_tx, reply_rx) = tokio::sync::oneshot::channel();
        broker
            .send_command(fila_core::SchedulerCommand::DeleteQueue {
                queue_id: req.queue,
                reply: reply_tx,
            })
            .map_err(broker_error_to_pair)?;

        let result = reply_rx.await.map_err(|_| {
            (
                ErrorCode::InternalError,
                "scheduler reply dropped".to_string(),
            )
        })?;

        match result {
            Ok(()) => Ok(fila_fibp::DeleteQueueResponse {
                error_code: ErrorCode::Ok,
            }),
            Err(fila_core::DeleteQueueError::QueueNotFound(_)) => {
                Ok(fila_fibp::DeleteQueueResponse {
                    error_code: ErrorCode::QueueNotFound,
                })
            }
            Err(fila_core::DeleteQueueError::Storage(_)) => Ok(fila_fibp::DeleteQueueResponse {
                error_code: ErrorCode::StorageError,
            }),
        }
    }
}

/// Handle a get stats request.
///
/// In cluster mode, enriches the scheduler stats with Raft leader and
/// replication count from the queue's Raft group metrics.
pub async fn handle_get_stats(
    broker: &Broker,
    cluster: Option<&ClusterHandle>,
    req: fila_fibp::GetStatsRequest,
) -> Result<fila_fibp::GetStatsResponse, (ErrorCode, String)> {
    let queue_name = req.queue;
    let (reply_tx, reply_rx) = tokio::sync::oneshot::channel();
    broker
        .send_command(fila_core::SchedulerCommand::GetStats {
            queue_id: queue_name.clone(),
            reply: reply_tx,
        })
        .map_err(broker_error_to_pair)?;

    let result = reply_rx.await.map_err(|_| {
        (
            ErrorCode::InternalError,
            "scheduler reply dropped".to_string(),
        )
    })?;

    match result {
        Ok(stats) => {
            // Enrich with cluster info if in cluster mode.
            let (leader_node_id, replication_count) = if let Some(cluster) = cluster {
                cluster
                    .multi_raft
                    .get_raft(&queue_name)
                    .await
                    .map(|raft| {
                        let metrics = raft.metrics().borrow().clone();
                        let leader_id = metrics.current_leader.unwrap_or(0);
                        let voters =
                            metrics.membership_config.membership().voter_ids().count() as u32;
                        (leader_id, voters)
                    })
                    .unwrap_or((0, 0))
            } else {
                (0, 0)
            };

            Ok(fila_fibp::GetStatsResponse {
                error_code: ErrorCode::Ok,
                depth: stats.depth,
                in_flight: stats.in_flight,
                active_fairness_keys: stats.active_fairness_keys,
                active_consumers: stats.active_consumers,
                quantum: stats.quantum,
                leader_node_id,
                replication_count,
                per_key_stats: stats
                    .per_key_stats
                    .into_iter()
                    .map(|s| fila_fibp::FairnessKeyStat {
                        key: s.key,
                        pending_count: s.pending_count,
                        current_deficit: s.current_deficit,
                        weight: s.weight,
                    })
                    .collect(),
                per_throttle_stats: stats
                    .per_throttle_stats
                    .into_iter()
                    .map(|s| fila_fibp::ThrottleKeyStat {
                        key: s.key,
                        tokens: s.tokens,
                        rate_per_second: s.rate_per_second,
                        burst: s.burst,
                    })
                    .collect(),
            })
        }
        Err(fila_core::StatsError::QueueNotFound(_)) => {
            Err((ErrorCode::QueueNotFound, "queue not found".to_string()))
        }
        Err(fila_core::StatsError::Storage(e)) => {
            Err((ErrorCode::StorageError, format!("storage error: {e}")))
        }
    }
}

/// Handle a list queues request.
///
/// In cluster mode, enriches each queue with its Raft leader node ID
/// and reports the total cluster node count.
pub async fn handle_list_queues(
    broker: &Broker,
    cluster: Option<&ClusterHandle>,
) -> Result<fila_fibp::ListQueuesResponse, (ErrorCode, String)> {
    let (reply_tx, reply_rx) = tokio::sync::oneshot::channel();
    broker
        .send_command(fila_core::SchedulerCommand::ListQueues { reply: reply_tx })
        .map_err(broker_error_to_pair)?;

    let result = reply_rx.await.map_err(|_| {
        (
            ErrorCode::InternalError,
            "scheduler reply dropped".to_string(),
        )
    })?;

    match result {
        Ok(summaries) => {
            let mut queues = Vec::with_capacity(summaries.len());
            for s in summaries {
                let leader_node_id = if let Some(cluster) = cluster {
                    cluster
                        .multi_raft
                        .get_raft(&s.name)
                        .await
                        .map(|raft| raft.metrics().borrow().current_leader.unwrap_or(0))
                        .unwrap_or(0)
                } else {
                    0
                };
                queues.push(fila_fibp::QueueInfo {
                    name: s.name,
                    depth: s.depth,
                    in_flight: s.in_flight,
                    active_consumers: s.active_consumers,
                    leader_node_id,
                });
            }

            let cluster_node_count = cluster
                .map(|c| {
                    let (members, _) = c.meta_members();
                    members.len() as u32
                })
                .unwrap_or(0);

            Ok(fila_fibp::ListQueuesResponse {
                error_code: ErrorCode::Ok,
                cluster_node_count,
                queues,
            })
        }
        Err(fila_core::ListQueuesError::Storage(e)) => {
            Err((ErrorCode::StorageError, format!("storage error: {e}")))
        }
    }
}

/// Handle a set config request.
pub async fn handle_set_config(
    broker: &Broker,
    req: fila_fibp::SetConfigRequest,
) -> Result<fila_fibp::SetConfigResponse, (ErrorCode, String)> {
    let (reply_tx, reply_rx) = tokio::sync::oneshot::channel();
    broker
        .send_command(fila_core::SchedulerCommand::SetConfig {
            key: req.key,
            value: req.value,
            reply: reply_tx,
        })
        .map_err(broker_error_to_pair)?;

    let result = reply_rx.await.map_err(|_| {
        (
            ErrorCode::InternalError,
            "scheduler reply dropped".to_string(),
        )
    })?;

    match result {
        Ok(()) => Ok(fila_fibp::SetConfigResponse {
            error_code: ErrorCode::Ok,
        }),
        Err(fila_core::ConfigError::InvalidValue(_)) => Ok(fila_fibp::SetConfigResponse {
            error_code: ErrorCode::InvalidConfigValue,
        }),
        Err(fila_core::ConfigError::Storage(_)) => Ok(fila_fibp::SetConfigResponse {
            error_code: ErrorCode::StorageError,
        }),
    }
}

/// Handle a get config request.
pub async fn handle_get_config(
    broker: &Broker,
    req: fila_fibp::GetConfigRequest,
) -> Result<fila_fibp::GetConfigResponse, (ErrorCode, String)> {
    let (reply_tx, reply_rx) = tokio::sync::oneshot::channel();
    broker
        .send_command(fila_core::SchedulerCommand::GetConfig {
            key: req.key,
            reply: reply_tx,
        })
        .map_err(broker_error_to_pair)?;

    let result = reply_rx.await.map_err(|_| {
        (
            ErrorCode::InternalError,
            "scheduler reply dropped".to_string(),
        )
    })?;

    match result {
        Ok(value) => Ok(fila_fibp::GetConfigResponse {
            error_code: ErrorCode::Ok,
            value: value.unwrap_or_default(),
        }),
        Err(fila_core::ConfigError::InvalidValue(_)) => Ok(fila_fibp::GetConfigResponse {
            error_code: ErrorCode::InvalidConfigValue,
            value: String::new(),
        }),
        Err(fila_core::ConfigError::Storage(_)) => Ok(fila_fibp::GetConfigResponse {
            error_code: ErrorCode::StorageError,
            value: String::new(),
        }),
    }
}

/// Handle a list config request.
pub async fn handle_list_config(
    broker: &Broker,
    req: fila_fibp::ListConfigRequest,
) -> Result<fila_fibp::ListConfigResponse, (ErrorCode, String)> {
    let (reply_tx, reply_rx) = tokio::sync::oneshot::channel();
    broker
        .send_command(fila_core::SchedulerCommand::ListConfig {
            prefix: req.prefix,
            reply: reply_tx,
        })
        .map_err(broker_error_to_pair)?;

    let result = reply_rx.await.map_err(|_| {
        (
            ErrorCode::InternalError,
            "scheduler reply dropped".to_string(),
        )
    })?;

    match result {
        Ok(entries) => Ok(fila_fibp::ListConfigResponse {
            error_code: ErrorCode::Ok,
            entries: entries
                .into_iter()
                .map(|(key, value)| fila_fibp::ConfigEntry { key, value })
                .collect(),
        }),
        Err(fila_core::ConfigError::InvalidValue(_)) => Ok(fila_fibp::ListConfigResponse {
            error_code: ErrorCode::InvalidConfigValue,
            entries: vec![],
        }),
        Err(fila_core::ConfigError::Storage(_)) => Ok(fila_fibp::ListConfigResponse {
            error_code: ErrorCode::StorageError,
            entries: vec![],
        }),
    }
}

/// Handle a redrive request.
pub async fn handle_redrive(
    broker: &Broker,
    req: fila_fibp::RedriveRequest,
) -> Result<fila_fibp::RedriveResponse, (ErrorCode, String)> {
    let (reply_tx, reply_rx) = tokio::sync::oneshot::channel();
    broker
        .send_command(fila_core::SchedulerCommand::Redrive {
            dlq_queue_id: req.dlq_queue,
            count: req.count,
            reply: reply_tx,
        })
        .map_err(broker_error_to_pair)?;

    let result = reply_rx.await.map_err(|_| {
        (
            ErrorCode::InternalError,
            "scheduler reply dropped".to_string(),
        )
    })?;

    match result {
        Ok(redriven) => Ok(fila_fibp::RedriveResponse {
            error_code: ErrorCode::Ok,
            redriven,
        }),
        Err(fila_core::RedriveError::QueueNotFound(_)) => {
            Err((ErrorCode::QueueNotFound, "queue not found".to_string()))
        }
        Err(fila_core::RedriveError::NotADLQ(_)) => {
            Err((ErrorCode::NotADLQ, "not a dead-letter queue".to_string()))
        }
        Err(fila_core::RedriveError::ParentQueueNotFound(_)) => Err((
            ErrorCode::ParentQueueNotFound,
            "parent queue not found".to_string(),
        )),
        Err(fila_core::RedriveError::Storage(e)) => {
            Err((ErrorCode::StorageError, format!("storage error: {e}")))
        }
    }
}

/// Handle a create API key request.
pub fn handle_create_api_key(
    broker: &Broker,
    req: fila_fibp::CreateApiKeyRequest,
) -> Result<fila_fibp::CreateApiKeyResponse, (ErrorCode, String)> {
    let expires_at = if req.expires_at_ms == 0 {
        None
    } else {
        Some(req.expires_at_ms)
    };
    let (key_id, token) = broker
        .create_api_key(&req.name, expires_at, req.is_superadmin)
        .map_err(|e| (ErrorCode::StorageError, format!("storage error: {e}")))?;
    Ok(fila_fibp::CreateApiKeyResponse {
        error_code: ErrorCode::Ok,
        key_id,
        key: token,
        is_superadmin: req.is_superadmin,
    })
}

/// Handle a revoke API key request.
pub fn handle_revoke_api_key(
    broker: &Broker,
    req: fila_fibp::RevokeApiKeyRequest,
) -> Result<fila_fibp::RevokeApiKeyResponse, (ErrorCode, String)> {
    let found = broker
        .revoke_api_key(&req.key_id)
        .map_err(|e| (ErrorCode::StorageError, format!("storage error: {e}")))?;
    if found {
        Ok(fila_fibp::RevokeApiKeyResponse {
            error_code: ErrorCode::Ok,
        })
    } else {
        Ok(fila_fibp::RevokeApiKeyResponse {
            error_code: ErrorCode::ApiKeyNotFound,
        })
    }
}

/// Handle a list API keys request.
pub fn handle_list_api_keys(
    broker: &Broker,
) -> Result<fila_fibp::ListApiKeysResponse, (ErrorCode, String)> {
    let entries = broker
        .list_api_keys()
        .map_err(|e| (ErrorCode::StorageError, format!("storage error: {e}")))?;
    let keys = entries
        .into_iter()
        .map(|e| fila_fibp::ApiKeyInfo {
            key_id: e.key_id,
            name: e.name,
            created_at_ms: e.created_at_ms,
            expires_at_ms: e.expires_at_ms.unwrap_or(0),
            is_superadmin: e.is_superadmin,
        })
        .collect();
    Ok(fila_fibp::ListApiKeysResponse {
        error_code: ErrorCode::Ok,
        keys,
    })
}

/// Handle a set ACL request.
pub fn handle_set_acl(
    broker: &Broker,
    req: fila_fibp::SetAclRequest,
) -> Result<fila_fibp::SetAclResponse, (ErrorCode, String)> {
    // Validate permission kinds before applying.
    for p in &req.permissions {
        match p.kind.as_str() {
            "produce" | "consume" | "admin" => {}
            other => {
                return Err((
                    ErrorCode::InvalidConfigValue,
                    format!(
                        "invalid permission kind \"{other}\": expected produce, consume, or admin"
                    ),
                ));
            }
        }
    }
    let permissions: Vec<(String, String)> = req
        .permissions
        .into_iter()
        .map(|p| (p.kind, p.pattern))
        .collect();
    let found = broker
        .set_acl(&req.key_id, permissions)
        .map_err(|e| (ErrorCode::StorageError, format!("storage error: {e}")))?;
    if found {
        Ok(fila_fibp::SetAclResponse {
            error_code: ErrorCode::Ok,
        })
    } else {
        Ok(fila_fibp::SetAclResponse {
            error_code: ErrorCode::ApiKeyNotFound,
        })
    }
}

/// Handle a get ACL request.
pub fn handle_get_acl(
    broker: &Broker,
    req: fila_fibp::GetAclRequest,
) -> Result<fila_fibp::GetAclResponse, (ErrorCode, String)> {
    match broker
        .get_acl(&req.key_id)
        .map_err(|e| (ErrorCode::StorageError, format!("storage error: {e}")))?
    {
        Some(entry) => Ok(fila_fibp::GetAclResponse {
            error_code: ErrorCode::Ok,
            key_id: entry.key_id,
            is_superadmin: entry.is_superadmin,
            permissions: entry
                .permissions
                .into_iter()
                .map(|(kind, pattern)| fila_fibp::AclPermission { kind, pattern })
                .collect(),
        }),
        None => Ok(fila_fibp::GetAclResponse {
            error_code: ErrorCode::ApiKeyNotFound,
            key_id: req.key_id,
            is_superadmin: false,
            permissions: vec![],
        }),
    }
}

/// Convert a BrokerError into an (ErrorCode, String) pair.
fn broker_error_to_pair(e: fila_core::BrokerError) -> (ErrorCode, String) {
    match e {
        fila_core::BrokerError::ChannelFull => {
            (ErrorCode::ChannelFull, "scheduler overloaded".to_string())
        }
        fila_core::BrokerError::ChannelDisconnected => (
            ErrorCode::InternalError,
            "scheduler unavailable".to_string(),
        ),
        fila_core::BrokerError::SchedulerSpawn(msg) => (ErrorCode::InternalError, msg),
        fila_core::BrokerError::SchedulerPanicked => {
            (ErrorCode::InternalError, "scheduler panicked".to_string())
        }
    }
}

/// Convert a ClusterWriteError into an (ErrorCode, String) pair.
fn cluster_write_err_to_pair(e: fila_core::ClusterWriteError) -> (ErrorCode, String) {
    match e {
        fila_core::ClusterWriteError::QueueGroupNotFound => (
            ErrorCode::QueueNotFound,
            "queue raft group not found".to_string(),
        ),
        fila_core::ClusterWriteError::NodeNotReady => (
            ErrorCode::NotLeader,
            "node not ready for this queue — retry on another node or wait".to_string(),
        ),
        fila_core::ClusterWriteError::NoLeader => {
            (ErrorCode::NotLeader, "no leader available".to_string())
        }
        fila_core::ClusterWriteError::Raft(e) => {
            (ErrorCode::InternalError, format!("raft error: {e}"))
        }
        fila_core::ClusterWriteError::Forward(e) => {
            (ErrorCode::InternalError, format!("forward error: {e}"))
        }
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

    // Select subset if cluster is larger than replication factor.
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

    // Preferred leader: least-loaded among selected (tie-break: lowest ID).
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
