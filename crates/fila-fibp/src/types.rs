use std::collections::HashMap;

use bytes::Bytes;

use crate::error::FrameError;
use crate::error_code::ErrorCode;
use crate::frame::{PayloadReader, PayloadWriter, RawFrame};
use crate::opcode::Opcode;

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

// ---------------------------------------------------------------------------
// Control frames
// ---------------------------------------------------------------------------

#[derive(Debug, Clone)]
pub struct Handshake {
    pub protocol_version: u16,
    pub api_key: Option<String>,
}

#[derive(Debug, Clone)]
pub struct HandshakeOk {
    pub negotiated_version: u16,
    pub node_id: u64,
    pub max_frame_size: u32,
}

// ---------------------------------------------------------------------------
// Hot-path frames
// ---------------------------------------------------------------------------

#[derive(Debug, Clone)]
pub struct EnqueueMessage {
    pub queue: String,
    pub headers: HashMap<String, String>,
    pub payload: Vec<u8>,
}

#[derive(Debug, Clone)]
pub struct EnqueueRequest {
    pub messages: Vec<EnqueueMessage>,
}

#[derive(Debug, Clone)]
pub struct EnqueueResultItem {
    pub error_code: ErrorCode,
    pub message_id: String,
}

#[derive(Debug, Clone)]
pub struct EnqueueResponse {
    pub results: Vec<EnqueueResultItem>,
}

#[derive(Debug, Clone)]
pub struct ConsumeRequest {
    pub queue: String,
}

#[derive(Debug, Clone)]
pub struct DeliveryMessage {
    pub message_id: String,
    pub queue: String,
    pub headers: HashMap<String, String>,
    pub payload: Vec<u8>,
    pub fairness_key: String,
    pub weight: u32,
    pub throttle_keys: Vec<String>,
    pub attempt_count: u32,
    pub enqueued_at: u64,
    pub leased_at: u64,
}

#[derive(Debug, Clone)]
pub struct DeliveryBatch {
    pub messages: Vec<DeliveryMessage>,
}

#[derive(Debug, Clone)]
pub struct AckItem {
    pub queue: String,
    pub message_id: String,
}

#[derive(Debug, Clone)]
pub struct AckRequest {
    pub items: Vec<AckItem>,
}

#[derive(Debug, Clone)]
pub struct AckResultItem {
    pub error_code: ErrorCode,
}

#[derive(Debug, Clone)]
pub struct AckResponse {
    pub results: Vec<AckResultItem>,
}

#[derive(Debug, Clone)]
pub struct NackItem {
    pub queue: String,
    pub message_id: String,
    pub error: String,
}

#[derive(Debug, Clone)]
pub struct NackRequest {
    pub items: Vec<NackItem>,
}

#[derive(Debug, Clone)]
pub struct NackResultItem {
    pub error_code: ErrorCode,
}

#[derive(Debug, Clone)]
pub struct NackResponse {
    pub results: Vec<NackResultItem>,
}

// ---------------------------------------------------------------------------
// Admin frames
// ---------------------------------------------------------------------------

#[derive(Debug, Clone)]
pub struct CreateQueueRequest {
    pub name: String,
    pub on_enqueue_script: Option<String>,
    pub on_failure_script: Option<String>,
    pub visibility_timeout_ms: u64,
}

#[derive(Debug, Clone)]
pub struct CreateQueueResponse {
    pub error_code: ErrorCode,
    pub queue_id: String,
}

#[derive(Debug, Clone)]
pub struct DeleteQueueRequest {
    pub queue: String,
}

#[derive(Debug, Clone)]
pub struct DeleteQueueResponse {
    pub error_code: ErrorCode,
}

#[derive(Debug, Clone)]
pub struct GetStatsRequest {
    pub queue: String,
}

#[derive(Debug, Clone)]
pub struct FairnessKeyStat {
    pub key: String,
    pub pending_count: u64,
    pub current_deficit: i64,
    pub weight: u32,
}

#[derive(Debug, Clone)]
pub struct ThrottleKeyStat {
    pub key: String,
    pub tokens: f64,
    pub rate_per_second: f64,
    pub burst: f64,
}

#[derive(Debug, Clone)]
pub struct GetStatsResponse {
    pub error_code: ErrorCode,
    pub depth: u64,
    pub in_flight: u64,
    pub active_fairness_keys: u64,
    pub active_consumers: u32,
    pub quantum: u32,
    pub leader_node_id: u64,
    pub replication_count: u32,
    pub per_key_stats: Vec<FairnessKeyStat>,
    pub per_throttle_stats: Vec<ThrottleKeyStat>,
}

#[derive(Debug, Clone)]
pub struct ListQueuesRequest;

#[derive(Debug, Clone)]
pub struct QueueInfo {
    pub name: String,
    pub depth: u64,
    pub in_flight: u64,
    pub active_consumers: u32,
    pub leader_node_id: u64,
}

#[derive(Debug, Clone)]
pub struct ListQueuesResponse {
    pub error_code: ErrorCode,
    pub cluster_node_count: u32,
    pub queues: Vec<QueueInfo>,
}

#[derive(Debug, Clone)]
pub struct SetConfigRequest {
    pub key: String,
    pub value: String,
}

#[derive(Debug, Clone)]
pub struct SetConfigResponse {
    pub error_code: ErrorCode,
}

#[derive(Debug, Clone)]
pub struct GetConfigRequest {
    pub key: String,
}

#[derive(Debug, Clone)]
pub struct GetConfigResponse {
    pub error_code: ErrorCode,
    pub value: String,
}

#[derive(Debug, Clone)]
pub struct ListConfigRequest {
    pub prefix: String,
}

#[derive(Debug, Clone)]
pub struct ConfigEntry {
    pub key: String,
    pub value: String,
}

#[derive(Debug, Clone)]
pub struct ListConfigResponse {
    pub error_code: ErrorCode,
    pub entries: Vec<ConfigEntry>,
}

#[derive(Debug, Clone)]
pub struct RedriveRequest {
    pub dlq_queue: String,
    pub count: u64,
}

#[derive(Debug, Clone)]
pub struct RedriveResponse {
    pub error_code: ErrorCode,
    pub redriven: u64,
}

// ---------------------------------------------------------------------------
// Auth & ACL frames
// ---------------------------------------------------------------------------

#[derive(Debug, Clone)]
pub struct CreateApiKeyRequest {
    pub name: String,
    pub expires_at_ms: u64,
    pub is_superadmin: bool,
}

#[derive(Debug, Clone)]
pub struct CreateApiKeyResponse {
    pub error_code: ErrorCode,
    pub key_id: String,
    pub key: String,
    pub is_superadmin: bool,
}

#[derive(Debug, Clone)]
pub struct RevokeApiKeyRequest {
    pub key_id: String,
}

#[derive(Debug, Clone)]
pub struct RevokeApiKeyResponse {
    pub error_code: ErrorCode,
}

#[derive(Debug, Clone)]
pub struct ListApiKeysRequest;

