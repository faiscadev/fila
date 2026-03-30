//! Binary protocol operation handlers — translate FIBP typed requests into
//! `SchedulerCommand` batch variants and convert results back to FIBP responses.

use fila_core::{Broker, Message};
use fila_fibp::{
    AckResponse, AckResultItem, EnqueueResponse, EnqueueResultItem, ErrorCode, NackResponse,
    NackResultItem,
};
use uuid::Uuid;

/// Handle a batch enqueue request.
pub async fn handle_enqueue(
    broker: &Broker,
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

    let (reply_tx, reply_rx) = tokio::sync::oneshot::channel();
    broker
        .send_command(fila_core::SchedulerCommand::Enqueue {
            messages,
            reply: reply_tx,
        })
        .map_err(|e| match e {
            fila_core::BrokerError::ChannelFull => {
                (ErrorCode::ChannelFull, "scheduler overloaded".to_string())
            }
            fila_core::BrokerError::ChannelDisconnected => (
                ErrorCode::InternalError,
                "scheduler unavailable".to_string(),
            ),
            other => (ErrorCode::InternalError, format!("{other}")),
        })?;

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

/// Handle a batch ack request.
pub async fn handle_ack(
    broker: &Broker,
    req: fila_fibp::AckRequest,
) -> Result<AckResponse, (ErrorCode, String)> {
    let items: Vec<fila_core::broker::command::AckItem> = req
        .items
        .into_iter()
        .map(|item| {
            let msg_id = item.message_id.parse::<Uuid>().unwrap_or(Uuid::nil());
            fila_core::broker::command::AckItem {
                queue_id: item.queue,
                msg_id,
            }
        })
        .collect();

    let (reply_tx, reply_rx) = tokio::sync::oneshot::channel();
    broker
        .send_command(fila_core::SchedulerCommand::Ack {
            items,
            reply: reply_tx,
        })
        .map_err(|e| match e {
            fila_core::BrokerError::ChannelFull => {
                (ErrorCode::ChannelFull, "scheduler overloaded".to_string())
            }
            fila_core::BrokerError::ChannelDisconnected => (
                ErrorCode::InternalError,
                "scheduler unavailable".to_string(),
            ),
            other => (ErrorCode::InternalError, format!("{other}")),
        })?;

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

/// Handle a batch nack request.
pub async fn handle_nack(
    broker: &Broker,
    req: fila_fibp::NackRequest,
) -> Result<NackResponse, (ErrorCode, String)> {
    let items: Vec<fila_core::broker::command::NackItem> = req
        .items
        .into_iter()
        .map(|item| {
            let msg_id = item.message_id.parse::<Uuid>().unwrap_or(Uuid::nil());
            fila_core::broker::command::NackItem {
                queue_id: item.queue,
                msg_id,
                error: item.error,
            }
        })
        .collect();

    let (reply_tx, reply_rx) = tokio::sync::oneshot::channel();
    broker
        .send_command(fila_core::SchedulerCommand::Nack {
            items,
            reply: reply_tx,
        })
        .map_err(|e| match e {
            fila_core::BrokerError::ChannelFull => {
                (ErrorCode::ChannelFull, "scheduler overloaded".to_string())
            }
            fila_core::BrokerError::ChannelDisconnected => (
                ErrorCode::InternalError,
                "scheduler unavailable".to_string(),
            ),
            other => (ErrorCode::InternalError, format!("{other}")),
        })?;

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

/// Handle a create queue request.
pub async fn handle_create_queue(
    broker: &Broker,
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

/// Handle a delete queue request.
pub async fn handle_delete_queue(
    broker: &Broker,
    req: fila_fibp::DeleteQueueRequest,
) -> Result<fila_fibp::DeleteQueueResponse, (ErrorCode, String)> {
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
        Err(fila_core::DeleteQueueError::QueueNotFound(_)) => Ok(fila_fibp::DeleteQueueResponse {
            error_code: ErrorCode::QueueNotFound,
        }),
        Err(fila_core::DeleteQueueError::Storage(_)) => Ok(fila_fibp::DeleteQueueResponse {
            error_code: ErrorCode::StorageError,
        }),
    }
}

/// Handle a get stats request.
pub async fn handle_get_stats(
    broker: &Broker,
    req: fila_fibp::GetStatsRequest,
) -> Result<fila_fibp::GetStatsResponse, (ErrorCode, String)> {
    let (reply_tx, reply_rx) = tokio::sync::oneshot::channel();
    broker
        .send_command(fila_core::SchedulerCommand::GetStats {
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
        Ok(stats) => Ok(fila_fibp::GetStatsResponse {
            error_code: ErrorCode::Ok,
            depth: stats.depth,
            in_flight: stats.in_flight,
            active_fairness_keys: stats.active_fairness_keys,
            active_consumers: stats.active_consumers,
            quantum: stats.quantum,
            leader_node_id: stats.leader_node_id,
            replication_count: stats.replication_count,
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
        }),
        Err(fila_core::StatsError::QueueNotFound(_)) => {
            Err((ErrorCode::QueueNotFound, "queue not found".to_string()))
        }
        Err(fila_core::StatsError::Storage(e)) => {
            Err((ErrorCode::StorageError, format!("storage error: {e}")))
        }
    }
}

/// Handle a list queues request.
pub async fn handle_list_queues(
    broker: &Broker,
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
            let queues = summaries
                .into_iter()
                .map(|s| fila_fibp::QueueInfo {
                    name: s.name,
                    depth: s.depth,
                    in_flight: s.in_flight,
                    active_consumers: s.active_consumers,
                    leader_node_id: s.leader_node_id,
                })
                .collect();
            Ok(fila_fibp::ListQueuesResponse {
                error_code: ErrorCode::Ok,
                cluster_node_count: 0,
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
