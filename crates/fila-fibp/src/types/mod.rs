mod admin;
mod auth;
mod cluster;
mod error;
mod handshake;
mod hotpath;

pub use admin::*;
pub use auth::*;
pub use cluster::*;
pub use error::*;
pub use handshake::*;
pub use hotpath::*;

use bytes::Bytes;

use crate::error::FrameError;
use crate::frame::RawFrame;

/// Trait for protocol messages that can be encoded to/decoded from wire frames.
pub trait ProtocolMessage: Sized {
    fn encode(&self, request_id: u32) -> RawFrame;
    fn decode(payload: Bytes) -> Result<Self, FrameError>;
}

/// Maximum number of items allowed in a decoded batch/array to prevent
/// memory-exhaustion attacks from a malicious or corrupt frame.
const MAX_DECODED_COUNT: usize = 1_000_000;

fn check_count(count: usize) -> Result<(), FrameError> {
    if count > MAX_DECODED_COUNT {
        return Err(FrameError::CountExceedsLimit {
            count,
            max: MAX_DECODED_COUNT,
        });
    }
    Ok(())
}