#[derive(Debug, Clone)]
pub struct ApiKeyInfo {
    pub key_id: String,
    pub name: String,
    pub created_at_ms: u64,
    pub expires_at_ms: u64,
    pub is_superadmin: bool,
}

#[derive(Debug, Clone)]
pub struct ListApiKeysResponse {
    pub error_code: ErrorCode,
    pub keys: Vec<ApiKeyInfo>,
}

#[derive(Debug, Clone)]
pub struct AclPermission {
    pub kind: String,
    pub pattern: String,
}

#[derive(Debug, Clone)]
pub struct SetAclRequest {
    pub key_id: String,
    pub permissions: Vec<AclPermission>,
}

#[derive(Debug, Clone)]
pub struct SetAclResponse {
    pub error_code: ErrorCode,
}

#[derive(Debug, Clone)]
pub struct GetAclRequest {
    pub key_id: String,
}

#[derive(Debug, Clone)]
pub struct GetAclResponse {
    pub error_code: ErrorCode,
    pub key_id: String,
    pub is_superadmin: bool,
    pub permissions: Vec<AclPermission>,
}

// ---------------------------------------------------------------------------
// Error frame
// ---------------------------------------------------------------------------

#[derive(Debug, Clone)]
pub struct ErrorFrame {
    pub error_code: ErrorCode,
    pub message: String,
    pub metadata: HashMap<String, String>,
}

// ---------------------------------------------------------------------------
// Encode implementations
// ---------------------------------------------------------------------------

impl Handshake {
    pub fn encode(&self, request_id: u32) -> RawFrame {
        let mut w = PayloadWriter::new();
        w.put_u16(self.protocol_version);
        w.put_optional_string(&self.api_key);
        RawFrame {
            opcode: Opcode::Handshake as u8,
            flags: 0,
            request_id,
            payload: w.finish(),
        }
    }

    pub fn decode(payload: Bytes) -> Result<Self, FrameError> {
        let mut r = PayloadReader::new(payload);
        let protocol_version = r.read_u16()?;
        let api_key = r.read_optional_string()?;
        Ok(Self {
            protocol_version,
            api_key,
        })
    }
}

impl HandshakeOk {
    pub fn encode(&self, request_id: u32) -> RawFrame {
        let mut w = PayloadWriter::new();
        w.put_u16(self.negotiated_version);
        w.put_u64(self.node_id);
        w.put_u32(self.max_frame_size);
        RawFrame {
            opcode: Opcode::HandshakeOk as u8,
            flags: 0,
            request_id,
            payload: w.finish(),
        }
    }

    pub fn decode(payload: Bytes) -> Result<Self, FrameError> {
        let mut r = PayloadReader::new(payload);
        let negotiated_version = r.read_u16()?;
        let node_id = r.read_u64()?;
        let max_frame_size = r.read_u32()?;
        Ok(Self {
            negotiated_version,
            node_id,
            max_frame_size,
        })
    }
}

impl EnqueueRequest {
    pub fn encode(&self, request_id: u32) -> RawFrame {
        let mut w = PayloadWriter::with_capacity(256 * self.messages.len());
        w.put_u32(self.messages.len() as u32);
        for msg in &self.messages {
            w.put_string(&msg.queue);
            w.put_string_map(&msg.headers);
            w.put_bytes(&msg.payload);
        }
        RawFrame {
            opcode: Opcode::Enqueue as u8,
            flags: 0,
            request_id,
            payload: w.finish(),
        }
    }

    pub fn decode(payload: Bytes) -> Result<Self, FrameError> {
        let mut r = PayloadReader::new(payload);
        let count = r.read_u32()? as usize;
        check_count(count)?;
        let mut messages = Vec::with_capacity(count);
        for _ in 0..count {
            let queue = r.read_string()?;
            let headers = r.read_string_map()?;
            let payload = r.read_bytes()?;
            messages.push(EnqueueMessage {
                queue,
                headers,
                payload,
            });
        }
        Ok(Self { messages })
    }
}

impl EnqueueResponse {
    pub fn encode(&self, request_id: u32) -> RawFrame {
        let mut w = PayloadWriter::new();
        w.put_u32(self.results.len() as u32);
        for item in &self.results {
            w.put_u8(item.error_code as u8);
            w.put_string(&item.message_id);
        }
        RawFrame {
            opcode: Opcode::EnqueueResult as u8,
            flags: 0,
            request_id,
            payload: w.finish(),
        }
    }

    pub fn decode(payload: Bytes) -> Result<Self, FrameError> {
        let mut r = PayloadReader::new(payload);
        let count = r.read_u32()? as usize;
        check_count(count)?;
        let mut results = Vec::with_capacity(count);
        for _ in 0..count {
            let error_code = ErrorCode::from_u8(r.read_u8()?);
            let message_id = r.read_string()?;
            results.push(EnqueueResultItem {
                error_code,
                message_id,
            });
        }
        Ok(Self { results })
    }
}

impl ConsumeRequest {
    pub fn encode(&self, request_id: u32) -> RawFrame {
        let mut w = PayloadWriter::new();
        w.put_string(&self.queue);
        RawFrame {
            opcode: Opcode::Consume as u8,
            flags: 0,
            request_id,
            payload: w.finish(),
        }
    }

    pub fn decode(payload: Bytes) -> Result<Self, FrameError> {
        let mut r = PayloadReader::new(payload);
        let queue = r.read_string()?;
        Ok(Self { queue })
    }
}

impl DeliveryBatch {
    pub fn encode(&self, request_id: u32) -> RawFrame {
        let mut w = PayloadWriter::with_capacity(256 * self.messages.len());
        w.put_u32(self.messages.len() as u32);
        for msg in &self.messages {
            w.put_string(&msg.message_id);
            w.put_string(&msg.queue);
            w.put_string_map(&msg.headers);
            w.put_bytes(&msg.payload);
            w.put_string(&msg.fairness_key);
            w.put_u32(msg.weight);
            w.put_string_array(&msg.throttle_keys);
            w.put_u32(msg.attempt_count);
            w.put_u64(msg.enqueued_at);
            w.put_u64(msg.leased_at);
        }
        RawFrame {
            opcode: Opcode::Delivery as u8,
            flags: 0,
            request_id,
            payload: w.finish(),
        }
    }

    pub fn decode(payload: Bytes) -> Result<Self, FrameError> {
        let mut r = PayloadReader::new(payload);
        let count = r.read_u32()? as usize;
        check_count(count)?;
        let mut messages = Vec::with_capacity(count);
        for _ in 0..count {
            let message_id = r.read_string()?;
            let queue = r.read_string()?;
            let headers = r.read_string_map()?;
            let payload = r.read_bytes()?;
            let fairness_key = r.read_string()?;
            let weight = r.read_u32()?;
            let throttle_keys = r.read_string_array()?;
            let attempt_count = r.read_u32()?;
            let enqueued_at = r.read_u64()?;
            let leased_at = r.read_u64()?;
            messages.push(DeliveryMessage {
                message_id,
                queue,
                headers,
                payload,
                fairness_key,
                weight,
                throttle_keys,
                attempt_count,
                enqueued_at,
                leased_at,
            });
        }
        Ok(Self { messages })
    }
}

