use std::collections::HashMap;

use bytes::{Buf, BufMut, Bytes, BytesMut};

use crate::error::FrameError;

/// Maximum frame size: 16 MiB.
pub const MAX_FRAME_SIZE: u32 = 16 * 1024 * 1024;

/// Fixed frame header size: opcode(1) + flags(1) + request_id(4) = 6 bytes.
pub const HEADER_SIZE: usize = 6;

/// Length prefix size: 4 bytes (u32 BE).
pub const LENGTH_PREFIX_SIZE: usize = 4;

/// Continuation flag (bit 0 of flags byte).
pub const FLAG_CONTINUATION: u8 = 0x01;

/// A raw frame read from the wire.
#[derive(Debug, Clone)]
pub struct RawFrame {
    pub opcode: u8,
    pub flags: u8,
    pub request_id: u32,
    pub payload: Bytes,
}

impl RawFrame {
    /// Returns true if the continuation flag is set.
    pub fn is_continuation(&self) -> bool {
        self.flags & FLAG_CONTINUATION != 0
    }

    /// Encode this frame into a BytesMut buffer (length-prefixed).
    pub fn encode(&self, buf: &mut BytesMut) {
        let body_len = HEADER_SIZE + self.payload.len();
        buf.reserve(LENGTH_PREFIX_SIZE + body_len);
        buf.put_u32(body_len as u32);
        buf.put_u8(self.opcode);
        buf.put_u8(self.flags);
        buf.put_u32(self.request_id);
        buf.extend_from_slice(&self.payload);
    }

    /// Try to decode a frame from a buffer. Returns None if not enough data.
    /// On success, advances the buffer past the consumed frame.
    pub fn decode(buf: &mut BytesMut) -> Result<Option<RawFrame>, FrameError> {
        if buf.len() < LENGTH_PREFIX_SIZE {
            return Ok(None);
        }

        let body_len = u32::from_be_bytes([buf[0], buf[1], buf[2], buf[3]]);

        if body_len > MAX_FRAME_SIZE {
            return Err(FrameError::FrameTooLarge {
                size: body_len,
                max: MAX_FRAME_SIZE,
            });
        }

        let total_len = LENGTH_PREFIX_SIZE + body_len as usize;
        if buf.len() < total_len {
            return Ok(None);
        }

        if (body_len as usize) < HEADER_SIZE {
            return Err(FrameError::IncompleteFrame {
                need: HEADER_SIZE,
                have: body_len as usize,
            });
        }

        // Consume the length prefix
        buf.advance(LENGTH_PREFIX_SIZE);

        let opcode = buf[0];
        let flags = buf[1];
        let request_id = u32::from_be_bytes([buf[2], buf[3], buf[4], buf[5]]);

        buf.advance(HEADER_SIZE);

        let payload_len = body_len as usize - HEADER_SIZE;
        let payload = buf.split_to(payload_len).freeze();

        Ok(Some(RawFrame {
            opcode,
            flags,
            request_id,
            payload,
        }))
    }
}

/// Default maximum reassembled payload size: 256 * MAX_FRAME_SIZE.
pub const DEFAULT_MAX_REASSEMBLED_SIZE: usize = 256 * MAX_FRAME_SIZE as usize;

/// In-progress reassembly state for a single request_id.
struct PendingReassembly {
    opcode: u8,
    /// Accumulated payload chunks (concatenated on completion).
    chunks: Vec<Bytes>,
    /// Running total of payload bytes accumulated so far.
    total_len: usize,
}

/// Tracks continuation frame reassembly across multiplexed request_ids.
///
/// When a frame arrives with `CONTINUATION=1`, its payload is buffered.
/// Subsequent frames for the same `request_id` append their payloads.
/// When a frame with `CONTINUATION=0` arrives for a tracked `request_id`,
/// the payloads are concatenated into a single assembled `RawFrame`.
///
/// Non-continuation frames (no prior state and `CONTINUATION=0`) pass
/// through unchanged.
/// Maximum number of concurrent in-flight continuation streams.
/// Prevents memory exhaustion from many unfinished request IDs.
pub const DEFAULT_MAX_PENDING_STREAMS: usize = 64;

