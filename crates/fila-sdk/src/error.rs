use tonic::Code;

/// Errors returned by the Fila client SDK.
#[derive(Debug, thiserror::Error)]
pub enum ClientError {
    #[error("connection failed: {0}")]
    Connect(#[from] tonic::transport::Error),

    #[error("queue not found: {0}")]
    QueueNotFound(String),

    #[error("message not found: {0}")]
    MessageNotFound(String),

    #[error("invalid argument: {0}")]
    InvalidArgument(String),

    #[error("already exists: {0}")]
    AlreadyExists(String),

    #[error("server unavailable: {0}")]
    Unavailable(String),

    #[error("internal server error: {0}")]
    Internal(String),

    #[error("unexpected gRPC error ({code:?}): {message}")]
    Rpc { code: Code, message: String },
}

/// Maps a tonic Status to ClientError in a queue-operation context
/// (NOT_FOUND → QueueNotFound).
pub(crate) fn queue_status_error(status: tonic::Status) -> ClientError {
    let message = status.message().to_string();
    match status.code() {
        Code::NotFound => ClientError::QueueNotFound(message),
        code => status_error_common(code, message),
    }
}

/// Maps a tonic Status to ClientError in a message-operation context
/// (NOT_FOUND → MessageNotFound).
pub(crate) fn message_status_error(status: tonic::Status) -> ClientError {
    let message = status.message().to_string();
    match status.code() {
        Code::NotFound => ClientError::MessageNotFound(message),
        code => status_error_common(code, message),
    }
}

fn status_error_common(code: Code, message: String) -> ClientError {
    match code {
        Code::InvalidArgument => ClientError::InvalidArgument(message),
        Code::AlreadyExists => ClientError::AlreadyExists(message),
        Code::Unavailable => ClientError::Unavailable(message),
        Code::Internal => ClientError::Internal(message),
        code => ClientError::Rpc { code, message },
    }
}