impl AckRequest {
    pub fn encode(&self, request_id: u32) -> RawFrame {
        let mut w = PayloadWriter::new();
        w.put_u32(self.items.len() as u32);
        for item in &self.items {
            w.put_string(&item.queue);
            w.put_string(&item.message_id);
        }
        RawFrame {
            opcode: Opcode::Ack as u8,
            flags: 0,
            request_id,
            payload: w.finish(),
        }
    }

    pub fn decode(payload: Bytes) -> Result<Self, FrameError> {
        let mut r = PayloadReader::new(payload);
        let count = r.read_u32()? as usize;
        check_count(count)?;
        let mut items = Vec::with_capacity(count);
        for _ in 0..count {
            let queue = r.read_string()?;
            let message_id = r.read_string()?;
            items.push(AckItem { queue, message_id });
        }
        Ok(Self { items })
    }
}

impl AckResponse {
    pub fn encode(&self, request_id: u32) -> RawFrame {
        let mut w = PayloadWriter::new();
        w.put_u32(self.results.len() as u32);
        for item in &self.results {
            w.put_u8(item.error_code as u8);
        }
        RawFrame {
            opcode: Opcode::AckResult as u8,
            flags: 0,
            request_id,
            payload: w.finish(),
        }
    }

    pub fn decode(payload: Bytes) -> Result<Self, FrameError> {
        let mut r = PayloadReader::new(payload);
        let count = r.read_u32()? as usize;
        check_count(count)?;
        let mut results = Vec::with_capacity(count);
        for _ in 0..count {
            let error_code = ErrorCode::from_u8(r.read_u8()?);
            results.push(AckResultItem { error_code });
        }
        Ok(Self { results })
    }
}

impl NackRequest {
    pub fn encode(&self, request_id: u32) -> RawFrame {
        let mut w = PayloadWriter::new();
        w.put_u32(self.items.len() as u32);
        for item in &self.items {
            w.put_string(&item.queue);
            w.put_string(&item.message_id);
            w.put_string(&item.error);
        }
        RawFrame {
            opcode: Opcode::Nack as u8,
            flags: 0,
            request_id,
            payload: w.finish(),
        }
    }

    pub fn decode(payload: Bytes) -> Result<Self, FrameError> {
        let mut r = PayloadReader::new(payload);
        let count = r.read_u32()? as usize;
        check_count(count)?;
        let mut items = Vec::with_capacity(count);
        for _ in 0..count {
            let queue = r.read_string()?;
            let message_id = r.read_string()?;
            let error = r.read_string()?;
            items.push(NackItem {
                queue,
                message_id,
                error,
            });
        }
        Ok(Self { items })
    }
}

impl NackResponse {
    pub fn encode(&self, request_id: u32) -> RawFrame {
        let mut w = PayloadWriter::new();
        w.put_u32(self.results.len() as u32);
        for item in &self.results {
            w.put_u8(item.error_code as u8);
        }
        RawFrame {
            opcode: Opcode::NackResult as u8,
            flags: 0,
            request_id,
            payload: w.finish(),
        }
    }

    pub fn decode(payload: Bytes) -> Result<Self, FrameError> {
        let mut r = PayloadReader::new(payload);
        let count = r.read_u32()? as usize;
        check_count(count)?;
        let mut results = Vec::with_capacity(count);
        for _ in 0..count {
            let error_code = ErrorCode::from_u8(r.read_u8()?);
            results.push(NackResultItem { error_code });
        }
        Ok(Self { results })
    }
}

// ---------------------------------------------------------------------------
// Admin encode/decode implementations
// ---------------------------------------------------------------------------

impl CreateQueueRequest {
    pub fn encode(&self, request_id: u32) -> RawFrame {
        let mut w = PayloadWriter::new();
        w.put_string(&self.name);
        w.put_optional_string(&self.on_enqueue_script);
        w.put_optional_string(&self.on_failure_script);
        w.put_u64(self.visibility_timeout_ms);
        RawFrame {
            opcode: Opcode::CreateQueue as u8,
            flags: 0,
            request_id,
            payload: w.finish(),
        }
    }

    pub fn decode(payload: Bytes) -> Result<Self, FrameError> {
        let mut r = PayloadReader::new(payload);
        let name = r.read_string()?;
        let on_enqueue_script = r.read_optional_string()?;
        let on_failure_script = r.read_optional_string()?;
        let visibility_timeout_ms = r.read_u64()?;
        Ok(Self {
            name,
            on_enqueue_script,
            on_failure_script,
            visibility_timeout_ms,
        })
    }
}

impl CreateQueueResponse {
    pub fn encode(&self, request_id: u32) -> RawFrame {
        let mut w = PayloadWriter::new();
        w.put_u8(self.error_code as u8);
        w.put_string(&self.queue_id);
        RawFrame {
            opcode: Opcode::CreateQueueResult as u8,
            flags: 0,
            request_id,
            payload: w.finish(),
        }
    }

    pub fn decode(payload: Bytes) -> Result<Self, FrameError> {
        let mut r = PayloadReader::new(payload);
        let error_code = ErrorCode::from_u8(r.read_u8()?);
        let queue_id = r.read_string()?;
        Ok(Self {
            error_code,
            queue_id,
        })
    }
}

impl DeleteQueueRequest {
    pub fn encode(&self, request_id: u32) -> RawFrame {
        let mut w = PayloadWriter::new();
        w.put_string(&self.queue);
        RawFrame {
            opcode: Opcode::DeleteQueue as u8,
            flags: 0,
            request_id,
            payload: w.finish(),
        }
    }

    pub fn decode(payload: Bytes) -> Result<Self, FrameError> {
        let mut r = PayloadReader::new(payload);
        let queue = r.read_string()?;
        Ok(Self { queue })
    }
}

impl DeleteQueueResponse {
    pub fn encode(&self, request_id: u32) -> RawFrame {
        let mut w = PayloadWriter::new();
        w.put_u8(self.error_code as u8);
        RawFrame {
            opcode: Opcode::DeleteQueueResult as u8,
            flags: 0,
            request_id,
            payload: w.finish(),
        }
    }

    pub fn decode(payload: Bytes) -> Result<Self, FrameError> {
        let mut r = PayloadReader::new(payload);
        let error_code = ErrorCode::from_u8(r.read_u8()?);
        Ok(Self { error_code })
    }
}

impl GetStatsRequest {
    pub fn encode(&self, request_id: u32) -> RawFrame {
        let mut w = PayloadWriter::new();
        w.put_string(&self.queue);
        RawFrame {
            opcode: Opcode::GetStats as u8,
            flags: 0,
            request_id,
            payload: w.finish(),
        }
    }

    pub fn decode(payload: Bytes) -> Result<Self, FrameError> {
        let mut r = PayloadReader::new(payload);
        let queue = r.read_string()?;
        Ok(Self { queue })
    }
}