pub struct ContinuationAssembler {
    pending: HashMap<u32, PendingReassembly>,
    max_reassembled_size: usize,
    max_pending_streams: usize,
}

impl ContinuationAssembler {
    pub fn new() -> Self {
        Self {
            pending: HashMap::new(),
            max_reassembled_size: DEFAULT_MAX_REASSEMBLED_SIZE,
            max_pending_streams: DEFAULT_MAX_PENDING_STREAMS,
        }
    }

    pub fn with_max_reassembled_size(mut self, max: usize) -> Self {
        self.max_reassembled_size = max;
        self
    }

    pub fn with_max_pending_streams(mut self, max: usize) -> Self {
        self.max_pending_streams = max;
        self
    }

    /// Feed a decoded frame into the assembler.
    ///
    /// Returns `Ok(Some(frame))` when a complete (possibly reassembled) frame
    /// is ready for dispatch. Returns `Ok(None)` when the frame was a
    /// continuation chunk that has been buffered.
    pub fn push_frame(&mut self, frame: RawFrame) -> Result<Option<RawFrame>, FrameError> {
        let is_continuation = frame.is_continuation();

        if self.pending.contains_key(&frame.request_id) {
            // We already have in-progress reassembly for this request_id.
            // Validate opcode consistency — peek the stored opcode before
            // taking a &mut borrow so we can remove on error cleanly.
            let initial_opcode = self.pending[&frame.request_id].opcode;

            if frame.opcode != initial_opcode {
                self.pending.remove(&frame.request_id);
                return Err(FrameError::ContinuationOpcodeMismatch {
                    request_id: frame.request_id,
                    initial: initial_opcode,
                    got: frame.opcode,
                });
            }

            let state = self.pending.get_mut(&frame.request_id).unwrap();

            let new_total = state.total_len + frame.payload.len();
            if new_total > self.max_reassembled_size {
                self.pending.remove(&frame.request_id);
                return Err(FrameError::ReassembledTooLarge {
                    request_id: frame.request_id,
                    size: new_total,
                    max: self.max_reassembled_size,
                });
            }

            state.total_len = new_total;
            state.chunks.push(frame.payload);

            if is_continuation {
                // Still accumulating.
                Ok(None)
            } else {
                // Final frame — assemble.
                let state = self.pending.remove(&frame.request_id).unwrap();
                let assembled_payload = concat_chunks(state.chunks, state.total_len);
                Ok(Some(RawFrame {
                    opcode: state.opcode,
                    flags: 0, // continuation cleared on assembled frame
                    request_id: frame.request_id,
                    payload: assembled_payload,
                }))
            }
        } else if is_continuation {
            // First frame of a new continuation sequence.
            if self.pending.len() >= self.max_pending_streams {
                return Err(FrameError::TooManyContinuationStreams {
                    count: self.pending.len() + 1,
                    max: self.max_pending_streams,
                });
            }
            let total_len = frame.payload.len();
            if total_len > self.max_reassembled_size {
                return Err(FrameError::ReassembledTooLarge {
                    request_id: frame.request_id,
                    size: total_len,
                    max: self.max_reassembled_size,
                });
            }
            self.pending.insert(
                frame.request_id,
                PendingReassembly {
                    opcode: frame.opcode,
                    chunks: vec![frame.payload],
                    total_len,
                },
            );
            Ok(None)
        } else {
            // Non-continuation, no pending state — pass through.
            Ok(Some(frame))
        }
    }

    /// Discard all in-progress reassembly state (e.g. on connection close).
    pub fn clear(&mut self) {
        self.pending.clear();
    }
}

impl Default for ContinuationAssembler {
    fn default() -> Self {
        Self::new()
    }
}

/// Concatenate a vec of Bytes chunks into a single Bytes.
fn concat_chunks(chunks: Vec<Bytes>, total_len: usize) -> Bytes {
    if chunks.len() == 1 {
        // Avoid copy when there's only one chunk.
        return chunks.into_iter().next().unwrap();
    }
    let mut buf = BytesMut::with_capacity(total_len);
    for chunk in chunks {
        buf.extend_from_slice(&chunk);
    }
    buf.freeze()
}

