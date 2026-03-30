use fila_fibp::ErrorCode;

/// Common protocol errors shared across all operations.
///
/// Analogous to `StorageError` in fila-core — the "infra" error that every
/// per-operation type embeds via `#[from]`.
#[derive(Debug, thiserror::Error)]
pub enum StatusError {
    #[error("invalid argument: {0}")]
    InvalidArgument(String),

    #[error("server unavailable: {0}")]
    Unavailable(String),

    #[error("internal server error: {0}")]
    Internal(String),

    #[error("unauthorized: {0}")]
    Unauthorized(String),

    #[error("forbidden: {0}")]
    Forbidden(String),

    #[error("transport error: {0}")]
    Transport(#[source] std::io::Error),

    #[error("protocol error: {0}")]
    Protocol(String),
}

// --- Per-operation error types ---

#[derive(Debug, thiserror::Error)]
pub enum ConnectError {
    #[error("connection failed: {0}")]
    Transport(#[source] std::io::Error),

    #[error("invalid argument: {0}")]
    InvalidArgument(String),

    #[error("handshake failed: {0}")]
    HandshakeFailed(String),

    #[error("TLS configuration error: {0}")]
    TlsConfig(String),

    #[error("connection timed out")]
    TimedOut,
}

#[derive(Debug, thiserror::Error)]
pub enum EnqueueError {
    #[error("queue not found: {0}")]
    QueueNotFound(String),

    #[error(transparent)]
    Status(#[from] StatusError),
}

#[derive(Debug, thiserror::Error)]
pub enum ConsumeError {
    #[error("queue not found: {0}")]
    QueueNotFound(String),

    #[error(transparent)]
    Status(#[from] StatusError),
}

#[derive(Debug, thiserror::Error)]
pub enum AckError {
    #[error("message not found: {0}")]
    MessageNotFound(String),

    #[error(transparent)]
    Status(#[from] StatusError),
}

#[derive(Debug, thiserror::Error)]
pub enum NackError {
    #[error("message not found: {0}")]
    MessageNotFound(String),

    #[error(transparent)]
    Status(#[from] StatusError),
}

// --- Mapping helpers ---

/// Map a binary protocol `ErrorCode` + message into a `StatusError`.
pub(crate) fn error_code_to_status(code: ErrorCode, message: String) -> StatusError {
    match code {
        ErrorCode::InvalidFrame | ErrorCode::InvalidConfigValue => {
            StatusError::InvalidArgument(message)
        }
        ErrorCode::NotLeader | ErrorCode::NodeNotReady | ErrorCode::ChannelFull => {
            StatusError::Unavailable(message)
        }
        ErrorCode::Unauthorized | ErrorCode::UnsupportedVersion => {
            StatusError::Unauthorized(message)
        }
        ErrorCode::Forbidden => StatusError::Forbidden(message),
        ErrorCode::StorageError | ErrorCode::InternalError => StatusError::Internal(message),
        // Errors that should not appear as generic status errors (they have
        // dedicated per-operation variants) are mapped to Internal as a fallback.
        ErrorCode::Ok
        | ErrorCode::QueueNotFound
        | ErrorCode::MessageNotFound
        | ErrorCode::QueueAlreadyExists
        | ErrorCode::LuaCompilationError
        | ErrorCode::NotADLQ
        | ErrorCode::ParentQueueNotFound
        | ErrorCode::ApiKeyNotFound => StatusError::Internal(message),
    }
}

pub(crate) fn enqueue_error_from_code(code: ErrorCode, message: String) -> EnqueueError {
    match code {
        ErrorCode::QueueNotFound => EnqueueError::QueueNotFound(message),
        _ => EnqueueError::Status(error_code_to_status(code, message)),
    }
}

pub(crate) fn consume_error_from_code(code: ErrorCode, message: String) -> ConsumeError {
    match code {
        ErrorCode::QueueNotFound => ConsumeError::QueueNotFound(message),
        _ => ConsumeError::Status(error_code_to_status(code, message)),
    }
}

pub(crate) fn ack_error_from_code(code: ErrorCode, message: String) -> AckError {
    match code {
        ErrorCode::MessageNotFound => AckError::MessageNotFound(message),
        _ => AckError::Status(error_code_to_status(code, message)),
    }
}

pub(crate) fn nack_error_from_code(code: ErrorCode, message: String) -> NackError {
    match code {
        ErrorCode::MessageNotFound => NackError::MessageNotFound(message),
        _ => NackError::Status(error_code_to_status(code, message)),
    }
}
