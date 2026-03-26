//! FIBP frame codec for `tokio_util::codec`.
//!
//! Wire format (each frame):
//! ```text
//! [4 bytes big-endian length] [flags:u8 | op:u8 | corr_id:u32 | payload...]
//! ```
//!
//! The 4-byte length prefix covers everything *after* itself: header + payload.

use bytes::{Buf, BufMut, Bytes, BytesMut};
use tokio_util::codec::{Decoder, Encoder};

use super::error::FibpError;
use super::FRAME_HEADER_LEN;

// ---------------------------------------------------------------------------
// Op codes
// ---------------------------------------------------------------------------

/// Enqueue one or more messages.
pub const OP_ENQUEUE: u8 = 0x01;
/// Open a consume stream.
pub const OP_CONSUME: u8 = 0x02;
/// Acknowledge a delivered message.
pub const OP_ACK: u8 = 0x03;
/// Negative-acknowledge a delivered message.
pub const OP_NACK: u8 = 0x04;

/// Create a queue.
pub const OP_CREATE_QUEUE: u8 = 0x10;
/// Delete a queue.
pub const OP_DELETE_QUEUE: u8 = 0x11;
/// Get queue statistics.
pub const OP_QUEUE_STATS: u8 = 0x12;
/// List queues.
pub const OP_LIST_QUEUES: u8 = 0x13;
/// Pause a queue.
pub const OP_PAUSE_QUEUE: u8 = 0x14;
/// Resume a paused queue.
pub const OP_RESUME_QUEUE: u8 = 0x15;
/// Redrive messages from a dead-letter queue.
pub const OP_REDRIVE: u8 = 0x16;

/// Flow-control window update.
pub const OP_FLOW: u8 = 0x20;
/// Keepalive heartbeat (ping/pong).
pub const OP_HEARTBEAT: u8 = 0x21;

/// Authentication frame.
pub const OP_AUTH: u8 = 0x30;

/// Error response.
pub const OP_ERROR: u8 = 0xFE;
/// Graceful shutdown notification.
pub const OP_GOAWAY: u8 = 0xFF;

// ---------------------------------------------------------------------------
// Flags
// ---------------------------------------------------------------------------

/// Bit 2: marks a frame as a server-push (stream) frame.
pub const FLAG_STREAM: u8 = 0x04;

// ---------------------------------------------------------------------------
// Frame
// ---------------------------------------------------------------------------

/// A decoded FIBP frame.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Frame {
    pub flags: u8,
    pub op: u8,
    pub correlation_id: u32,
    pub payload: Bytes,
}

impl Frame {
    /// Create a new frame with the given fields.
    pub fn new(flags: u8, op: u8, correlation_id: u32, payload: Bytes) -> Self {
        Self {
            flags,
            op,
            correlation_id,
            payload,
        }
    }

    /// Convenience: build an error response frame for a given correlation id.
    pub fn error(correlation_id: u32, message: &str) -> Self {
        Self {
            flags: 0,
            op: OP_ERROR,
            correlation_id,
            payload: Bytes::copy_from_slice(message.as_bytes()),
        }
    }

    /// Convenience: build a GoAway frame.
    pub fn goaway(message: &str) -> Self {
        Self {
            flags: 0,
            op: OP_GOAWAY,
            correlation_id: 0,
            payload: Bytes::copy_from_slice(message.as_bytes()),
        }
    }
}

// ---------------------------------------------------------------------------
// Codec
// ---------------------------------------------------------------------------

/// Length-delimited FIBP frame codec.
///
/// Implements `Decoder<Item = Frame>` and `Encoder<Frame>` for use with
/// `tokio_util::codec::Framed`.
#[derive(Debug)]
pub struct FibpCodec {
    max_frame_size: u32,
}

impl FibpCodec {
    pub fn new(max_frame_size: u32) -> Self {
        Self { max_frame_size }
    }

    /// Return the configured maximum frame size.
    pub fn max_frame_size(&self) -> u32 {
        self.max_frame_size
    }
}

impl Decoder for FibpCodec {
    type Item = Frame;
    type Error = FibpError;

    fn decode(&mut self, src: &mut BytesMut) -> Result<Option<Frame>, FibpError> {
        // Need at least the 4-byte length prefix.
        if src.len() < 4 {
            return Ok(None);
        }

        // Peek the length without advancing the cursor.
        let frame_len = u32::from_be_bytes([src[0], src[1], src[2], src[3]]);

        // Enforce max frame size.
        if frame_len > self.max_frame_size {
            return Err(FibpError::FrameTooLarge {
                size: frame_len,
                limit: self.max_frame_size,
            });
        }

        // The frame body must contain at least the 6-byte header.
        if (frame_len as usize) < FRAME_HEADER_LEN {
            return Err(FibpError::Io(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                format!(
                    "frame body too short: {frame_len} bytes, need at least {FRAME_HEADER_LEN}"
                ),
            )));
        }

        let total = 4 + frame_len as usize;
        if src.len() < total {
            // Reserve space for the rest of the frame to reduce reallocations.
            src.reserve(total - src.len());
            return Ok(None);
        }

        // Consume the length prefix.
        src.advance(4);

