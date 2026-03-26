//! FIBP binary wire encoding and decoding for data operation payloads.
//!
//! Each data operation (enqueue, consume, ack, nack, flow) has a compact
//! binary representation defined here. All multi-byte integers are big-endian.
//! Strings are UTF-8 with a `u16` length prefix.

use std::collections::HashMap;

use bytes::{Buf, BufMut, Bytes, BytesMut};

use super::error::FibpError;

// ---------------------------------------------------------------------------
// Error codes used in response payloads
// ---------------------------------------------------------------------------

/// Enqueue error codes sent in FIBP enqueue responses.
pub mod enqueue_err {
    pub const QUEUE_NOT_FOUND: u16 = 1;
    pub const STORAGE: u16 = 2;
}

/// Ack/Nack error codes sent in FIBP ack/nack responses.
pub mod ack_nack_err {
    pub const MESSAGE_NOT_FOUND: u16 = 1;
    pub const STORAGE: u16 = 2;
}

// ---------------------------------------------------------------------------
// Enqueue request / response
// ---------------------------------------------------------------------------

/// A single message in an enqueue request.
#[derive(Debug, Clone)]
pub struct WireEnqueueMessage {
    pub headers: HashMap<String, String>,
    pub payload: Bytes,
}

/// Parsed enqueue request payload.
#[derive(Debug)]
pub struct EnqueueRequest {
    pub queue: String,
    pub messages: Vec<WireEnqueueMessage>,
}

/// Decode an enqueue request from the frame payload.
pub fn decode_enqueue_request(mut buf: Bytes) -> Result<EnqueueRequest, FibpError> {
    let queue = read_string16(&mut buf)?;
    let msg_count = read_u16(&mut buf)? as usize;

    let mut messages = Vec::with_capacity(msg_count);
    for _ in 0..msg_count {
        let header_count = read_u8(&mut buf)? as usize;
        let mut headers = HashMap::with_capacity(header_count);
        for _ in 0..header_count {
            let key = read_string16(&mut buf)?;
            let val = read_string16(&mut buf)?;
            headers.insert(key, val);
        }
        let payload_len = read_u32(&mut buf)? as usize;
        if buf.remaining() < payload_len {
            return Err(FibpError::InvalidPayload {
                reason: format!(
                    "payload length {payload_len} exceeds remaining {} bytes",
                    buf.remaining()
                ),
            });
        }
        let payload = buf.split_to(payload_len);
        messages.push(WireEnqueueMessage { headers, payload });
    }

    Ok(EnqueueRequest { queue, messages })
}

/// A single result in the enqueue response.
pub enum EnqueueResultItem {
    Ok { msg_id: String },
    Err { code: u16, message: String },
}

/// Encode an enqueue response payload.
pub fn encode_enqueue_response(results: &[EnqueueResultItem]) -> Bytes {
    let mut buf = BytesMut::new();
    buf.put_u16(results.len() as u16);
    for item in results {
        match item {
            EnqueueResultItem::Ok { msg_id } => {
                buf.put_u8(1); // success
                write_string16(&mut buf, msg_id);
            }
            EnqueueResultItem::Err { code, message } => {
                buf.put_u8(0); // error
                buf.put_u16(*code);
                write_string16(&mut buf, message);
            }
        }
    }
    buf.freeze()
}

// ---------------------------------------------------------------------------
// Consume request
// ---------------------------------------------------------------------------

/// Parsed consume request payload.
#[derive(Debug)]
pub struct ConsumeRequest {
    pub queue: String,
    pub initial_credits: u32,
}

/// Decode a consume request from the frame payload.
pub fn decode_consume_request(mut buf: Bytes) -> Result<ConsumeRequest, FibpError> {
    let queue = read_string16(&mut buf)?;
    let initial_credits = read_u32(&mut buf)?;
    Ok(ConsumeRequest {
        queue,
        initial_credits,
    })
}

// ---------------------------------------------------------------------------
// Consume push frame (server → client)
// ---------------------------------------------------------------------------

/// Encode a batch of ready messages as a consume push payload.
pub fn encode_consume_push(messages: &[ConsumePushMessage]) -> Bytes {
    let mut buf = BytesMut::new();
    buf.put_u16(messages.len() as u16);
    for msg in messages {
        write_string16(&mut buf, &msg.msg_id);
        write_string16(&mut buf, &msg.fairness_key);
        buf.put_u32(msg.attempt_count);
        // headers
        buf.put_u8(msg.headers.len() as u8);
        for (k, v) in &msg.headers {
            write_string16(&mut buf, k);
            write_string16(&mut buf, v);
        }
        // payload
        buf.put_u32(msg.payload.len() as u32);
        buf.extend_from_slice(&msg.payload);
    }
    buf.freeze()
}

/// A message prepared for the consume push frame.
pub struct ConsumePushMessage {
    pub msg_id: String,
    pub fairness_key: String,
    pub attempt_count: u32,
    pub headers: HashMap<String, String>,
    pub payload: Bytes,
}

// ---------------------------------------------------------------------------
// Flow (credit update)
// ---------------------------------------------------------------------------

