//! Wire-level error types for the FIBP codec and payload encoding/decoding.

/// Errors produced by the FIBP codec and wire encoding/decoding layer.
///
/// These are pure wire-format errors -- they do not include server-side
/// domain errors (broker unavailable, auth failed, etc.), which belong
/// in `fila-core`'s own error hierarchy.
#[derive(Debug, thiserror::Error)]
pub enum FibpCodecError {
    /// A frame exceeded the configured `max_frame_size`.
    #[error("frame too large: {size} bytes exceeds limit of {limit} bytes")]
    FrameTooLarge { size: u32, limit: u32 },

    /// A frame payload could not be decoded (malformed wire data).
    #[error("invalid payload: {reason}")]
    InvalidPayload { reason: String },

    /// An I/O error on the underlying transport.
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),
}
