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
    #[error("io error")]
    Io(#[from] std::io::Error),
}