/// Decode a flow (credit) frame payload.
pub fn decode_flow(mut buf: Bytes) -> Result<u32, FibpError> {
    read_u32(&mut buf)
}

// ---------------------------------------------------------------------------
// Ack request / response
// ---------------------------------------------------------------------------

/// A single item in an ack request.
#[derive(Debug)]
pub struct AckItem {
    pub queue: String,
    pub msg_id: String,
}

/// Decode an ack request payload.
pub fn decode_ack_request(mut buf: Bytes) -> Result<Vec<AckItem>, FibpError> {
    let count = read_u16(&mut buf)? as usize;
    let mut items = Vec::with_capacity(count);
    for _ in 0..count {
        let queue = read_string16(&mut buf)?;
        let msg_id = read_string16(&mut buf)?;
        items.push(AckItem { queue, msg_id });
    }
    Ok(items)
}

/// A single item in a nack request.
#[derive(Debug)]
pub struct NackItem {
    pub queue: String,
    pub msg_id: String,
    pub error: String,
}

/// Decode a nack request payload.
pub fn decode_nack_request(mut buf: Bytes) -> Result<Vec<NackItem>, FibpError> {
    let count = read_u16(&mut buf)? as usize;
    let mut items = Vec::with_capacity(count);
    for _ in 0..count {
        let queue = read_string16(&mut buf)?;
        let msg_id = read_string16(&mut buf)?;
        let error = read_string16(&mut buf)?;
        items.push(NackItem {
            queue,
            msg_id,
            error,
        });
    }
    Ok(items)
}

/// A single result in the ack/nack response.
pub enum AckNackResultItem {
    Ok,
    Err { code: u16, message: String },
}

/// Encode an ack or nack response payload.
pub fn encode_ack_nack_response(results: &[AckNackResultItem]) -> Bytes {
    let mut buf = BytesMut::new();
    buf.put_u16(results.len() as u16);
    for item in results {
        match item {
            AckNackResultItem::Ok => {
                buf.put_u8(1);
            }
            AckNackResultItem::Err { code, message } => {
                buf.put_u8(0);
                buf.put_u16(*code);
                write_string16(&mut buf, message);
            }
        }
    }
    buf.freeze()
}

// ---------------------------------------------------------------------------
// Primitive readers / writers
// ---------------------------------------------------------------------------

fn read_u8(buf: &mut Bytes) -> Result<u8, FibpError> {
    if buf.remaining() < 1 {
        return Err(FibpError::InvalidPayload {
            reason: "unexpected end of payload reading u8".into(),
        });
    }
    Ok(buf.get_u8())
}

fn read_u16(buf: &mut Bytes) -> Result<u16, FibpError> {
    if buf.remaining() < 2 {
        return Err(FibpError::InvalidPayload {
            reason: "unexpected end of payload reading u16".into(),
        });
    }
    Ok(buf.get_u16())
}

fn read_u32(buf: &mut Bytes) -> Result<u32, FibpError> {
    if buf.remaining() < 4 {
        return Err(FibpError::InvalidPayload {
            reason: "unexpected end of payload reading u32".into(),
        });
    }
    Ok(buf.get_u32())
}

fn read_string16(buf: &mut Bytes) -> Result<String, FibpError> {
    let len = read_u16(buf)? as usize;
    if buf.remaining() < len {
        return Err(FibpError::InvalidPayload {
            reason: format!(
                "string length {len} exceeds remaining {} bytes",
                buf.remaining()
            ),
        });
    }
    let raw = buf.split_to(len);
    String::from_utf8(raw.to_vec()).map_err(|e| FibpError::InvalidPayload {
        reason: format!("invalid utf8: {e}"),
    })
}