impl GetStatsResponse {
    pub fn encode(&self, request_id: u32) -> RawFrame {
        let mut w = PayloadWriter::new();
        w.put_u8(self.error_code as u8);
        w.put_u64(self.depth);
        w.put_u64(self.in_flight);
        w.put_u64(self.active_fairness_keys);
        w.put_u32(self.active_consumers);
        w.put_u32(self.quantum);
        w.put_u64(self.leader_node_id);
        w.put_u32(self.replication_count);
        assert!(
            self.per_key_stats.len() <= u16::MAX as usize,
            "per_key_stats count exceeds u16::MAX"
        );
        w.put_u16(self.per_key_stats.len() as u16);
        for s in &self.per_key_stats {
            w.put_string(&s.key);
            w.put_u64(s.pending_count);
            w.put_i64(s.current_deficit);
            w.put_u32(s.weight);
        }
        assert!(
            self.per_throttle_stats.len() <= u16::MAX as usize,
            "per_throttle_stats count exceeds u16::MAX"
        );
        w.put_u16(self.per_throttle_stats.len() as u16);
        for s in &self.per_throttle_stats {
            w.put_string(&s.key);
            w.put_f64(s.tokens);
            w.put_f64(s.rate_per_second);
            w.put_f64(s.burst);
        }
        RawFrame {
            opcode: Opcode::GetStatsResult as u8,
            flags: 0,
            request_id,
            payload: w.finish(),
        }
    }

    pub fn decode(payload: Bytes) -> Result<Self, FrameError> {
        let mut r = PayloadReader::new(payload);
        let error_code = ErrorCode::from_u8(r.read_u8()?);
        let depth = r.read_u64()?;
        let in_flight = r.read_u64()?;
        let active_fairness_keys = r.read_u64()?;
        let active_consumers = r.read_u32()?;
        let quantum = r.read_u32()?;
        let leader_node_id = r.read_u64()?;
        let replication_count = r.read_u32()?;
        let per_key_count = r.read_u16()? as usize;
        check_count(per_key_count)?;
        let mut per_key_stats = Vec::with_capacity(per_key_count);
        for _ in 0..per_key_count {
            let key = r.read_string()?;
            let pending_count = r.read_u64()?;
            let current_deficit = r.read_i64()?;
            let weight = r.read_u32()?;
            per_key_stats.push(FairnessKeyStat {
                key,
                pending_count,
                current_deficit,
                weight,
            });
        }
        let per_throttle_count = r.read_u16()? as usize;
        check_count(per_throttle_count)?;
        let mut per_throttle_stats = Vec::with_capacity(per_throttle_count);
        for _ in 0..per_throttle_count {
            let key = r.read_string()?;
            let tokens = r.read_f64()?;
            let rate_per_second = r.read_f64()?;
            let burst = r.read_f64()?;
            per_throttle_stats.push(ThrottleKeyStat {
                key,
                tokens,
                rate_per_second,
                burst,
            });
        }
        Ok(Self {
            error_code,
            depth,
            in_flight,
            active_fairness_keys,
            active_consumers,
            quantum,
            leader_node_id,
            replication_count,
            per_key_stats,
            per_throttle_stats,
        })
    }
}

impl ListQueuesRequest {
    pub fn encode(&self, request_id: u32) -> RawFrame {
        RawFrame {
            opcode: Opcode::ListQueues as u8,
            flags: 0,
            request_id,
            payload: Bytes::new(),
        }
    }

    pub fn decode(_payload: Bytes) -> Result<Self, FrameError> {
        Ok(Self)
    }
}

impl ListQueuesResponse {
    pub fn encode(&self, request_id: u32) -> RawFrame {
        let mut w = PayloadWriter::new();
        w.put_u8(self.error_code as u8);
        w.put_u32(self.cluster_node_count);
        w.put_u16(self.queues.len() as u16);
        for q in &self.queues {
            w.put_string(&q.name);
            w.put_u64(q.depth);
            w.put_u64(q.in_flight);
            w.put_u32(q.active_consumers);
            w.put_u64(q.leader_node_id);
        }
        RawFrame {
            opcode: Opcode::ListQueuesResult as u8,
            flags: 0,
            request_id,
            payload: w.finish(),
        }
    }

    pub fn decode(payload: Bytes) -> Result<Self, FrameError> {
        let mut r = PayloadReader::new(payload);
        let error_code = ErrorCode::from_u8(r.read_u8()?);
        let cluster_node_count = r.read_u32()?;
        let queue_count = r.read_u16()? as usize;
        check_count(queue_count)?;
        let mut queues = Vec::with_capacity(queue_count);
        for _ in 0..queue_count {
            let name = r.read_string()?;
            let depth = r.read_u64()?;
            let in_flight = r.read_u64()?;
            let active_consumers = r.read_u32()?;
            let leader_node_id = r.read_u64()?;
            queues.push(QueueInfo {
                name,
                depth,
                in_flight,
                active_consumers,
                leader_node_id,
            });
        }
        Ok(Self {
            error_code,
            cluster_node_count,
            queues,
        })
    }
}

impl SetConfigRequest {
    pub fn encode(&self, request_id: u32) -> RawFrame {
        let mut w = PayloadWriter::new();
        w.put_string(&self.key);
        w.put_string(&self.value);
        RawFrame {
            opcode: Opcode::SetConfig as u8,
            flags: 0,
            request_id,
            payload: w.finish(),
        }
    }

    pub fn decode(payload: Bytes) -> Result<Self, FrameError> {
        let mut r = PayloadReader::new(payload);
        let key = r.read_string()?;
        let value = r.read_string()?;
        Ok(Self { key, value })
    }
}

impl SetConfigResponse {
    pub fn encode(&self, request_id: u32) -> RawFrame {
        let mut w = PayloadWriter::new();
        w.put_u8(self.error_code as u8);
        RawFrame {
            opcode: Opcode::SetConfigResult as u8,
            flags: 0,
            request_id,
            payload: w.finish(),
        }
    }

    pub fn decode(payload: Bytes) -> Result<Self, FrameError> {
        let mut r = PayloadReader::new(payload);
        let error_code = ErrorCode::from_u8(r.read_u8()?);
        Ok(Self { error_code })
    }
}

impl GetConfigRequest {
    pub fn encode(&self, request_id: u32) -> RawFrame {
        let mut w = PayloadWriter::new();
        w.put_string(&self.key);
        RawFrame {
            opcode: Opcode::GetConfig as u8,
            flags: 0,
            request_id,
            payload: w.finish(),
        }
    }

    pub fn decode(payload: Bytes) -> Result<Self, FrameError> {
        let mut r = PayloadReader::new(payload);
        let key = r.read_string()?;
        Ok(Self { key })
    }
}

impl GetConfigResponse {
    pub fn encode(&self, request_id: u32) -> RawFrame {
        let mut w = PayloadWriter::new();
        w.put_u8(self.error_code as u8);
        w.put_string(&self.value);
        RawFrame {
            opcode: Opcode::GetConfigResult as u8,
            flags: 0,
            request_id,
            payload: w.finish(),
        }
    }

