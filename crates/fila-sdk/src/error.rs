use tonic::Code;

/// Common gRPC status errors shared across all operations.
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

    #[error("unexpected gRPC error ({code:?}): {message}")]
    Rpc { code: Code, message: String },
}

// --- Per-operation error types ---

#[derive(Debug, thiserror::Error)]
pub enum ConnectError {
    #[error("connection failed: {0}")]
    Transport(#[from] tonic::transport::Error),

    #[error("invalid argument: {0}")]
    InvalidArgument(String),
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

pub(crate) fn status_error(status: tonic::Status) -> StatusError {
    let message = status.message().to_string();
    match status.code() {
        Code::InvalidArgument => StatusError::InvalidArgument(message),
        Code::Unavailable => StatusError::Unavailable(message),
        Code::Internal => StatusError::Internal(message),
        code => StatusError::Rpc { code, message },
    }
}

pub(crate) fn enqueue_status_error(status: tonic::Status) -> EnqueueError {
    let message = status.message().to_string();
    match status.code() {
        Code::NotFound => EnqueueError::QueueNotFound(message),
        _ => EnqueueError::Status(status_error(status)),
    }
}

pub(crate) fn consume_status_error(status: tonic::Status) -> ConsumeError {
    let message = status.message().to_string();
    match status.code() {
        Code::NotFound => ConsumeError::QueueNotFound(message),
        _ => ConsumeError::Status(status_error(status)),
    }
}

pub(crate) fn ack_status_error(status: tonic::Status) -> AckError {
    let message = status.message().to_string();
    match status.code() {
        Code::NotFound => AckError::MessageNotFound(message),
        _ => AckError::Status(status_error(status)),
    }
}

pub(crate) fn nack_status_error(status: tonic::Status) -> NackError {
    let message = status.message().to_string();
    match status.code() {
        Code::NotFound => NackError::MessageNotFound(message),
        _ => NackError::Status(status_error(status)),
    }
}