        // Read the 6-byte header.
        let flags = src[0];
        let op = src[1];
        let correlation_id = u32::from_be_bytes([src[2], src[3], src[4], src[5]]);
        src.advance(FRAME_HEADER_LEN);

        // The remaining bytes are the payload.
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
    type Error = FibpError;

    fn encode(&mut self, frame: Frame, dst: &mut BytesMut) -> Result<(), FibpError> {
        let body_len = FRAME_HEADER_LEN + frame.payload.len();

        if body_len > self.max_frame_size as usize {
            return Err(FibpError::FrameTooLarge {
                size: body_len as u32,
                limit: self.max_frame_size,
            });
        }

        // Reserve space for the entire frame.
        dst.reserve(4 + body_len);

        // Length prefix (everything after this 4-byte field).
        dst.put_u32(body_len as u32);

        // 6-byte header.
        dst.put_u8(frame.flags);
        dst.put_u8(frame.op);
        dst.put_u32(frame.correlation_id);

        // Payload.
        dst.extend_from_slice(&frame.payload);

        Ok(())
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

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

        assert_eq!(decoded, frame);
    }

    #[test]
    fn round_trip_empty_payload() {
        let mut codec = FibpCodec::new(16_777_216);
        let frame = Frame::new(FLAG_STREAM, OP_HEARTBEAT, 0, Bytes::new());

        let mut buf = BytesMut::new();
        codec.encode(frame.clone(), &mut buf).unwrap();
        let decoded = codec.decode(&mut buf).unwrap().unwrap();

        assert_eq!(decoded, frame);
    }

    #[test]
    fn round_trip_large_correlation_id() {
        let mut codec = FibpCodec::new(16_777_216);
        let frame = Frame::new(0, OP_ACK, u32::MAX, Bytes::from_static(b"x"));

        let mut buf = BytesMut::new();
        codec.encode(frame.clone(), &mut buf).unwrap();
        let decoded = codec.decode(&mut buf).unwrap().unwrap();

        assert_eq!(decoded, frame);
    }

    #[test]
    fn decode_partial_length() {
        let mut codec = FibpCodec::new(16_777_216);
        let mut buf = BytesMut::from(&[0u8, 0][..]);
        assert!(codec.decode(&mut buf).unwrap().is_none());
    }

    #[test]
    fn decode_partial_body() {
        let mut codec = FibpCodec::new(16_777_216);
        let frame = Frame::new(0, OP_ENQUEUE, 1, Bytes::from_static(b"data"));

        let mut full = BytesMut::new();
        codec.encode(frame, &mut full).unwrap();

        // Feed only the first half.
        let mut partial = full.split_to(full.len() / 2);
        assert!(codec.decode(&mut partial).unwrap().is_none());
    }

    #[test]
    fn decode_oversized_frame() {
        let mut codec = FibpCodec::new(64);

        // Write a length prefix claiming 128 bytes.
        let mut buf = BytesMut::new();
        buf.put_u32(128);
        buf.put_bytes(0, 128);

        let err = codec.decode(&mut buf).unwrap_err();
        assert!(
            matches!(
                err,
                FibpError::FrameTooLarge {
                    size: 128,
                    limit: 64
                }
            ),
            "expected FrameTooLarge, got: {err:?}"
        );
    }

    #[test]
    fn encode_oversized_frame() {
        let mut codec = FibpCodec::new(8);
        let frame = Frame::new(0, OP_ENQUEUE, 1, Bytes::from(vec![0u8; 100]));
        let mut buf = BytesMut::new();
        let err = codec.encode(frame, &mut buf).unwrap_err();
        assert!(
            matches!(err, FibpError::FrameTooLarge { .. }),
            "expected FrameTooLarge, got: {err:?}"
        );
    }

    #[test]
    fn multiple_frames_in_buffer() {
        let mut codec = FibpCodec::new(16_777_216);
        let f1 = Frame::new(0, OP_ENQUEUE, 1, Bytes::from_static(b"one"));
        let f2 = Frame::new(0, OP_ACK, 2, Bytes::from_static(b"two"));

        let mut buf = BytesMut::new();
        codec.encode(f1.clone(), &mut buf).unwrap();
        codec.encode(f2.clone(), &mut buf).unwrap();

        let d1 = codec.decode(&mut buf).unwrap().unwrap();
        let d2 = codec.decode(&mut buf).unwrap().unwrap();
        assert_eq!(d1, f1);
        assert_eq!(d2, f2);
        assert!(codec.decode(&mut buf).unwrap().is_none());
    }

    #[test]
    fn error_frame_helper() {
        let f = Frame::error(99, "something went wrong");
        assert_eq!(f.op, OP_ERROR);
        assert_eq!(f.correlation_id, 99);
        assert_eq!(f.payload, Bytes::from_static(b"something went wrong"));
    }

    #[test]
    fn goaway_frame_helper() {
        let f = Frame::goaway("shutting down");
        assert_eq!(f.op, OP_GOAWAY);
        assert_eq!(f.correlation_id, 0);
        assert_eq!(f.payload, Bytes::from_static(b"shutting down"));
    }
}