    pub fn decode(payload: Bytes) -> Result<Self, FrameError> {
        let mut r = PayloadReader::new(payload);
        let error_code = ErrorCode::from_u8(r.read_u8()?);
        let value = r.read_string()?;
        Ok(Self { error_code, value })
    }
}

impl ListConfigRequest {
    pub fn encode(&self, request_id: u32) -> RawFrame {
        let mut w = PayloadWriter::new();
        w.put_string(&self.prefix);
        RawFrame {
            opcode: Opcode::ListConfig as u8,
            flags: 0,
            request_id,
            payload: w.finish(),
        }
    }

    pub fn decode(payload: Bytes) -> Result<Self, FrameError> {
        let mut r = PayloadReader::new(payload);
        let prefix = r.read_string()?;
        Ok(Self { prefix })
    }
}

impl ListConfigResponse {
    pub fn encode(&self, request_id: u32) -> RawFrame {
        let mut w = PayloadWriter::new();
        w.put_u8(self.error_code as u8);
        w.put_u16(self.entries.len() as u16);
        for entry in &self.entries {
            w.put_string(&entry.key);
            w.put_string(&entry.value);
        }
        RawFrame {
            opcode: Opcode::ListConfigResult as u8,
            flags: 0,
            request_id,
            payload: w.finish(),
        }
    }

    pub fn decode(payload: Bytes) -> Result<Self, FrameError> {
        let mut r = PayloadReader::new(payload);
        let error_code = ErrorCode::from_u8(r.read_u8()?);
        let count = r.read_u16()? as usize;
        check_count(count)?;
        let mut entries = Vec::with_capacity(count);
        for _ in 0..count {
            let key = r.read_string()?;
            let value = r.read_string()?;
            entries.push(ConfigEntry { key, value });
        }
        Ok(Self {
            error_code,
            entries,
        })
    }
}

impl RedriveRequest {
    pub fn encode(&self, request_id: u32) -> RawFrame {
        let mut w = PayloadWriter::new();
        w.put_string(&self.dlq_queue);
        w.put_u64(self.count);
        RawFrame {
            opcode: Opcode::Redrive as u8,
            flags: 0,
            request_id,
            payload: w.finish(),
        }
    }

    pub fn decode(payload: Bytes) -> Result<Self, FrameError> {
        let mut r = PayloadReader::new(payload);
        let dlq_queue = r.read_string()?;
        let count = r.read_u64()?;
        Ok(Self { dlq_queue, count })
    }
}

impl RedriveResponse {
    pub fn encode(&self, request_id: u32) -> RawFrame {
        let mut w = PayloadWriter::new();
        w.put_u8(self.error_code as u8);
        w.put_u64(self.redriven);
        RawFrame {
            opcode: Opcode::RedriveResult as u8,
            flags: 0,
            request_id,
            payload: w.finish(),
        }
    }

    pub fn decode(payload: Bytes) -> Result<Self, FrameError> {
        let mut r = PayloadReader::new(payload);
        let error_code = ErrorCode::from_u8(r.read_u8()?);
        let redriven = r.read_u64()?;
        Ok(Self {
            error_code,
            redriven,
        })
    }
}

// ---------------------------------------------------------------------------
// Auth & ACL encode/decode implementations
// ---------------------------------------------------------------------------

impl CreateApiKeyRequest {
    pub fn encode(&self, request_id: u32) -> RawFrame {
        let mut w = PayloadWriter::new();
        w.put_string(&self.name);
        w.put_u64(self.expires_at_ms);
        w.put_bool(self.is_superadmin);
        RawFrame {
            opcode: Opcode::CreateApiKey as u8,
            flags: 0,
            request_id,
            payload: w.finish(),
        }
    }

    pub fn decode(payload: Bytes) -> Result<Self, FrameError> {
        let mut r = PayloadReader::new(payload);
        let name = r.read_string()?;
        let expires_at_ms = r.read_u64()?;
        let is_superadmin = r.read_bool()?;
        Ok(Self {
            name,
            expires_at_ms,
            is_superadmin,
        })
    }
}

impl CreateApiKeyResponse {
    pub fn encode(&self, request_id: u32) -> RawFrame {
        let mut w = PayloadWriter::new();
        w.put_u8(self.error_code as u8);
        w.put_string(&self.key_id);
        w.put_string(&self.key);
        w.put_bool(self.is_superadmin);
        RawFrame {
            opcode: Opcode::CreateApiKeyResult as u8,
            flags: 0,
            request_id,
            payload: w.finish(),
        }
    }

    pub fn decode(payload: Bytes) -> Result<Self, FrameError> {
        let mut r = PayloadReader::new(payload);
        let error_code = ErrorCode::from_u8(r.read_u8()?);
        let key_id = r.read_string()?;
        let key = r.read_string()?;
        let is_superadmin = r.read_bool()?;
        Ok(Self {
            error_code,
            key_id,
            key,
            is_superadmin,
        })
    }
}

impl RevokeApiKeyRequest {
    pub fn encode(&self, request_id: u32) -> RawFrame {
        let mut w = PayloadWriter::new();
        w.put_string(&self.key_id);
        RawFrame {
            opcode: Opcode::RevokeApiKey as u8,
            flags: 0,
            request_id,
            payload: w.finish(),
        }
    }

    pub fn decode(payload: Bytes) -> Result<Self, FrameError> {
        let mut r = PayloadReader::new(payload);
        let key_id = r.read_string()?;
        Ok(Self { key_id })
    }
}

impl RevokeApiKeyResponse {
    pub fn encode(&self, request_id: u32) -> RawFrame {
        let mut w = PayloadWriter::new();
        w.put_u8(self.error_code as u8);
        RawFrame {
            opcode: Opcode::RevokeApiKeyResult as u8,
            flags: 0,
            request_id,
            payload: w.finish(),
        }
    }

    pub fn decode(payload: Bytes) -> Result<Self, FrameError> {
        let mut r = PayloadReader::new(payload);
        let error_code = ErrorCode::from_u8(r.read_u8()?);
        Ok(Self { error_code })
    }
}

impl ListApiKeysRequest {
    pub fn encode(&self, request_id: u32) -> RawFrame {
        RawFrame {
            opcode: Opcode::ListApiKeys as u8,
            flags: 0,
            request_id,
            payload: Bytes::new(),
        }
    }

    pub fn decode(_payload: Bytes) -> Result<Self, FrameError> {
        Ok(Self)
    }
}

impl ListApiKeysResponse {
    pub fn encode(&self, request_id: u32) -> RawFrame {
        let mut w = PayloadWriter::new();
        w.put_u8(self.error_code as u8);
        w.put_u16(self.keys.len() as u16);
        for k in &self.keys {
            w.put_string(&k.key_id);
            w.put_string(&k.name);
            w.put_u64(k.created_at_ms);
            w.put_u64(k.expires_at_ms);
            w.put_bool(k.is_superadmin);
        }
        RawFrame {
            opcode: Opcode::ListApiKeysResult as u8,
            flags: 0,
            request_id,
            payload: w.finish(),
        }
    }

