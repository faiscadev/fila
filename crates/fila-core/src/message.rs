use std::collections::HashMap;

use bytes::Bytes;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// Core message domain type. This is the internal representation used by the
/// scheduler and storage layer — distinct from the protobuf wire type.
///
/// `payload` uses `bytes::Bytes` for zero-copy reference-counted clones on the
/// hot path (enqueue → storage → delivery). This avoids memcpy when the same
/// payload is passed through multiple layers.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Message {
    pub id: Uuid,
    pub queue_id: String,
    pub headers: HashMap<String, String>,
    #[serde(with = "serde_bytes_as_bytes")]
    pub payload: Bytes,
    pub fairness_key: String,
    pub weight: u32,
    pub throttle_keys: Vec<String>,
    pub attempt_count: u32,
    pub enqueued_at: u64,
    pub leased_at: Option<u64>,
}

/// Custom serde module for `bytes::Bytes` that serializes as a byte sequence
/// (matching the previous `Vec<u8>` wire format) so existing stored messages
/// deserialize correctly.
mod serde_bytes_as_bytes {
    use bytes::Bytes;
    use serde::{self, Deserialize, Deserializer, Serializer};

    pub fn serialize<S>(data: &Bytes, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_bytes(data)
    }

    pub fn deserialize<'de, D>(deserializer: D) -> Result<Bytes, D::Error>
    where
        D: Deserializer<'de>,
    {
        let v: Vec<u8> = Deserialize::deserialize(deserializer)?;
        Ok(Bytes::from(v))
    }
}

impl Message {
    /// Generate a new UUIDv7 message ID.
    pub fn new_id() -> Uuid {
        Uuid::now_v7()
    }
}
