/// Common status errors shared across all operations.
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

    #[error("protocol error: {0}")]
    Protocol(String),
}

// --- Per-operation error types ---

#[derive(Debug, thiserror::Error)]
pub enum ConnectError {
    #[error("connection failed: {0}")]
    Io(#[from] std::io::Error),

    #[error("invalid argument: {0}")]
    InvalidArgument(String),

    #[error("handshake failed: {0}")]
    Handshake(String),

    #[error("authentication failed: {0}")]
    Auth(String),

    #[error("TLS error: {0}")]
    Tls(String),
}

#[derive(Debug, thiserror::Error)]
pub enum EnqueueError {
    #[error("queue not found: {0}")]
    QueueNotFound(String),

    #[error("permission denied: {0}")]
    PermissionDenied(String),

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

    #[error("permission denied: {0}")]
    PermissionDenied(String),

    #[error(transparent)]
    Status(#[from] StatusError),
}

#[derive(Debug, thiserror::Error)]
pub enum NackError {
    #[error("message not found: {0}")]
    MessageNotFound(String),

    #[error("permission denied: {0}")]
    PermissionDenied(String),

    #[error(transparent)]
    Status(#[from] StatusError),
}