/// Helper for building frame payloads.
pub struct PayloadWriter {
    buf: BytesMut,
}

impl Default for PayloadWriter {
    fn default() -> Self {
        Self::new()
    }
}

impl PayloadWriter {
    pub fn new() -> Self {
        Self {
            buf: BytesMut::with_capacity(256),
        }
    }

    pub fn with_capacity(cap: usize) -> Self {
        Self {
            buf: BytesMut::with_capacity(cap),
        }
    }

    pub fn put_u8(&mut self, v: u8) {
        self.buf.put_u8(v);
    }

    pub fn put_u16(&mut self, v: u16) {
        self.buf.put_u16(v);
    }

    pub fn put_u32(&mut self, v: u32) {
        self.buf.put_u32(v);
    }

    pub fn put_u64(&mut self, v: u64) {
        self.buf.put_u64(v);
    }

    pub fn put_i64(&mut self, v: i64) {
        self.buf.put_i64(v);
    }

    pub fn put_f64(&mut self, v: f64) {
        self.buf.put_f64(v);
    }

    pub fn put_bool(&mut self, v: bool) {
        self.buf.put_u8(if v { 0x01 } else { 0x00 });
    }

    /// Write a length-prefixed string (u16 length + UTF-8 bytes).
    ///
    /// # Panics
    /// Panics if the string exceeds `u16::MAX` bytes.
    pub fn put_string(&mut self, s: &str) {
        let bytes = s.as_bytes();
        assert!(
            bytes.len() <= u16::MAX as usize,
            "string length {} exceeds u16::MAX ({})",
            bytes.len(),
            u16::MAX,
        );
        self.buf.put_u16(bytes.len() as u16);
        self.buf.extend_from_slice(bytes);
    }

    /// Write a length-prefixed byte array (u32 length + raw bytes).
    pub fn put_bytes(&mut self, b: &[u8]) {
        self.buf.put_u32(b.len() as u32);
        self.buf.extend_from_slice(b);
    }

    /// Write a map<string, string>: u16 count + repeated (string key, string value).
    ///
    /// # Panics
    /// Panics if the map has more than `u16::MAX` entries.
    pub fn put_string_map(&mut self, map: &std::collections::HashMap<String, String>) {
        assert!(
            map.len() <= u16::MAX as usize,
            "string map length {} exceeds u16::MAX ({})",
            map.len(),
            u16::MAX,
        );
        self.buf.put_u16(map.len() as u16);
        for (k, v) in map {
            self.put_string(k);
            self.put_string(v);
        }
    }

    /// Write a string array: u16 count + repeated string.
    ///
    /// # Panics
    /// Panics if the array has more than `u16::MAX` entries.
    pub fn put_string_array(&mut self, arr: &[String]) {
        assert!(
            arr.len() <= u16::MAX as usize,
            "string array length {} exceeds u16::MAX ({})",
            arr.len(),
            u16::MAX,
        );
        self.buf.put_u16(arr.len() as u16);
        for s in arr {
            self.put_string(s);
        }
    }

    /// Write optional<T>: u8 present flag, then T if present.
    pub fn put_optional_string(&mut self, opt: &Option<String>) {
        match opt {
            Some(s) => {
                self.put_u8(0x01);
                self.put_string(s);
            }
            None => {
                self.put_u8(0x00);
            }
        }
    }

    pub fn finish(self) -> Bytes {
        self.buf.freeze()
    }
}

/// Helper for reading frame payloads.
pub struct PayloadReader {
    buf: Bytes,
    pos: usize,
}

impl PayloadReader {
    pub fn new(buf: Bytes) -> Self {
        Self { buf, pos: 0 }
    }

    pub fn remaining(&self) -> usize {
        self.buf.len() - self.pos
    }

    fn ensure(&self, n: usize) -> Result<(), FrameError> {
        if self.remaining() < n {
            Err(FrameError::IncompleteFrame {
                need: n,
                have: self.remaining(),
            })
        } else {
            Ok(())
        }
    }

    pub fn read_u8(&mut self) -> Result<u8, FrameError> {
        self.ensure(1)?;
        let v = self.buf[self.pos];
        self.pos += 1;
        Ok(v)
    }

