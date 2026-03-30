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
