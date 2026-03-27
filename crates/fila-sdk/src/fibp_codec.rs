//! FIBP frame codec for the SDK client.
//!
//! Reimplements the FIBP wire format from fila-core so the SDK does not
//! depend on the full core crate. The wire format is intentionally simple:
//!
//! ```text
//! [4 bytes BE length] [flags:u8 | op:u8 | corr_id:u32 | payload...]
//! ```
//!
//! The 4-byte length covers everything after itself (header + payload).

use bytes::{Buf, BufMut, Bytes, BytesMut};
use tokio_util::codec::{Decoder, Encoder};

use crate::error::StatusError;

// ---------------------------------------------------------------------------
// Protocol constants
// ---------------------------------------------------------------------------

/// Protocol magic bytes: `FIBP` + major version 1, minor version 0.
pub const MAGIC: &[u8; 6] = b"FIBP\x01\x00";

/// Frame header length after the 4-byte length prefix:
/// flags (1) + op (1) + correlation_id (4) = 6 bytes.
const FRAME_HEADER_LEN: usize = 6;

// Op codes
pub const OP_ENQUEUE: u8 = 0x01;
pub const OP_CONSUME: u8 = 0x02;
pub const OP_ACK: u8 = 0x03;
pub const OP_NACK: u8 = 0x04;
pub const OP_CREATE_QUEUE: u8 = 0x10;
pub const OP_DELETE_QUEUE: u8 = 0x11;
pub const OP_QUEUE_STATS: u8 = 0x12;
pub const OP_LIST_QUEUES: u8 = 0x13;
pub const OP_REDRIVE: u8 = 0x14;
pub const OP_CONFIG_SET: u8 = 0x15;
pub const OP_CONFIG_GET: u8 = 0x16;
pub const OP_CONFIG_LIST: u8 = 0x17;
pub const OP_FLOW: u8 = 0x20;
pub const _OP_HEARTBEAT: u8 = 0x21;
pub const OP_AUTH: u8 = 0x30;
pub const OP_AUTH_CREATE_KEY: u8 = 0x31;
pub const OP_AUTH_REVOKE_KEY: u8 = 0x32;
pub const OP_AUTH_LIST_KEYS: u8 = 0x33;
pub const OP_AUTH_SET_ACL: u8 = 0x34;
pub const OP_AUTH_GET_ACL: u8 = 0x35;
pub const OP_ERROR: u8 = 0xFE;
pub const OP_GOAWAY: u8 = 0xFF;

// Flags
pub const FLAG_STREAM: u8 = 0x04;

// ---------------------------------------------------------------------------
// Frame
// ---------------------------------------------------------------------------

/// A decoded FIBP frame.
#[derive(Debug, Clone)]
pub struct Frame {
    pub flags: u8,
    pub op: u8,
    pub correlation_id: u32,
    pub payload: Bytes,
}

impl Frame {
    pub fn new(flags: u8, op: u8, correlation_id: u32, payload: Bytes) -> Self {
        Self {
            flags,
            op,
            correlation_id,
            payload,
        }
    }
}

// ---------------------------------------------------------------------------
// Codec
// ---------------------------------------------------------------------------

/// Length-delimited FIBP frame codec.
pub struct FibpCodec {
    max_frame_size: u32,
}

impl FibpCodec {
    pub fn new(max_frame_size: u32) -> Self {
        Self { max_frame_size }
    }
}

/// Codec error type for the SDK.
#[derive(Debug, thiserror::Error)]
pub enum CodecError {
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),

    #[error("frame too large: {size} bytes exceeds limit of {limit} bytes")]
    FrameTooLarge { size: u32, limit: u32 },

    #[error("invalid frame: {0}")]
    InvalidFrame(String),
}

impl Decoder for FibpCodec {
    type Item = Frame;
    type Error = CodecError;

    fn decode(&mut self, src: &mut BytesMut) -> Result<Option<Frame>, CodecError> {
        if src.len() < 4 {
            return Ok(None);
        }

        let frame_len = u32::from_be_bytes([src[0], src[1], src[2], src[3]]);

        if frame_len > self.max_frame_size {
            return Err(CodecError::FrameTooLarge {
                size: frame_len,
                limit: self.max_frame_size,
            });
        }

        if (frame_len as usize) < FRAME_HEADER_LEN {
            return Err(CodecError::InvalidFrame(format!(
                "frame body too short: {frame_len} bytes, need at least {FRAME_HEADER_LEN}"
            )));
        }

        let total = 4 + frame_len as usize;
        if src.len() < total {
            src.reserve(total - src.len());
            return Ok(None);
        }

        src.advance(4);

        let flags = src[0];
        let op = src[1];
        let correlation_id = u32::from_be_bytes([src[2], src[3], src[4], src[5]]);
        src.advance(FRAME_HEADER_LEN);

        let payload_len = frame_len as usize - FRAME_HEADER_LEN;
        let payload = src.split_to(payload_len).freeze();

        Ok(Some(Frame {
            flags,
            op,
            correlation_id,
            payload,
        }))
    }
}