    pub fn read_u16(&mut self) -> Result<u16, FrameError> {
        self.ensure(2)?;
        let v = u16::from_be_bytes([self.buf[self.pos], self.buf[self.pos + 1]]);
        self.pos += 2;
        Ok(v)
    }

    pub fn read_u32(&mut self) -> Result<u32, FrameError> {
        self.ensure(4)?;
        let v = u32::from_be_bytes([
            self.buf[self.pos],
            self.buf[self.pos + 1],
            self.buf[self.pos + 2],
            self.buf[self.pos + 3],
        ]);
        self.pos += 4;
        Ok(v)
    }

    pub fn read_u64(&mut self) -> Result<u64, FrameError> {
        self.ensure(8)?;
        let v = u64::from_be_bytes([
            self.buf[self.pos],
            self.buf[self.pos + 1],
            self.buf[self.pos + 2],
            self.buf[self.pos + 3],
            self.buf[self.pos + 4],
            self.buf[self.pos + 5],
            self.buf[self.pos + 6],
            self.buf[self.pos + 7],
        ]);
        self.pos += 8;
        Ok(v)
    }

    pub fn read_i64(&mut self) -> Result<i64, FrameError> {
        self.ensure(8)?;
        let v = i64::from_be_bytes([
            self.buf[self.pos],
            self.buf[self.pos + 1],
            self.buf[self.pos + 2],
            self.buf[self.pos + 3],
            self.buf[self.pos + 4],
            self.buf[self.pos + 5],
            self.buf[self.pos + 6],
            self.buf[self.pos + 7],
        ]);
        self.pos += 8;
        Ok(v)
    }

    pub fn read_f64(&mut self) -> Result<f64, FrameError> {
        self.ensure(8)?;
        let v = f64::from_be_bytes([
            self.buf[self.pos],
            self.buf[self.pos + 1],
            self.buf[self.pos + 2],
            self.buf[self.pos + 3],
            self.buf[self.pos + 4],
            self.buf[self.pos + 5],
            self.buf[self.pos + 6],
            self.buf[self.pos + 7],
        ]);
        self.pos += 8;
        Ok(v)
    }

    pub fn read_bool(&mut self) -> Result<bool, FrameError> {
        Ok(self.read_u8()? != 0)
    }

    /// Read a length-prefixed string (u16 length + UTF-8 bytes).
    pub fn read_string(&mut self) -> Result<String, FrameError> {
        let len = self.read_u16()? as usize;
        self.ensure(len)?;
        let s = std::str::from_utf8(&self.buf[self.pos..self.pos + len])
            .map_err(|_| FrameError::InvalidUtf8)?
            .to_string();
        self.pos += len;
        Ok(s)
    }

    /// Read a length-prefixed byte array (u32 length + raw bytes).
    pub fn read_bytes(&mut self) -> Result<Vec<u8>, FrameError> {
        let len = self.read_u32()? as usize;
        self.ensure(len)?;
        let v = self.buf[self.pos..self.pos + len].to_vec();
        self.pos += len;
        Ok(v)
    }

    /// Read a map<string, string>.
    pub fn read_string_map(
        &mut self,
    ) -> Result<std::collections::HashMap<String, String>, FrameError> {
        let count = self.read_u16()? as usize;
        let mut map = std::collections::HashMap::with_capacity(count);
        for _ in 0..count {
            let k = self.read_string()?;
            let v = self.read_string()?;
            map.insert(k, v);
        }
        Ok(map)
    }

    /// Read a string array.
    pub fn read_string_array(&mut self) -> Result<Vec<String>, FrameError> {
        let count = self.read_u16()? as usize;
        let mut arr = Vec::with_capacity(count);
        for _ in 0..count {
            arr.push(self.read_string()?);
        }
        Ok(arr)
    }