    pub fn decode(payload: Bytes) -> Result<Self, FrameError> {
        let mut r = PayloadReader::new(payload);
        let error_code = ErrorCode::from_u8(r.read_u8()?);
        let count = r.read_u16()? as usize;
        check_count(count)?;
        let mut keys = Vec::with_capacity(count);
        for _ in 0..count {
            let key_id = r.read_string()?;
            let name = r.read_string()?;
            let created_at_ms = r.read_u64()?;
            let expires_at_ms = r.read_u64()?;
            let is_superadmin = r.read_bool()?;
            keys.push(ApiKeyInfo {
                key_id,
                name,
                created_at_ms,
                expires_at_ms,
                is_superadmin,
            });
        }
        Ok(Self { error_code, keys })
    }
}

impl SetAclRequest {
    pub fn encode(&self, request_id: u32) -> RawFrame {
        let mut w = PayloadWriter::new();
        w.put_string(&self.key_id);
        w.put_u16(self.permissions.len() as u16);
        for p in &self.permissions {
            w.put_string(&p.kind);
            w.put_string(&p.pattern);
        }
        RawFrame {
            opcode: Opcode::SetAcl as u8,
            flags: 0,
            request_id,
            payload: w.finish(),
        }
    }

    pub fn decode(payload: Bytes) -> Result<Self, FrameError> {
        let mut r = PayloadReader::new(payload);
        let key_id = r.read_string()?;
        let count = r.read_u16()? as usize;
        check_count(count)?;
        let mut permissions = Vec::with_capacity(count);
        for _ in 0..count {
            let kind = r.read_string()?;
            let pattern = r.read_string()?;
            permissions.push(AclPermission { kind, pattern });
        }
        Ok(Self {
            key_id,
            permissions,
        })
    }
}

impl SetAclResponse {
    pub fn encode(&self, request_id: u32) -> RawFrame {
        let mut w = PayloadWriter::new();
        w.put_u8(self.error_code as u8);
        RawFrame {
            opcode: Opcode::SetAclResult as u8,
            flags: 0,
            request_id,
            payload: w.finish(),
        }
    }

    pub fn decode(payload: Bytes) -> Result<Self, FrameError> {
        let mut r = PayloadReader::new(payload);
        let error_code = ErrorCode::from_u8(r.read_u8()?);
        Ok(Self { error_code })
    }
}

impl GetAclRequest {
    pub fn encode(&self, request_id: u32) -> RawFrame {
        let mut w = PayloadWriter::new();
        w.put_string(&self.key_id);
        RawFrame {
            opcode: Opcode::GetAcl as u8,
            flags: 0,
            request_id,
            payload: w.finish(),
        }
    }

    pub fn decode(payload: Bytes) -> Result<Self, FrameError> {
        let mut r = PayloadReader::new(payload);
        let key_id = r.read_string()?;
        Ok(Self { key_id })
    }
}

impl GetAclResponse {
    pub fn encode(&self, request_id: u32) -> RawFrame {
        let mut w = PayloadWriter::new();
        w.put_u8(self.error_code as u8);
        w.put_string(&self.key_id);
        w.put_bool(self.is_superadmin);
        w.put_u16(self.permissions.len() as u16);
        for p in &self.permissions {
            w.put_string(&p.kind);
            w.put_string(&p.pattern);
        }
        RawFrame {
            opcode: Opcode::GetAclResult as u8,
            flags: 0,
            request_id,
            payload: w.finish(),
        }
    }

    pub fn decode(payload: Bytes) -> Result<Self, FrameError> {
        let mut r = PayloadReader::new(payload);
        let error_code = ErrorCode::from_u8(r.read_u8()?);
        let key_id = r.read_string()?;
        let is_superadmin = r.read_bool()?;
        let count = r.read_u16()? as usize;
        check_count(count)?;
        let mut permissions = Vec::with_capacity(count);
        for _ in 0..count {
            let kind = r.read_string()?;
            let pattern = r.read_string()?;
            permissions.push(AclPermission { kind, pattern });
        }
        Ok(Self {
            error_code,
            key_id,
            is_superadmin,
            permissions,
        })
    }
}

impl ErrorFrame {
    pub fn encode(&self, request_id: u32) -> RawFrame {
        let mut w = PayloadWriter::new();
        w.put_u8(self.error_code as u8);
        w.put_string(&self.message);
        w.put_string_map(&self.metadata);
        RawFrame {
            opcode: Opcode::Error as u8,
            flags: 0,
            request_id,
            payload: w.finish(),
        }
    }