impl Encoder<Frame> for FibpCodec {
    type Error = CodecError;

    fn encode(&mut self, frame: Frame, dst: &mut BytesMut) -> Result<(), CodecError> {
        let body_len = FRAME_HEADER_LEN + frame.payload.len();

        if body_len > self.max_frame_size as usize {
            return Err(CodecError::FrameTooLarge {
                size: body_len as u32,
                limit: self.max_frame_size,
            });
        }

        dst.reserve(4 + body_len);
        dst.put_u32(body_len as u32);
        dst.put_u8(frame.flags);
        dst.put_u8(frame.op);
        dst.put_u32(frame.correlation_id);
        dst.extend_from_slice(&frame.payload);

        Ok(())
    }
}

// ---------------------------------------------------------------------------
// Wire encoding helpers (client-side)
// ---------------------------------------------------------------------------

/// Encode an enqueue request payload.
pub fn encode_enqueue_request(queue: &str, messages: &[EnqueueWireMessage]) -> Bytes {
    let mut buf = BytesMut::new();
    write_string16(&mut buf, queue);
    buf.put_u16(messages.len() as u16);
    for msg in messages {
        buf.put_u8(msg.headers.len() as u8);
        for (k, v) in &msg.headers {
            write_string16(&mut buf, k);
            write_string16(&mut buf, v);
        }
        buf.put_u32(msg.payload.len() as u32);
        buf.extend_from_slice(&msg.payload);
    }
    buf.freeze()
}

/// A single message for wire encoding.
pub struct EnqueueWireMessage {
    pub headers: std::collections::HashMap<String, String>,
    pub payload: Vec<u8>,
}

/// Encode a consume request payload.
pub fn encode_consume_request(queue: &str, initial_credits: u32) -> Bytes {
    let mut buf = BytesMut::new();
    write_string16(&mut buf, queue);
    buf.put_u32(initial_credits);
    buf.freeze()
}

/// Encode a flow (credit update) payload.
pub fn encode_flow(credits: u32) -> Bytes {
    let mut buf = BytesMut::with_capacity(4);
    buf.put_u32(credits);
    buf.freeze()
}

/// Encode an ack request payload.
pub fn encode_ack_request(items: &[(&str, &str)]) -> Bytes {
    let mut buf = BytesMut::new();
    buf.put_u16(items.len() as u16);
    for (queue, msg_id) in items {
        write_string16(&mut buf, queue);
        write_string16(&mut buf, msg_id);
    }
    buf.freeze()
}

/// Encode a nack request payload.
pub fn encode_nack_request(items: &[(&str, &str, &str)]) -> Bytes {
    let mut buf = BytesMut::new();
    buf.put_u16(items.len() as u16);
    for (queue, msg_id, error) in items {
        write_string16(&mut buf, queue);
        write_string16(&mut buf, msg_id);
        write_string16(&mut buf, error);
    }
    buf.freeze()
}

// ---------------------------------------------------------------------------
// Wire decoding helpers (client-side — decodes server responses)
// ---------------------------------------------------------------------------

/// Enqueue error codes from the server.
pub mod enqueue_err {
    pub const QUEUE_NOT_FOUND: u16 = 1;
    pub const _STORAGE: u16 = 2;
}

/// Ack/Nack error codes from the server.
pub mod ack_nack_err {
    pub const MESSAGE_NOT_FOUND: u16 = 1;
    pub const _STORAGE: u16 = 2;
}

/// A single enqueue result from the server.
pub enum EnqueueResultItem {
    Ok { msg_id: String },
    Err { code: u16, message: String },
}

/// Decode an enqueue response payload.
pub fn decode_enqueue_response(mut buf: Bytes) -> Result<Vec<EnqueueResultItem>, StatusError> {
    let count = read_u16(&mut buf)? as usize;
    let mut results = Vec::with_capacity(count);
    for _ in 0..count {
        let tag = read_u8(&mut buf)?;
        if tag == 1 {
            let msg_id = read_string16(&mut buf)?;
            results.push(EnqueueResultItem::Ok { msg_id });
        } else {
            let code = read_u16(&mut buf)?;
            let message = read_string16(&mut buf)?;
            results.push(EnqueueResultItem::Err { code, message });
        }
    }
    Ok(results)
}

/// A single ack/nack result from the server.
pub enum AckNackResultItem {
    Ok,
    Err { code: u16, message: String },
}