    /// Read optional<string>.
    pub fn read_optional_string(&mut self) -> Result<Option<String>, FrameError> {
        let present = self.read_u8()?;
        if present != 0 {
            Ok(Some(self.read_string()?))
        } else {
            Ok(None)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::opcode::Opcode;

    #[test]
    fn frame_encode_decode_round_trip() {
        let frame = RawFrame {
            opcode: Opcode::Ping as u8,
            flags: 0,
            request_id: 42,
            payload: Bytes::new(),
        };

        let mut buf = BytesMut::new();
        frame.encode(&mut buf);

        let decoded = RawFrame::decode(&mut buf).unwrap().unwrap();
        assert_eq!(decoded.opcode, Opcode::Ping as u8);
        assert_eq!(decoded.flags, 0);
        assert_eq!(decoded.request_id, 42);
        assert!(decoded.payload.is_empty());
    }

    #[test]
    fn frame_with_payload() {
        let payload = Bytes::from_static(b"hello world");
        let frame = RawFrame {
            opcode: Opcode::Enqueue as u8,
            flags: 0,
            request_id: 1,
            payload: payload.clone(),
        };

        let mut buf = BytesMut::new();
        frame.encode(&mut buf);

        let decoded = RawFrame::decode(&mut buf).unwrap().unwrap();
        assert_eq!(decoded.payload, payload);
    }

    #[test]
    fn frame_decode_incomplete() {
        let mut buf = BytesMut::from(&[0u8, 0, 0][..]);
        assert!(RawFrame::decode(&mut buf).unwrap().is_none());
    }

    #[test]
    fn frame_too_large() {
        let mut buf = BytesMut::new();
        buf.put_u32(MAX_FRAME_SIZE + 1);
        assert!(matches!(
            RawFrame::decode(&mut buf),
            Err(FrameError::FrameTooLarge { .. })
        ));
    }

    #[test]
    fn continuation_flag() {
        let frame = RawFrame {
            opcode: Opcode::Enqueue as u8,
            flags: FLAG_CONTINUATION,
            request_id: 1,
            payload: Bytes::from_static(b"chunk1"),
        };
        assert!(frame.is_continuation());
    }

    #[test]
    fn payload_writer_reader_primitives() {
        let mut w = PayloadWriter::new();
        w.put_u8(0xFF);
        w.put_u16(1234);
        w.put_u32(567890);
        w.put_u64(u64::MAX);
        w.put_i64(-42);
        w.put_f64(3.14);
        w.put_bool(true);
        w.put_bool(false);
        w.put_string("hello");
        w.put_bytes(&[1, 2, 3]);

        let mut map = std::collections::HashMap::new();
        map.insert("k".to_string(), "v".to_string());
        w.put_string_map(&map);

        w.put_string_array(&["a".to_string(), "b".to_string()]);
        w.put_optional_string(&Some("opt".to_string()));
        w.put_optional_string(&None);

        let data = w.finish();
        let mut r = PayloadReader::new(data);

        assert_eq!(r.read_u8().unwrap(), 0xFF);
        assert_eq!(r.read_u16().unwrap(), 1234);
        assert_eq!(r.read_u32().unwrap(), 567890);
        assert_eq!(r.read_u64().unwrap(), u64::MAX);
        assert_eq!(r.read_i64().unwrap(), -42);
        assert!((r.read_f64().unwrap() - 3.14).abs() < f64::EPSILON);
        assert!(r.read_bool().unwrap());
        assert!(!r.read_bool().unwrap());
        assert_eq!(r.read_string().unwrap(), "hello");
        assert_eq!(r.read_bytes().unwrap(), vec![1, 2, 3]);

        let m = r.read_string_map().unwrap();
        assert_eq!(m.get("k").unwrap(), "v");

        let arr = r.read_string_array().unwrap();
        assert_eq!(arr, vec!["a", "b"]);

        assert_eq!(r.read_optional_string().unwrap(), Some("opt".to_string()));
        assert_eq!(r.read_optional_string().unwrap(), None);

        assert_eq!(r.remaining(), 0);
    }

    // --- ContinuationAssembler tests ---

    #[test]
    fn assembler_single_frame_passthrough() {
        let mut asm = ContinuationAssembler::new();
        let frame = RawFrame {
            opcode: Opcode::Enqueue as u8,
            flags: 0,
            request_id: 1,
            payload: Bytes::from_static(b"complete"),
        };
        let result = asm.push_frame(frame).unwrap();
        assert!(result.is_some());
        let out = result.unwrap();
        assert_eq!(out.opcode, Opcode::Enqueue as u8);
        assert_eq!(out.request_id, 1);
        assert_eq!(out.payload, Bytes::from_static(b"complete"));
        assert!(!out.is_continuation());
    }

    #[test]
    fn assembler_two_frame_reassembly() {
        let mut asm = ContinuationAssembler::new();

        // First frame: continuation=1
        let frame1 = RawFrame {
            opcode: Opcode::Enqueue as u8,
            flags: FLAG_CONTINUATION,
            request_id: 1,
            payload: Bytes::from_static(b"hello "),
        };
        assert!(asm.push_frame(frame1).unwrap().is_none());

        // Final frame: continuation=0
        let frame2 = RawFrame {
            opcode: Opcode::Enqueue as u8,
            flags: 0,
            request_id: 1,
            payload: Bytes::from_static(b"world"),
        };
        let result = asm.push_frame(frame2).unwrap();
        assert!(result.is_some());
        let out = result.unwrap();
        assert_eq!(out.opcode, Opcode::Enqueue as u8);
        assert_eq!(out.request_id, 1);
        assert_eq!(out.payload.as_ref(), b"hello world");
        assert!(!out.is_continuation());
    }

    #[test]
    fn assembler_three_frame_reassembly() {
        let mut asm = ContinuationAssembler::new();

        let f1 = RawFrame {
            opcode: Opcode::Enqueue as u8,
            flags: FLAG_CONTINUATION,
            request_id: 5,
            payload: Bytes::from_static(b"aaa"),
        };
        let f2 = RawFrame {
            opcode: Opcode::Enqueue as u8,
            flags: FLAG_CONTINUATION,
            request_id: 5,
            payload: Bytes::from_static(b"bbb"),
        };
        let f3 = RawFrame {
            opcode: Opcode::Enqueue as u8,
            flags: 0,
            request_id: 5,
            payload: Bytes::from_static(b"ccc"),
        };

        assert!(asm.push_frame(f1).unwrap().is_none());
        assert!(asm.push_frame(f2).unwrap().is_none());
        let out = asm.push_frame(f3).unwrap().unwrap();
        assert_eq!(out.payload.as_ref(), b"aaabbbccc");
    }

    #[test]
    fn assembler_interleaved_request_ids() {
        let mut asm = ContinuationAssembler::new();

        // Start continuation for request_id 1
        let f1a = RawFrame {
            opcode: Opcode::Enqueue as u8,
            flags: FLAG_CONTINUATION,
            request_id: 1,
            payload: Bytes::from_static(b"1a-"),
        };
        assert!(asm.push_frame(f1a).unwrap().is_none());

        // Start continuation for request_id 2
        let f2a = RawFrame {
            opcode: Opcode::Ack as u8,
            flags: FLAG_CONTINUATION,
            request_id: 2,
            payload: Bytes::from_static(b"2a-"),
        };
        assert!(asm.push_frame(f2a).unwrap().is_none());

        // A standalone frame for request_id 3 passes through
        let f3 = RawFrame {
            opcode: Opcode::Ping as u8,
            flags: 0,
            request_id: 3,
            payload: Bytes::new(),
        };
        let pass = asm.push_frame(f3).unwrap().unwrap();
        assert_eq!(pass.request_id, 3);
        assert_eq!(pass.opcode, Opcode::Ping as u8);

        // Finish request_id 2
        let f2b = RawFrame {
            opcode: Opcode::Ack as u8,
            flags: 0,
            request_id: 2,
            payload: Bytes::from_static(b"2b"),
        };
        let out2 = asm.push_frame(f2b).unwrap().unwrap();
        assert_eq!(out2.request_id, 2);
        assert_eq!(out2.opcode, Opcode::Ack as u8);
        assert_eq!(out2.payload.as_ref(), b"2a-2b");

        // Finish request_id 1
        let f1b = RawFrame {
            opcode: Opcode::Enqueue as u8,
            flags: 0,
            request_id: 1,
            payload: Bytes::from_static(b"1b"),
        };
        let out1 = asm.push_frame(f1b).unwrap().unwrap();
        assert_eq!(out1.request_id, 1);
        assert_eq!(out1.opcode, Opcode::Enqueue as u8);
        assert_eq!(out1.payload.as_ref(), b"1a-1b");
    }

    #[test]
    fn assembler_opcode_mismatch_error() {
        let mut asm = ContinuationAssembler::new();

        let f1 = RawFrame {
            opcode: Opcode::Enqueue as u8,
            flags: FLAG_CONTINUATION,
            request_id: 1,
            payload: Bytes::from_static(b"chunk"),
        };
        assert!(asm.push_frame(f1).unwrap().is_none());

        let f2 = RawFrame {
            opcode: Opcode::Ack as u8, // wrong opcode
            flags: 0,
            request_id: 1,
            payload: Bytes::from_static(b"bad"),
        };
        let err = asm.push_frame(f2).unwrap_err();
        assert!(matches!(
            err,
            FrameError::ContinuationOpcodeMismatch {
                request_id: 1,
                initial,
                got,
            } if initial == Opcode::Enqueue as u8 && got == Opcode::Ack as u8
        ));
    }

    #[test]
    fn assembler_max_size_exceeded_error() {
        let mut asm = ContinuationAssembler::new().with_max_reassembled_size(10);

        let f1 = RawFrame {
            opcode: Opcode::Enqueue as u8,
            flags: FLAG_CONTINUATION,
            request_id: 1,
            payload: Bytes::from_static(b"12345678"), // 8 bytes
        };
        assert!(asm.push_frame(f1).unwrap().is_none());

        let f2 = RawFrame {
            opcode: Opcode::Enqueue as u8,
            flags: 0,
            request_id: 1,
            payload: Bytes::from_static(b"abc"), // 8 + 3 = 11 > 10
        };
        let err = asm.push_frame(f2).unwrap_err();
        assert!(matches!(
            err,
            FrameError::ReassembledTooLarge {
                request_id: 1,
                size: 11,
                max: 10,
            }
        ));
    }

    #[test]
    fn assembler_max_size_exceeded_on_first_frame() {
        let mut asm = ContinuationAssembler::new().with_max_reassembled_size(3);

        let f1 = RawFrame {
            opcode: Opcode::Enqueue as u8,
            flags: FLAG_CONTINUATION,
            request_id: 7,
            payload: Bytes::from_static(b"toolong"),
        };
        let err = asm.push_frame(f1).unwrap_err();
        assert!(matches!(
            err,
            FrameError::ReassembledTooLarge {
                request_id: 7,
                max: 3,
                ..
            }
        ));
    }

    #[test]
    fn assembler_clear_discards_pending() {
        let mut asm = ContinuationAssembler::new();

        let f1 = RawFrame {
            opcode: Opcode::Enqueue as u8,
            flags: FLAG_CONTINUATION,
            request_id: 1,
            payload: Bytes::from_static(b"partial"),
        };
        assert!(asm.push_frame(f1).unwrap().is_none());

        asm.clear();

        // After clear, a non-continuation frame with request_id 1
        // should pass through (not treated as a continuation).
        let f2 = RawFrame {
            opcode: Opcode::Enqueue as u8,
            flags: 0,
            request_id: 1,
            payload: Bytes::from_static(b"fresh"),
        };
        let out = asm.push_frame(f2).unwrap().unwrap();
        assert_eq!(out.payload.as_ref(), b"fresh");
    }

    #[test]
    fn assembler_max_pending_streams() {
        let mut asm = ContinuationAssembler::new().with_max_pending_streams(2);

        // Fill 2 pending streams
        for rid in 0..2 {
            let f = RawFrame {
                opcode: Opcode::Enqueue as u8,
                flags: FLAG_CONTINUATION,
                request_id: rid,
                payload: Bytes::from_static(b"chunk"),
            };
            assert!(asm.push_frame(f).unwrap().is_none());
        }

        // Third should be rejected
        let f3 = RawFrame {
            opcode: Opcode::Enqueue as u8,
            flags: FLAG_CONTINUATION,
            request_id: 99,
            payload: Bytes::from_static(b"chunk"),
        };
        let err = asm.push_frame(f3).unwrap_err();
        assert!(
            matches!(
                err,
                FrameError::TooManyContinuationStreams { count: 3, max: 2 }
            ),
            "expected TooManyContinuationStreams, got: {err:?}"
        );
    }
}
