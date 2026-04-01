/// Protocol error codes as defined in docs/protocol.md.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[repr(u8)]
pub enum ErrorCode {
    Ok = 0x00,
    QueueNotFound = 0x01,
    MessageNotFound = 0x02,
    QueueAlreadyExists = 0x03,
    LuaCompilationError = 0x04,
    StorageError = 0x05,
    NotADLQ = 0x06,
    ParentQueueNotFound = 0x07,
    InvalidConfigValue = 0x08,
    ChannelFull = 0x09,
    Unauthorized = 0x0A,
    Forbidden = 0x0B,
    NotLeader = 0x0C,
    UnsupportedVersion = 0x0D,
    InvalidFrame = 0x0E,
    ApiKeyNotFound = 0x0F,
    NodeNotReady = 0x10,
    InternalError = 0xFF,
}

impl ErrorCode {
    pub fn from_u8(byte: u8) -> ErrorCode {
        match byte {
            0x00 => ErrorCode::Ok,
            0x01 => ErrorCode::QueueNotFound,
            0x02 => ErrorCode::MessageNotFound,
            0x03 => ErrorCode::QueueAlreadyExists,
            0x04 => ErrorCode::LuaCompilationError,
            0x05 => ErrorCode::StorageError,
            0x06 => ErrorCode::NotADLQ,
            0x07 => ErrorCode::ParentQueueNotFound,
            0x08 => ErrorCode::InvalidConfigValue,
            0x09 => ErrorCode::ChannelFull,
            0x0A => ErrorCode::Unauthorized,
            0x0B => ErrorCode::Forbidden,
            0x0C => ErrorCode::NotLeader,
            0x0D => ErrorCode::UnsupportedVersion,
            0x0E => ErrorCode::InvalidFrame,
            0x0F => ErrorCode::ApiKeyNotFound,
            0x10 => ErrorCode::NodeNotReady,
            _ => ErrorCode::InternalError,
        }
    }
}