    pub fn decode(payload: Bytes) -> Result<Self, FrameError> {
        let mut r = PayloadReader::new(payload);
        let error_code = ErrorCode::from_u8(r.read_u8()?);
        let message = r.read_string()?;
        let metadata = r.read_string_map()?;
        Ok(Self {
            error_code,
            message,
            metadata,
        })
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use uuid::Uuid;

    #[test]
    fn handshake_round_trip() {
        let hs = Handshake {
            protocol_version: 1,
            api_key: Some("secret-key".to_string()),
        };
        let frame = hs.encode(0);
        let decoded = Handshake::decode(frame.payload).unwrap();
        assert_eq!(decoded.protocol_version, 1);
        assert_eq!(decoded.api_key, Some("secret-key".to_string()));
    }

    #[test]
    fn handshake_ok_round_trip() {
        let ok = HandshakeOk {
            negotiated_version: 1,
            node_id: 42,
            max_frame_size: 0,
        };
        let frame = ok.encode(0);
        let decoded = HandshakeOk::decode(frame.payload).unwrap();
        assert_eq!(decoded.negotiated_version, 1);
        assert_eq!(decoded.node_id, 42);
        assert_eq!(decoded.max_frame_size, 0);
    }

    #[test]
    fn enqueue_round_trip() {
        let mut headers = HashMap::new();
        headers.insert("type".to_string(), "order".to_string());

        let req = EnqueueRequest {
            messages: vec![
                EnqueueMessage {
                    queue: "orders".to_string(),
                    headers: headers.clone(),
                    payload: vec![1, 2, 3, 4],
                },
                EnqueueMessage {
                    queue: "events".to_string(),
                    headers: HashMap::new(),
                    payload: vec![5, 6],
                },
            ],
        };

        let frame = req.encode(1);
        assert_eq!(frame.opcode, Opcode::Enqueue as u8);
        assert_eq!(frame.request_id, 1);

        let decoded = EnqueueRequest::decode(frame.payload).unwrap();
        assert_eq!(decoded.messages.len(), 2);
        assert_eq!(decoded.messages[0].queue, "orders");
        assert_eq!(decoded.messages[0].headers.get("type").unwrap(), "order");
        assert_eq!(decoded.messages[0].payload, vec![1, 2, 3, 4]);
        assert_eq!(decoded.messages[1].queue, "events");
    }

    #[test]
    fn enqueue_response_round_trip() {
        let resp = EnqueueResponse {
            results: vec![
                EnqueueResultItem {
                    error_code: ErrorCode::Ok,
                    message_id: Uuid::nil().to_string(),
                },
                EnqueueResultItem {
                    error_code: ErrorCode::QueueNotFound,
                    message_id: String::new(),
                },
            ],
        };

        let frame = resp.encode(1);
        let decoded = EnqueueResponse::decode(frame.payload).unwrap();
        assert_eq!(decoded.results.len(), 2);
        assert_eq!(decoded.results[0].error_code, ErrorCode::Ok);
        assert_eq!(decoded.results[1].error_code, ErrorCode::QueueNotFound);
    }

    #[test]
    fn ack_round_trip() {
        let req = AckRequest {
            items: vec![AckItem {
                queue: "q1".to_string(),
                message_id: "abc-123".to_string(),
            }],
        };
        let frame = req.encode(5);
        let decoded = AckRequest::decode(frame.payload).unwrap();
        assert_eq!(decoded.items[0].queue, "q1");
        assert_eq!(decoded.items[0].message_id, "abc-123");
    }

    #[test]
    fn nack_round_trip() {
        let req = NackRequest {
            items: vec![NackItem {
                queue: "q1".to_string(),
                message_id: "abc-123".to_string(),
                error: "processing failed".to_string(),
            }],
        };
        let frame = req.encode(6);
        let decoded = NackRequest::decode(frame.payload).unwrap();
        assert_eq!(decoded.items[0].error, "processing failed");
    }

    #[test]
    fn delivery_round_trip() {
        let batch = DeliveryBatch {
            messages: vec![DeliveryMessage {
                message_id: "msg-1".to_string(),
                queue: "orders".to_string(),
                headers: HashMap::new(),
                payload: vec![10, 20, 30],
                fairness_key: "tenant-a".to_string(),
                weight: 1,
                throttle_keys: vec!["k1".to_string()],
                attempt_count: 1,
                enqueued_at: 1700000000000,
                leased_at: 1700000001000,
            }],
        };
        let frame = batch.encode(10);
        assert_eq!(frame.opcode, Opcode::Delivery as u8);

        let decoded = DeliveryBatch::decode(frame.payload).unwrap();
        assert_eq!(decoded.messages.len(), 1);
        assert_eq!(decoded.messages[0].message_id, "msg-1");
        assert_eq!(decoded.messages[0].fairness_key, "tenant-a");
        assert_eq!(decoded.messages[0].enqueued_at, 1700000000000);
    }

    #[test]
    fn create_queue_round_trip() {
        let req = CreateQueueRequest {
            name: "my-queue".to_string(),
            on_enqueue_script: Some("lua code".to_string()),
            on_failure_script: None,
            visibility_timeout_ms: 60000,
        };
        let frame = req.encode(1);
        assert_eq!(frame.opcode, Opcode::CreateQueue as u8);
        let decoded = CreateQueueRequest::decode(frame.payload).unwrap();
        assert_eq!(decoded.name, "my-queue");
        assert_eq!(decoded.on_enqueue_script, Some("lua code".to_string()));
        assert_eq!(decoded.on_failure_script, None);
        assert_eq!(decoded.visibility_timeout_ms, 60000);
    }

    #[test]
    fn create_queue_response_round_trip() {
        let resp = CreateQueueResponse {
            error_code: ErrorCode::Ok,
            queue_id: "my-queue".to_string(),
        };
        let frame = resp.encode(1);
        let decoded = CreateQueueResponse::decode(frame.payload).unwrap();
        assert_eq!(decoded.error_code, ErrorCode::Ok);
        assert_eq!(decoded.queue_id, "my-queue");
    }

    #[test]
    fn delete_queue_round_trip() {
        let req = DeleteQueueRequest {
            queue: "my-queue".to_string(),
        };
        let frame = req.encode(2);
        let decoded = DeleteQueueRequest::decode(frame.payload).unwrap();
        assert_eq!(decoded.queue, "my-queue");

        let resp = DeleteQueueResponse {
            error_code: ErrorCode::Ok,
        };
        let frame = resp.encode(2);
        let decoded = DeleteQueueResponse::decode(frame.payload).unwrap();
        assert_eq!(decoded.error_code, ErrorCode::Ok);
    }

    #[test]
    fn get_stats_round_trip() {
        let req = GetStatsRequest {
            queue: "q1".to_string(),
        };
        let frame = req.encode(3);
        let decoded = GetStatsRequest::decode(frame.payload).unwrap();
        assert_eq!(decoded.queue, "q1");

        let resp = GetStatsResponse {
            error_code: ErrorCode::Ok,
            depth: 100,
            in_flight: 5,
            active_fairness_keys: 3,
            active_consumers: 2,
            quantum: 10,
            leader_node_id: 1,
            replication_count: 3,
            per_key_stats: vec![FairnessKeyStat {
                key: "tenant-a".to_string(),
                pending_count: 50,
                current_deficit: -10,
                weight: 2,
            }],
            per_throttle_stats: vec![ThrottleKeyStat {
                key: "provider".to_string(),
                tokens: 9.5,
                rate_per_second: 10.0,
                burst: 20.0,
            }],
        };
        let frame = resp.encode(3);
        let decoded = GetStatsResponse::decode(frame.payload).unwrap();
        assert_eq!(decoded.depth, 100);
        assert_eq!(decoded.in_flight, 5);
        assert_eq!(decoded.per_key_stats.len(), 1);
        assert_eq!(decoded.per_key_stats[0].key, "tenant-a");
        assert_eq!(decoded.per_key_stats[0].current_deficit, -10);
        assert_eq!(decoded.per_throttle_stats.len(), 1);
        assert!((decoded.per_throttle_stats[0].tokens - 9.5).abs() < f64::EPSILON);
    }

    #[test]
    fn list_queues_round_trip() {
        let req = ListQueuesRequest;
        let frame = req.encode(4);
        assert_eq!(frame.opcode, Opcode::ListQueues as u8);
        ListQueuesRequest::decode(frame.payload).unwrap();

        let resp = ListQueuesResponse {
            error_code: ErrorCode::Ok,
            cluster_node_count: 3,
            queues: vec![QueueInfo {
                name: "q1".to_string(),
                depth: 10,
                in_flight: 2,
                active_consumers: 1,
                leader_node_id: 1,
            }],
        };
        let frame = resp.encode(4);
        let decoded = ListQueuesResponse::decode(frame.payload).unwrap();
        assert_eq!(decoded.cluster_node_count, 3);
        assert_eq!(decoded.queues.len(), 1);
        assert_eq!(decoded.queues[0].name, "q1");
    }

    #[test]
    fn set_get_config_round_trip() {
        let req = SetConfigRequest {
            key: "throttle.x".to_string(),
            value: "10.0,100.0".to_string(),
        };
        let frame = req.encode(5);
        let decoded = SetConfigRequest::decode(frame.payload).unwrap();
        assert_eq!(decoded.key, "throttle.x");
        assert_eq!(decoded.value, "10.0,100.0");

        let resp = SetConfigResponse {
            error_code: ErrorCode::Ok,
        };
        let frame = resp.encode(5);
        let decoded = SetConfigResponse::decode(frame.payload).unwrap();
        assert_eq!(decoded.error_code, ErrorCode::Ok);

        let req = GetConfigRequest {
            key: "throttle.x".to_string(),
        };
        let frame = req.encode(6);
        let decoded = GetConfigRequest::decode(frame.payload).unwrap();
        assert_eq!(decoded.key, "throttle.x");

        let resp = GetConfigResponse {
            error_code: ErrorCode::Ok,
            value: "10.0,100.0".to_string(),
        };
        let frame = resp.encode(6);
        let decoded = GetConfigResponse::decode(frame.payload).unwrap();
        assert_eq!(decoded.value, "10.0,100.0");
    }

    #[test]
    fn list_config_round_trip() {
        let req = ListConfigRequest {
            prefix: "throttle.".to_string(),
        };
        let frame = req.encode(7);
        let decoded = ListConfigRequest::decode(frame.payload).unwrap();
        assert_eq!(decoded.prefix, "throttle.");

        let resp = ListConfigResponse {
            error_code: ErrorCode::Ok,
            entries: vec![
                ConfigEntry {
                    key: "throttle.a".to_string(),
                    value: "1.0".to_string(),
                },
                ConfigEntry {
                    key: "throttle.b".to_string(),
                    value: "2.0".to_string(),
                },
            ],
        };
        let frame = resp.encode(7);
        let decoded = ListConfigResponse::decode(frame.payload).unwrap();
        assert_eq!(decoded.entries.len(), 2);
        assert_eq!(decoded.entries[0].key, "throttle.a");
    }

    #[test]
    fn redrive_round_trip() {
        let req = RedriveRequest {
            dlq_queue: "q.dlq".to_string(),
            count: 50,
        };
        let frame = req.encode(8);
        let decoded = RedriveRequest::decode(frame.payload).unwrap();
        assert_eq!(decoded.dlq_queue, "q.dlq");
        assert_eq!(decoded.count, 50);

        let resp = RedriveResponse {
            error_code: ErrorCode::Ok,
            redriven: 42,
        };
        let frame = resp.encode(8);
        let decoded = RedriveResponse::decode(frame.payload).unwrap();
        assert_eq!(decoded.redriven, 42);
    }

    #[test]
    fn create_api_key_round_trip() {
        let req = CreateApiKeyRequest {
            name: "my-key".to_string(),
            expires_at_ms: 1700000000000,
            is_superadmin: true,
        };
        let frame = req.encode(10);
        let decoded = CreateApiKeyRequest::decode(frame.payload).unwrap();
        assert_eq!(decoded.name, "my-key");
        assert_eq!(decoded.expires_at_ms, 1700000000000);
        assert!(decoded.is_superadmin);

        let resp = CreateApiKeyResponse {
            error_code: ErrorCode::Ok,
            key_id: "kid-1".to_string(),
            key: "secret-token".to_string(),
            is_superadmin: true,
        };
        let frame = resp.encode(10);
        let decoded = CreateApiKeyResponse::decode(frame.payload).unwrap();
        assert_eq!(decoded.key_id, "kid-1");
        assert_eq!(decoded.key, "secret-token");
        assert!(decoded.is_superadmin);
    }

    #[test]
    fn revoke_api_key_round_trip() {
        let req = RevokeApiKeyRequest {
            key_id: "kid-1".to_string(),
        };
        let frame = req.encode(11);
        let decoded = RevokeApiKeyRequest::decode(frame.payload).unwrap();
        assert_eq!(decoded.key_id, "kid-1");

        let resp = RevokeApiKeyResponse {
            error_code: ErrorCode::Ok,
        };
        let frame = resp.encode(11);
        let decoded = RevokeApiKeyResponse::decode(frame.payload).unwrap();
        assert_eq!(decoded.error_code, ErrorCode::Ok);
    }

    #[test]
    fn list_api_keys_round_trip() {
        let req = ListApiKeysRequest;
        let frame = req.encode(12);
        ListApiKeysRequest::decode(frame.payload).unwrap();

        let resp = ListApiKeysResponse {
            error_code: ErrorCode::Ok,
            keys: vec![ApiKeyInfo {
                key_id: "kid-1".to_string(),
                name: "test-key".to_string(),
                created_at_ms: 1700000000000,
                expires_at_ms: 0,
                is_superadmin: false,
            }],
        };
        let frame = resp.encode(12);
        let decoded = ListApiKeysResponse::decode(frame.payload).unwrap();
        assert_eq!(decoded.keys.len(), 1);
        assert_eq!(decoded.keys[0].key_id, "kid-1");
        assert_eq!(decoded.keys[0].name, "test-key");
    }

    #[test]
    fn set_acl_round_trip() {
        let req = SetAclRequest {
            key_id: "kid-1".to_string(),
            permissions: vec![
                AclPermission {
                    kind: "produce".to_string(),
                    pattern: "orders.*".to_string(),
                },
                AclPermission {
                    kind: "admin".to_string(),
                    pattern: "*".to_string(),
                },
            ],
        };
        let frame = req.encode(13);
        let decoded = SetAclRequest::decode(frame.payload).unwrap();
        assert_eq!(decoded.key_id, "kid-1");
        assert_eq!(decoded.permissions.len(), 2);
        assert_eq!(decoded.permissions[0].kind, "produce");
        assert_eq!(decoded.permissions[0].pattern, "orders.*");

        let resp = SetAclResponse {
            error_code: ErrorCode::Ok,
        };
        let frame = resp.encode(13);
        let decoded = SetAclResponse::decode(frame.payload).unwrap();
        assert_eq!(decoded.error_code, ErrorCode::Ok);
    }

    #[test]
    fn get_acl_round_trip() {
        let req = GetAclRequest {
            key_id: "kid-1".to_string(),
        };
        let frame = req.encode(14);
        let decoded = GetAclRequest::decode(frame.payload).unwrap();
        assert_eq!(decoded.key_id, "kid-1");

        let resp = GetAclResponse {
            error_code: ErrorCode::Ok,
            key_id: "kid-1".to_string(),
            is_superadmin: false,
            permissions: vec![AclPermission {
                kind: "consume".to_string(),
                pattern: "*".to_string(),
            }],
        };
        let frame = resp.encode(14);
        let decoded = GetAclResponse::decode(frame.payload).unwrap();
        assert_eq!(decoded.key_id, "kid-1");
        assert!(!decoded.is_superadmin);
        assert_eq!(decoded.permissions.len(), 1);
    }

    #[test]
    fn error_frame_round_trip() {
        let mut metadata = HashMap::new();
        metadata.insert("leader_addr".to_string(), "127.0.0.1:5555".to_string());

        let err = ErrorFrame {
            error_code: ErrorCode::NotLeader,
            message: "not the leader".to_string(),
            metadata,
        };
        let frame = err.encode(99);
        let decoded = ErrorFrame::decode(frame.payload).unwrap();
        assert_eq!(decoded.error_code, ErrorCode::NotLeader);
        assert_eq!(decoded.message, "not the leader");
        assert_eq!(
            decoded.metadata.get("leader_addr").unwrap(),
            "127.0.0.1:5555"
        );
    }
}
