/// Errors produced by the FIBP protocol layer.
#[derive(Debug, thiserror::Error)]
pub enum FibpError {
    /// The remote peer sent an invalid or unsupported magic/version handshake.
    #[error("handshake failed: {reason}")]
    HandshakeFailed { reason: String },

    /// A frame exceeded the configured `max_frame_size`.
    #[error("frame too large: {size} bytes exceeds limit of {limit} bytes")]
    FrameTooLarge { size: u32, limit: u32 },

    /// The peer sent a frame with an unrecognised op code.
    #[error("unknown op code: 0x{op:02X}")]
    UnknownOp { op: u8 },

    /// An I/O error on the underlying TCP stream.
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),

    /// The operation requested is not yet implemented.
    #[error("operation not implemented: 0x{op:02X}")]
    NotImplemented { op: u8 },

    /// A frame payload could not be decoded (malformed wire data).
    #[error("invalid payload: {reason}")]
    InvalidPayload { reason: String },

    /// The scheduler command channel is unavailable.
    #[error("broker unavailable: {0}")]
    BrokerUnavailable(#[from] crate::error::BrokerError),

    /// The scheduler reply channel was dropped before a response arrived.
    #[error("scheduler reply dropped")]
    ReplyDropped,

    /// Authentication required but not provided or invalid.
    #[error("authentication failed: {reason}")]
    AuthFailed { reason: String },

    /// The caller does not have permission for the requested operation.
    #[error("permission denied: {reason}")]
    PermissionDenied { reason: String },

    /// Wire decode error on an admin payload.
    #[error("wire decode error: {0}")]
    WireDecode(#[from] fila_fibp::FibpCodecError),

    /// TLS configuration error.
    #[error("tls error: {reason}")]
    TlsConfig { reason: String },

    /// A storage error during auth validation.
    #[error("storage error: {0}")]
    Storage(#[from] crate::error::StorageError),
}