/// Decode an ack/nack response payload.
pub fn decode_ack_nack_response(mut buf: Bytes) -> Result<Vec<AckNackResultItem>, StatusError> {
    let count = read_u16(&mut buf)? as usize;
    let mut results = Vec::with_capacity(count);
    for _ in 0..count {
        let tag = read_u8(&mut buf)?;
        if tag == 1 {
            results.push(AckNackResultItem::Ok);
        } else {
            let code = read_u16(&mut buf)?;
            let message = read_string16(&mut buf)?;
            results.push(AckNackResultItem::Err { code, message });
        }
    }
    Ok(results)
}

/// A consumed message from a push frame.
pub struct ConsumePushMessage {
    pub msg_id: String,
    pub fairness_key: String,
    pub attempt_count: u32,
    pub headers: std::collections::HashMap<String, String>,
    pub payload: Vec<u8>,
}

/// Decode a consume push payload (server -> client).
pub fn decode_consume_push(mut buf: Bytes) -> Result<Vec<ConsumePushMessage>, StatusError> {
    let count = read_u16(&mut buf)? as usize;
    let mut messages = Vec::with_capacity(count);
    for _ in 0..count {
        let msg_id = read_string16(&mut buf)?;
        let fairness_key = read_string16(&mut buf)?;
        let attempt_count = read_u32(&mut buf)?;
        let header_count = read_u8(&mut buf)? as usize;
        let mut headers = std::collections::HashMap::with_capacity(header_count);
        for _ in 0..header_count {
            let k = read_string16(&mut buf)?;
            let v = read_string16(&mut buf)?;
            headers.insert(k, v);
        }
        let payload_len = read_u32(&mut buf)? as usize;
        if buf.remaining() < payload_len {
            return Err(StatusError::Internal(format!(
                "payload length {payload_len} exceeds remaining {} bytes",
                buf.remaining()
            )));
        }
        let payload = buf.split_to(payload_len).to_vec();
        messages.push(ConsumePushMessage {
            msg_id,
            fairness_key,
            attempt_count,
            headers,
            payload,
        });
    }
    Ok(messages)
}

// ---------------------------------------------------------------------------
// Primitive readers / writers
// ---------------------------------------------------------------------------

fn read_u8(buf: &mut Bytes) -> Result<u8, StatusError> {
    if buf.remaining() < 1 {
        return Err(StatusError::Internal(
            "unexpected end of payload reading u8".into(),
        ));
    }
    Ok(buf.get_u8())
}

fn read_u16(buf: &mut Bytes) -> Result<u16, StatusError> {
    if buf.remaining() < 2 {
        return Err(StatusError::Internal(
            "unexpected end of payload reading u16".into(),
        ));
    }
    Ok(buf.get_u16())
}

fn read_u32(buf: &mut Bytes) -> Result<u32, StatusError> {
    if buf.remaining() < 4 {
        return Err(StatusError::Internal(
            "unexpected end of payload reading u32".into(),
        ));
    }
    Ok(buf.get_u32())
}

fn read_string16(buf: &mut Bytes) -> Result<String, StatusError> {
    let len = read_u16(buf)? as usize;
    if buf.remaining() < len {
        return Err(StatusError::Internal(format!(
            "string length {len} exceeds remaining {} bytes",
            buf.remaining()
        )));
    }
    let raw = buf.split_to(len);
    String::from_utf8(raw.to_vec()).map_err(|e| StatusError::Internal(format!("invalid utf8: {e}")))
}

fn write_string16(buf: &mut BytesMut, s: &str) {
    buf.put_u16(s.len() as u16);
    buf.extend_from_slice(s.as_bytes());
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trip_frame() {
        let mut codec = FibpCodec::new(16_777_216);
        let frame = Frame::new(0, OP_ENQUEUE, 42, Bytes::from_static(b"hello"));

        let mut buf = BytesMut::new();
        codec.encode(frame.clone(), &mut buf).unwrap();
        let decoded = codec.decode(&mut buf).unwrap().unwrap();

        assert_eq!(decoded.op, frame.op);
        assert_eq!(decoded.correlation_id, frame.correlation_id);
        assert_eq!(decoded.payload, frame.payload);
    }

    #[test]
    fn encode_decode_enqueue_request() {
        let msgs = vec![EnqueueWireMessage {
            headers: [("k".to_string(), "v".to_string())].into_iter().collect(),
            payload: b"data".to_vec(),
        }];
        let encoded = encode_enqueue_request("test-q", &msgs);
        // Just verify it doesn't panic and produces non-empty bytes.
        assert!(!encoded.is_empty());
    }

    #[test]
    fn encode_decode_ack_request() {
        let items = vec![("q1", "msg-1")];
        let encoded = encode_ack_request(&items);
        assert!(!encoded.is_empty());
    }

    #[test]
    fn encode_decode_nack_request() {
        let items = vec![("q1", "msg-1", "error")];
        let encoded = encode_nack_request(&items);
        assert!(!encoded.is_empty());
    }
}
