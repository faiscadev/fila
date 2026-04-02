use std::collections::HashMap;

use bytes::Bytes;

use crate::error::FrameError;
use crate::error_code::ErrorCode;
use crate::frame::{PayloadReader, PayloadWriter, RawFrame};
use crate::opcode::Opcode;

use super::check_count;
use super::ProtocolMessage;

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
pub struct ConsumeOkResponse {
    pub consumer_id: String,
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
// Encode/decode implementations
// ---------------------------------------------------------------------------

impl ProtocolMessage for EnqueueRequest {
    fn encode(&self, request_id: u32) -> RawFrame {
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

    fn decode(payload: Bytes) -> Result<Self, FrameError> {
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

impl ProtocolMessage for EnqueueResponse {
    fn encode(&self, request_id: u32) -> RawFrame {
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

    fn decode(payload: Bytes) -> Result<Self, FrameError> {
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

impl ProtocolMessage for ConsumeRequest {
    fn encode(&self, request_id: u32) -> RawFrame {
        let mut w = PayloadWriter::new();
        w.put_string(&self.queue);
        RawFrame {
            opcode: Opcode::Consume as u8,
            flags: 0,
            request_id,
            payload: w.finish(),
        }
    }

    fn decode(payload: Bytes) -> Result<Self, FrameError> {
        let mut r = PayloadReader::new(payload);
        let queue = r.read_string()?;
        Ok(Self { queue })
    }
}

impl ProtocolMessage for ConsumeOkResponse {
    fn encode(&self, request_id: u32) -> RawFrame {
        let mut w = PayloadWriter::new();
        w.put_string(&self.consumer_id);
        RawFrame {
            opcode: Opcode::ConsumeOk as u8,
            flags: 0,
            request_id,
            payload: w.finish(),
        }
    }

    fn decode(payload: Bytes) -> Result<Self, FrameError> {
        let mut r = PayloadReader::new(payload);
        let consumer_id = r.read_string()?;
        Ok(Self { consumer_id })
    }
}

impl ProtocolMessage for DeliveryBatch {
    fn encode(&self, request_id: u32) -> RawFrame {
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

    fn decode(payload: Bytes) -> Result<Self, FrameError> {
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

impl ProtocolMessage for AckRequest {
    fn encode(&self, request_id: u32) -> RawFrame {
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

    fn decode(payload: Bytes) -> Result<Self, FrameError> {
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

impl ProtocolMessage for AckResponse {
    fn encode(&self, request_id: u32) -> RawFrame {
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

    fn decode(payload: Bytes) -> Result<Self, FrameError> {
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

impl ProtocolMessage for NackRequest {
    fn encode(&self, request_id: u32) -> RawFrame {
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

    fn decode(payload: Bytes) -> Result<Self, FrameError> {
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

impl ProtocolMessage for NackResponse {
    fn encode(&self, request_id: u32) -> RawFrame {
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

    fn decode(payload: Bytes) -> Result<Self, FrameError> {
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

#[cfg(test)]
mod tests {
    use super::*;
    use uuid::Uuid;

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
    fn consume_ok_round_trip() {
        let resp = ConsumeOkResponse {
            consumer_id: "consumer-abc-123".to_string(),
        };
        let frame = resp.encode(42);
        assert_eq!(frame.opcode, Opcode::ConsumeOk as u8);
        assert_eq!(frame.request_id, 42);

        let decoded = ConsumeOkResponse::decode(frame.payload).unwrap();
        assert_eq!(decoded.consumer_id, "consumer-abc-123");
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
}