fn write_string16(buf: &mut BytesMut, s: &str) {
    buf.put_u16(s.len() as u16);
    buf.extend_from_slice(s.as_bytes());
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trip_enqueue_request() {
        let mut buf = BytesMut::new();
        let queue = "test-queue";
        write_string16(&mut buf, queue);
        buf.put_u16(2); // 2 messages

        // Message 1: one header, small payload
        buf.put_u8(1); // header_count
        write_string16(&mut buf, "key1");
        write_string16(&mut buf, "val1");
        buf.put_u32(5);
        buf.extend_from_slice(b"hello");

        // Message 2: no headers, empty payload
        buf.put_u8(0);
        buf.put_u32(0);

        let req = decode_enqueue_request(buf.freeze()).unwrap();
        assert_eq!(req.queue, "test-queue");
        assert_eq!(req.messages.len(), 2);
        assert_eq!(req.messages[0].headers.get("key1").unwrap(), "val1");
        assert_eq!(&req.messages[0].payload[..], b"hello");
        assert!(req.messages[1].headers.is_empty());
        assert!(req.messages[1].payload.is_empty());
    }

    #[test]
    fn round_trip_enqueue_response() {
        let results = vec![
            EnqueueResultItem::Ok {
                msg_id: "abc-123".to_string(),
            },
            EnqueueResultItem::Err {
                code: enqueue_err::QUEUE_NOT_FOUND,
                message: "queue not found".to_string(),
            },
        ];
        let encoded = encode_enqueue_response(&results);
        let mut buf = encoded;

        let count = buf.get_u16();
        assert_eq!(count, 2);

        // First result: success
        assert_eq!(buf.get_u8(), 1);
        let id_len = buf.get_u16() as usize;
        let id = String::from_utf8(buf.split_to(id_len).to_vec()).unwrap();
        assert_eq!(id, "abc-123");

        // Second result: error
        assert_eq!(buf.get_u8(), 0);
        assert_eq!(buf.get_u16(), enqueue_err::QUEUE_NOT_FOUND);
        let msg_len = buf.get_u16() as usize;
        let msg = String::from_utf8(buf.split_to(msg_len).to_vec()).unwrap();
        assert_eq!(msg, "queue not found");
    }

    #[test]
    fn round_trip_ack_request() {
        let mut buf = BytesMut::new();
        buf.put_u16(1); // 1 item
        write_string16(&mut buf, "q1");
        write_string16(&mut buf, "msg-1");

        let items = decode_ack_request(buf.freeze()).unwrap();
        assert_eq!(items.len(), 1);
        assert_eq!(items[0].queue, "q1");
        assert_eq!(items[0].msg_id, "msg-1");
    }

    #[test]
    fn round_trip_nack_request() {
        let mut buf = BytesMut::new();
        buf.put_u16(1);
        write_string16(&mut buf, "q1");
        write_string16(&mut buf, "msg-1");
        write_string16(&mut buf, "processing failed");

        let items = decode_nack_request(buf.freeze()).unwrap();
        assert_eq!(items.len(), 1);
        assert_eq!(items[0].queue, "q1");
        assert_eq!(items[0].msg_id, "msg-1");
        assert_eq!(items[0].error, "processing failed");
    }

    #[test]
    fn round_trip_consume_request() {
        let mut buf = BytesMut::new();
        write_string16(&mut buf, "my-queue");
        buf.put_u32(100);

        let req = decode_consume_request(buf.freeze()).unwrap();
        assert_eq!(req.queue, "my-queue");
        assert_eq!(req.initial_credits, 100);
    }

    #[test]
    fn round_trip_flow() {
        let mut buf = BytesMut::new();
        buf.put_u32(42);
        let credits = decode_flow(buf.freeze()).unwrap();
        assert_eq!(credits, 42);
    }

    #[test]
    fn round_trip_consume_push() {
        let mut headers = HashMap::new();
        headers.insert("h1".to_string(), "v1".to_string());
        let messages = vec![ConsumePushMessage {
            msg_id: "id-1".to_string(),
            fairness_key: "default".to_string(),
            attempt_count: 3,
            headers,
            payload: Bytes::from_static(b"data"),
        }];
        let encoded = encode_consume_push(&messages);
        let mut buf = encoded;

        assert_eq!(buf.get_u16(), 1);

        // msg_id
        let id_len = buf.get_u16() as usize;
        let id = String::from_utf8(buf.split_to(id_len).to_vec()).unwrap();
        assert_eq!(id, "id-1");

        // fairness_key
        let fk_len = buf.get_u16() as usize;
        let fk = String::from_utf8(buf.split_to(fk_len).to_vec()).unwrap();
        assert_eq!(fk, "default");

        // attempt_count
        assert_eq!(buf.get_u32(), 3);

        // headers
        let hcount = buf.get_u8();
        assert_eq!(hcount, 1);
        let klen = buf.get_u16() as usize;
        let k = String::from_utf8(buf.split_to(klen).to_vec()).unwrap();
        let vlen = buf.get_u16() as usize;
        let v = String::from_utf8(buf.split_to(vlen).to_vec()).unwrap();
        assert_eq!(k, "h1");
        assert_eq!(v, "v1");

        // payload
        let plen = buf.get_u32() as usize;
        assert_eq!(&buf.split_to(plen)[..], b"data");
    }

    #[test]
    fn round_trip_ack_nack_response() {
        let results = vec![
            AckNackResultItem::Ok,
            AckNackResultItem::Err {
                code: ack_nack_err::MESSAGE_NOT_FOUND,
                message: "not found".to_string(),
            },
        ];
        let encoded = encode_ack_nack_response(&results);
        let mut buf = encoded;

        assert_eq!(buf.get_u16(), 2);

        // First: success
        assert_eq!(buf.get_u8(), 1);

        // Second: error
        assert_eq!(buf.get_u8(), 0);
        assert_eq!(buf.get_u16(), ack_nack_err::MESSAGE_NOT_FOUND);
        let msg_len = buf.get_u16() as usize;
        let msg = String::from_utf8(buf.split_to(msg_len).to_vec()).unwrap();
        assert_eq!(msg, "not found");
    }

    #[test]
    fn decode_truncated_enqueue_request_fails() {
        // Just a queue name, but says 5 messages follow with no data.
        let mut buf = BytesMut::new();
        write_string16(&mut buf, "q");
        buf.put_u16(5);
        let err = decode_enqueue_request(buf.freeze()).unwrap_err();
        assert!(
            matches!(err, FibpError::InvalidPayload { .. }),
            "expected InvalidPayload, got: {err:?}"
        );
    }
}
