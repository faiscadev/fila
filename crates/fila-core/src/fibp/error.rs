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

    /// The operation requested is not yet implemented (placeholder for Story 36.2).
    #[error("operation not implemented: 0x{op:02X}")]
    NotImplemented { op: u8 },
}
