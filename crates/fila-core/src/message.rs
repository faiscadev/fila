use std::collections::HashMap;

use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// Core message domain type. This is the internal representation used by the
/// scheduler and storage layer — distinct from the protobuf wire type.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Message {
    pub id: Uuid,
    pub queue_id: String,
    pub headers: HashMap<String, String>,
    pub payload: Vec<u8>,
    pub fairness_key: String,
    pub weight: u32,
    pub throttle_keys: Vec<String>,
    pub attempt_count: u32,
    pub enqueued_at: u64,
    pub leased_at: Option<u64>,
}

impl Message {
    /// Generate a new UUIDv7 message ID.
    pub fn new_id() -> Uuid {
        Uuid::now_v7()
    }
}
