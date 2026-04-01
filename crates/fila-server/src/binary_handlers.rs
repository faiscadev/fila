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
