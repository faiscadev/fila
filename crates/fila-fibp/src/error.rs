/// Errors that can occur during frame encoding/decoding.
#[derive(Debug, thiserror::Error)]
pub enum FrameError {
    #[error("frame too large: {size} bytes exceeds max {max} bytes")]
    FrameTooLarge { size: u32, max: u32 },
    #[error("unknown opcode: 0x{0:02x}")]
    UnknownOpcode(u8),
    #[error("incomplete frame: need {need} bytes, have {have}")]
    IncompleteFrame { need: usize, have: usize },
    #[error("decoded count {count} exceeds maximum allowed {max}")]
    CountExceedsLimit { count: usize, max: usize },
    #[error("invalid string: not valid UTF-8")]
    InvalidUtf8,
    #[error(
        "continuation opcode mismatch for request_id {request_id}: \
         initial 0x{initial:02x}, got 0x{got:02x}"
    )]
    ContinuationOpcodeMismatch {
        request_id: u32,
        initial: u8,
        got: u8,
    },
    #[error(
        "reassembled payload too large for request_id {request_id}: \
         {size} bytes exceeds max {max} bytes"
    )]
    ReassembledTooLarge {
        request_id: u32,
        size: usize,
        max: usize,
    },
    #[error("io error")]
    Io(#[from] std::io::Error),
}
