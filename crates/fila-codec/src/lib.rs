#![no_std]

extern crate alloc;

mod string8;
pub use string8::{String8, TooLong};

use core::str::Utf8Error;

#[derive(PartialEq, Debug, Copy, Clone)]
pub enum Opcode {
    Handshake = 0x01,   // 0x01
    HandshakeOk = 0x02, // 0x02
    Ping = 0x03,        // 0x03
}

impl From<Opcode> for u8 {
    fn from(opcode: Opcode) -> Self {
        opcode as u8
    }
}

#[derive(Debug, PartialEq)]
pub struct FrameHeader {
    pub opcode: Opcode,
    pub request_id: u32,
}

#[derive(Debug, PartialEq)]
pub enum OpcodeError {
    Missing,
    Invalid(u8),
}

#[derive(Debug, PartialEq)]
pub enum FlagsError {
    Missing,
    ContinuationUnsupported,
    ReservedSet { flags: u8 },
}

#[derive(Debug, PartialEq)]
pub enum HeaderError {
    Flags(FlagsError),
    Opcode(OpcodeError),
    RequestIdMissing,
}

impl TryFrom<u8> for Opcode {
    type Error = OpcodeError;

    fn try_from(value: u8) -> Result<Self, Self::Error> {
        match value {
            0x01 => Ok(Self::Handshake),
            0x02 => Ok(Self::HandshakeOk),
            0x03 => Ok(Self::Ping),
            v => Err(OpcodeError::Invalid(v)),
        }
    }
}

#[derive(Debug, PartialEq)]
pub enum FrameError {
    Header(HeaderError),
    LengthBelowHeader { length: u32 },
    LengthAboveMaximum { length: u32 },
    Body(BodyError),
}

#[derive(Debug, PartialEq)]
pub enum Decoded {
    Success {
        frame: Frame,
        length: u32,
        consumed: usize,
    },
    Incomplete,
    Invalid(FrameError),
}

const CONTINUATION: u8 = 0b0000_0001;
const RESERVED: u8 = 0b1111_1110;

fn validate_flags(flags: u8) -> Option<FlagsError> {
    if flags & CONTINUATION != 0 {
        Some(FlagsError::ContinuationUnsupported)
    } else if flags & RESERVED != 0 {
        Some(FlagsError::ReservedSet { flags })
    } else {
        None
    }
}

#[derive(Debug, PartialEq)]
pub struct Frame {
    pub request_id: u32,
    pub body: FrameBody,
}

impl Frame {
    pub fn opcode(&self) -> Opcode {
        match &self.body {
            FrameBody::Handshake(_) => Opcode::Handshake,
            FrameBody::HandshakeOk(_) => Opcode::HandshakeOk,
            FrameBody::Ping => Opcode::Ping,
        }
    }
}

#[derive(Debug, PartialEq)]
pub struct Handshake {
    pub min_version: u16,
    pub max_version: u16,
    pub api_key: Option<String8>,
    pub client_capabilities: u32,
}

#[derive(Debug, PartialEq)]
pub enum HandshakeError {
    MissingMinProtocolVersion,
    MissingMaxProtocolVersion,
    MissingApiKeyPresenceFlag,
    InvalidApiKey(String8Error),
    MissingClientCapabilities,
}

#[derive(Debug, PartialEq)]
pub struct HandshakeOk {
    pub negotiated_version: u16,
    pub node_id: u64,        // 0 if single-node
    pub max_frame_size: u32, // 0 = default 16 MiB
    pub server_capabilities: u32,
}

#[derive(Debug, PartialEq)]
pub enum HandshakeOkError {
    MissingNegotiatedVersion,
    MissingNodeId,
    MissingMaxFrameSize,
    MissingServerCapabilities,
}

#[derive(Debug, PartialEq)]
pub enum FrameBody {
    Handshake(Handshake),
    HandshakeOk(HandshakeOk),
    Ping,
}

#[derive(Debug, PartialEq)]
pub enum BodyError {
    Handshake(HandshakeError),
    HandshakeOk(HandshakeOkError),
}

pub struct Reader<'a> {
    buf: &'a [u8],
    pos: usize,
}

#[derive(Debug, PartialEq)]
pub enum String8Error {
    Truncated { declared: u8, available: usize },
    InvalidPresenceFlag(u8),
    InvalidUtf8(Utf8Error),
}

impl<'a> Reader<'a> {
    pub fn new(buf: &'a [u8]) -> Self {
        Self { buf, pos: 0 }
    }

    fn u8(&mut self) -> Option<u8> {
        if self.pos + 1 > self.buf.len() {
            return None;
        }
        let value = self.buf[self.pos];
        self.pos += 1;
        Some(value)
    }

    fn u16(&mut self) -> Option<u16> {
        if self.pos + 2 > self.buf.len() {
            return None;
        }
        let value = u16::from_be_bytes([self.buf[self.pos], self.buf[self.pos + 1]]);
        self.pos += 2;
        Some(value)
    }

    fn u32(&mut self) -> Option<u32> {
        if self.pos + 4 > self.buf.len() {
            return None;
        }
        let value = u32::from_be_bytes([
            self.buf[self.pos],
            self.buf[self.pos + 1],
            self.buf[self.pos + 2],
            self.buf[self.pos + 3],
        ]);
        self.pos += 4;
        Some(value)
    }

    fn u64(&mut self) -> Option<u64> {
        if self.pos + 8 > self.buf.len() {
            return None;
        }
        let value = u64::from_be_bytes([
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
        Some(value)
    }

    fn frame_header(&mut self) -> Result<FrameHeader, HeaderError> {
        let opcode = self.u8().ok_or(HeaderError::Opcode(OpcodeError::Missing))?;
        let opcode = opcode.try_into().map_err(HeaderError::Opcode)?;

        // we don't support any flags yet, so just validate none are set
        let flags = self.u8().ok_or(HeaderError::Flags(FlagsError::Missing))?;
        if let Some(err) = validate_flags(flags) {
            return Err(HeaderError::Flags(err));
        }

        let request_id = self.u32().ok_or(HeaderError::RequestIdMissing)?;

        Ok(FrameHeader { opcode, request_id })
    }

    fn optional_string8(&mut self) -> Option<Result<Option<String8>, String8Error>> {
        if self.pos >= self.buf.len() {
            return None;
        }

        if self.buf[self.pos] != 0 && self.buf[self.pos] != 1 {
            return Some(Err(String8Error::InvalidPresenceFlag(self.buf[self.pos])));
        }

        let is_present = self.buf[self.pos] == 1;
        self.pos += 1;

        if !is_present {
            return Some(Ok(None));
        }

        let len = self.u8()? as usize;
        if self.pos + len > self.buf.len() {
            return Some(Err(String8Error::Truncated {
                declared: len as u8,
                available: self.buf.len() - self.pos,
            }));
        }

        let string_bytes = &self.buf[self.pos..self.pos + len];
        self.pos += len;

        Some(
            String8::from_wire(string_bytes)
                .map(Some)
                .map_err(String8Error::InvalidUtf8),
        )
    }

    fn handshake(&mut self) -> Result<Handshake, HandshakeError> {
        let min_version = self
            .u16()
            .ok_or(HandshakeError::MissingMinProtocolVersion)?;
        let max_version = self
            .u16()
            .ok_or(HandshakeError::MissingMaxProtocolVersion)?;
        let api_key = self
            .optional_string8()
            .ok_or(HandshakeError::MissingApiKeyPresenceFlag)?
            .map_err(HandshakeError::InvalidApiKey)?;
        let client_capabilities = self
            .u32()
            .ok_or(HandshakeError::MissingClientCapabilities)?;

        Ok(Handshake {
            min_version,
            max_version,
            api_key,
            client_capabilities,
        })
    }

    fn handshake_ok(&mut self) -> Result<HandshakeOk, HandshakeOkError> {
        let negotiated_version = self
            .u16()
            .ok_or(HandshakeOkError::MissingNegotiatedVersion)?;
        let node_id = self.u64().ok_or(HandshakeOkError::MissingNodeId)?;
        let max_frame_size = self.u32().ok_or(HandshakeOkError::MissingMaxFrameSize)?;
        let server_capabilities = self
            .u32()
            .ok_or(HandshakeOkError::MissingServerCapabilities)?;

        Ok(HandshakeOk {
            negotiated_version,
            node_id,
            max_frame_size,
            server_capabilities,
        })
    }

    fn frame(&mut self) -> Result<Frame, FrameError> {
        let frame_header = self.frame_header().map_err(FrameError::Header)?;

        match frame_header.opcode {
            Opcode::Handshake => {
                let handshake = self
                    .handshake()
                    .map_err(|e| FrameError::Body(BodyError::Handshake(e)))?;
                Ok(Frame {
                    request_id: frame_header.request_id,
                    body: FrameBody::Handshake(handshake),
                })
            }
            Opcode::HandshakeOk => {
                let handshake_ok = self
                    .handshake_ok()
                    .map_err(|e| FrameError::Body(BodyError::HandshakeOk(e)))?;

                Ok(Frame {
                    request_id: frame_header.request_id,
                    body: FrameBody::HandshakeOk(handshake_ok),
                })
            }
            Opcode::Ping => Ok(Frame {
                request_id: frame_header.request_id,
                body: FrameBody::Ping,
            }),
        }
    }
}

/// Bytes of big-endian `u32` length prefix in front of every frame.
const LENGTH_PREFIX_LEN: usize = 4;

/// Bytes of header at the start of every frame body: opcode, flags, request id.
const FRAME_HEADER_LEN: usize = 6;

/// A frame body carries at least its header, so this is the smallest legal prefix.
const MIN_FRAME_BODY_LEN: u32 = FRAME_HEADER_LEN as u32;

const MAX_FRAME_BODY_LEN: u32 = 1024 * 1024 * 16; // 16 MiB

pub fn decode(buf: &[u8]) -> Decoded {
    if buf.len() < LENGTH_PREFIX_LEN {
        return Decoded::Incomplete;
    }

    let frame_len = u32::from_be_bytes([buf[0], buf[1], buf[2], buf[3]]);

    if frame_len > MAX_FRAME_BODY_LEN {
        return Decoded::Invalid(FrameError::LengthAboveMaximum { length: frame_len });
    }

    if frame_len < MIN_FRAME_BODY_LEN {
        return Decoded::Invalid(FrameError::LengthBelowHeader { length: frame_len });
    }

    if buf.len() < (frame_len as usize) + LENGTH_PREFIX_LEN {
        return Decoded::Incomplete;
    }

    let buf = &buf[LENGTH_PREFIX_LEN..(frame_len as usize) + LENGTH_PREFIX_LEN];
    let mut reader = Reader::new(buf);
    let frame = reader.frame();
    match frame {
        Ok(frame) => Decoded::Success {
            frame,
            length: frame_len,
            consumed: frame_len as usize + LENGTH_PREFIX_LEN,
        },
        Err(err) => Decoded::Invalid(err),
    }
}

fn encode_u8(value: impl Into<u8>, buf: &mut [u8]) -> Option<usize> {
    let value: u8 = value.into();
    let as_bytes = value.to_be_bytes();
    let len = as_bytes.len();
    buf.get_mut(..len)?.copy_from_slice(&as_bytes);
    Some(as_bytes.len())
}

fn encode_u32(value: impl Into<u32>, buf: &mut [u8]) -> Option<usize> {
    let value = value.into();
    let as_bytes = value.to_be_bytes();
    let len = as_bytes.len();
    buf.get_mut(..len)?.copy_from_slice(&as_bytes);
    Some(as_bytes.len())
}

fn encode_frame_header(frame_header: &FrameHeader, buf: &mut [u8]) -> Option<usize> {
    let mut cursor = 0;

    cursor += encode_u8(frame_header.opcode, &mut buf[cursor..])?;

    let flags = 0b0000_0000;
    cursor += encode_u8(flags, &mut buf[cursor..])?;

    cursor += encode_u32(frame_header.request_id, &mut buf[cursor..])?;

    Some(cursor)
}

fn encode_u16(value: impl Into<u16>, buf: &mut [u8]) -> Option<usize> {
    let value = value.into();
    let as_bytes = value.to_be_bytes();
    let len = as_bytes.len();
    buf.get_mut(..len)?.copy_from_slice(&as_bytes);
    Some(as_bytes.len())
}

fn encode_u64(value: impl Into<u64>, buf: &mut [u8]) -> Option<usize> {
    let value = value.into();
    let as_bytes = value.to_be_bytes();
    let len = as_bytes.len();
    buf.get_mut(..len)?.copy_from_slice(&as_bytes);
    Some(as_bytes.len())
}

fn encode_optional_string(value: &Option<String8>, buf: &mut [u8]) -> Option<usize> {
    let mut cursor = 0;

    match value {
        Some(s) => {
            cursor += encode_u8(1u8, &mut buf[cursor..])?;
            let s_bytes = s.as_bytes();
            let s_len = s_bytes.len() as u8;
            cursor += encode_u8(s_len, &mut buf[cursor..])?;
            buf.get_mut(cursor..cursor + s_len as usize)?
                .copy_from_slice(s_bytes);
            cursor += s_len as usize;
        }
        None => {
            cursor += encode_u8(0u8, &mut buf[cursor..])?;
        }
    }

    Some(cursor)
}

fn encode_handshake(handshake: &Handshake, buf: &mut [u8]) -> Option<usize> {
    let mut cursor = 0;

    cursor += encode_u16(handshake.min_version, &mut buf[cursor..])?;
    cursor += encode_u16(handshake.max_version, &mut buf[cursor..])?;
    cursor += encode_optional_string(&handshake.api_key, &mut buf[cursor..])?;
    cursor += encode_u32(handshake.client_capabilities, &mut buf[cursor..])?;

    Some(cursor)
}

fn encode_handshake_ok(handshake_ok: &HandshakeOk, buf: &mut [u8]) -> Option<usize> {
    let mut cursor = 0;

    cursor += encode_u16(handshake_ok.negotiated_version, &mut buf[cursor..])?;
    cursor += encode_u64(handshake_ok.node_id, &mut buf[cursor..])?;
    cursor += encode_u32(handshake_ok.max_frame_size, &mut buf[cursor..])?;
    cursor += encode_u32(handshake_ok.server_capabilities, &mut buf[cursor..])?;

    Some(cursor)
}

fn encode_frame_body(body: &FrameBody, buf: &mut [u8]) -> Option<usize> {
    let mut cursor = 0;

    match body {
        FrameBody::Handshake(handshake) => {
            cursor += encode_handshake(handshake, &mut buf[cursor..])?;
        }
        FrameBody::HandshakeOk(handshake_ok) => {
            cursor += encode_handshake_ok(handshake_ok, &mut buf[cursor..])?;
        }
        FrameBody::Ping => {}
    }

    Some(cursor)
}

fn encode_frame(frame: &Frame, buf: &mut [u8]) -> Option<usize> {
    let mut cursor = 0;

    let frame_header = FrameHeader {
        opcode: frame.opcode(),
        request_id: frame.request_id,
    };
    cursor += encode_frame_header(&frame_header, &mut buf[cursor..])?;

    cursor += encode_frame_body(&frame.body, &mut buf[cursor..])?;

    Some(cursor)
}

// return None if buf can't fit frame
pub fn encode(frame: &Frame, buf: &mut [u8]) -> Option<usize> {
    // starts at 4 to offset the length
    let mut cursor = LENGTH_PREFIX_LEN;
    if buf.len() < cursor {
        return None;
    }

    cursor += encode_frame(frame, &mut buf[cursor..])?;

    let frame_length = (cursor - LENGTH_PREFIX_LEN) as u32;
    encode_u32(frame_length, &mut buf[..])?;

    Some(cursor)
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloc::string::String;

    /// The smallest legal frame: a 6-byte body carrying only the header.
    /// prefix = 6, opcode = 0x03 (Ping), flags = 0, request_id = 1
    const PING: [u8; 10] = [0, 0, 0, 6, 0x03, 0x00, 0, 0, 0, 1];

    #[test]
    fn decodes_header_fields() {
        match decode(&PING) {
            Decoded::Success {
                length,
                frame,
                consumed,
            } => {
                assert_eq!(length, 6);
                assert_eq!(frame.opcode(), Opcode::Ping);
                assert_eq!(frame.request_id, 1);
                assert_eq!(consumed, 10, "prefix (4) + body (6)");
            }
            other => panic!("expected a frame, got {other:?}"),
        }
    }

    #[test]
    fn request_id_is_big_endian() {
        let mut buf = PING;
        buf[6..10].copy_from_slice(&0x0A0B0C0Du32.to_be_bytes());
        match decode(&buf) {
            Decoded::Success { frame, .. } => assert_eq!(frame.request_id, 0x0A0B0C0D),
            other => panic!("expected a frame, got {other:?}"),
        }
    }

    #[test]
    fn every_truncation_is_incomplete() {
        for n in 0..PING.len() {
            assert_eq!(
                decode(&PING[..n]),
                Decoded::Incomplete,
                "{n} of {} bytes should be Incomplete",
                PING.len()
            );
        }
    }

    #[test]
    fn continuation_flag_is_rejected_for_now() {
        let mut buf = PING;
        buf[5] = 0b0000_0001;
        assert_eq!(
            decode(&buf),
            Decoded::Invalid(FrameError::Header(HeaderError::Flags(
                FlagsError::ContinuationUnsupported
            )))
        );
    }

    #[test]
    fn body_shorter_than_a_header_is_invalid() {
        // A body carries its 6-byte header, so 0 through 5 are all impossible. The
        // prefix settles it: no body bytes need to have arrived.
        for length in 0..MIN_FRAME_BODY_LEN {
            assert_eq!(
                decode(&length.to_be_bytes()),
                Decoded::Invalid(FrameError::LengthBelowHeader { length }),
                "a body of {length} bytes cannot hold a 6-byte header"
            );
        }
    }

    #[test]
    fn consumed_points_at_the_next_frame() {
        let mut stream = PING.to_vec();
        stream.extend_from_slice(&PING);
        let Decoded::Success { consumed, .. } = decode(&stream) else {
            panic!("expected the first frame");
        };
        match decode(&stream[consumed..]) {
            Decoded::Success { .. } => {}
            other => panic!(
                "draining `consumed` bytes should leave the second frame at the front, got {other:?}"
            ),
        }
    }

    #[test]
    fn trailing_bytes_do_not_affect_the_first_frame() {
        let mut stream = PING.to_vec();
        stream.extend_from_slice(&[0xFF; 3]); // half of something else
        match decode(&stream) {
            Decoded::Success { consumed, .. } => assert_eq!(consumed, 10),
            other => panic!("expected a frame, got {other:?}"),
        }
    }

    #[test]
    fn a_length_above_the_maximum_frame_size_is_invalid() {
        // protocol.md caps a frame at 16 MiB. Without the check these four bytes
        // commit the reader to waiting for 4 GiB that will never arrive.
        let buf = (16 * 1024 * 1024 + 1u32).to_be_bytes();
        assert_eq!(
            decode(&buf),
            Decoded::Invalid(FrameError::LengthAboveMaximum { length: 16_777_217 })
        );
    }

    #[test]
    fn a_reserved_flag_bit_is_invalid() {
        // protocol.md, "Frame Header": bits 1-7 are reserved and must be 0.
        // Accepting them silently means a peer speaking a future version looks
        // fine while its extra semantics are ignored.
        for bit in 1..8 {
            let mut buf = PING;
            buf[5] = 1 << bit;
            assert_eq!(
                decode(&buf),
                Decoded::Invalid(FrameError::Header(HeaderError::Flags(
                    FlagsError::ReservedSet { flags: buf[5] }
                ))),
                "flags = {:#010b} sets reserved bit {bit} and should be rejected",
                buf[5]
            );
        }
    }

    #[test]
    fn a_frame_round_trips() {
        let frame = Frame {
            request_id: 1,
            body: FrameBody::Ping,
        };
        let frame_length = 6;

        let mut out = [0u8; 10];
        let written = encode(&frame, &mut out).expect("a valid frame should encode");
        assert_eq!(written, 10, "prefix (4) + body (6)");
        assert_eq!(out, PING, "encoded bytes should match the wire layout");

        match decode(&out) {
            Decoded::Success {
                length,
                frame: decoded,
                ..
            } => {
                assert_eq!(length, frame_length);
                assert_eq!(decoded.opcode(), frame.opcode());
                assert_eq!(decoded.request_id, frame.request_id);
            }
            other => panic!("what we encoded should decode, got {other:?}"),
        }
    }

    #[test]
    fn encode_never_emits_a_frame_decode_rejects() {
        // Whatever encode emits, decode has to accept: the two halves are one
        // codec, so a prefix encode writes must agree with the bytes after it.
        let frame = Frame {
            request_id: 1,
            body: FrameBody::Ping,
        };

        let mut out = [0u8; 64];
        let written = encode(&frame, &mut out).expect("a 64-byte buffer is plenty");
        match decode(&out[..written]) {
            Decoded::Success { .. } => {}
            other => panic!(
                "encode emitted {written} bytes that its own decode will not accept: {other:?}"
            ),
        }
    }

    #[test]
    fn encode_refuses_a_buffer_that_is_too_small() {
        let frame = Frame {
            request_id: 1,
            body: FrameBody::Ping,
        };

        // Every size below a whole frame should be declined, not panic. The
        // interesting ones are under 4: encode offsets its cursor past the length
        // prefix before anything has checked the buffer can hold it.
        let mut backing = [0u8; 10];
        for size in 0..10 {
            assert_eq!(
                encode(&frame, &mut backing[..size]),
                None,
                "a {size}-byte buffer cannot hold a 10-byte frame"
            );
        }
    }

    #[test]
    fn the_maximum_length_itself_is_accepted() {
        // MAX_FRAME_BODY_LEN is the largest legal body, not the first illegal one.
        // With only the prefix present the answer is Incomplete — more bytes are
        // needed — rather than Invalid.
        let buf = MAX_FRAME_BODY_LEN.to_be_bytes();
        assert_eq!(
            decode(&buf),
            Decoded::Incomplete,
            "a body of exactly the maximum is legal, so decode should ask for more bytes"
        );
    }

    #[test]
    fn continuation_combined_with_a_reserved_bit_reports_continuation() {
        // Bit 0 says continuation, which is a feature we have not built; bits 1-7
        // are not flags at all. When both are set the continuation is the more
        // specific fact, and the one a peer can act on.
        let mut buf = PING;
        buf[5] = 0b0000_0011;
        assert_eq!(
            decode(&buf),
            Decoded::Invalid(FrameError::Header(HeaderError::Flags(
                FlagsError::ContinuationUnsupported
            )))
        );
    }

    /// Handshake, request_id 0, versions 1 to 1, no api key, capabilities 1.
    /// body = opcode(1) flags(1) request_id(4) min(2) max(2) absent(1) caps(4) = 15
    const HANDSHAKE_NO_KEY: [u8; 19] = [
        0, 0, 0, 15, 0x01, 0x00, 0, 0, 0, 0, 0x00, 0x01, 0x00, 0x01, 0x00, 0, 0, 0, 1,
    ];

    #[test]
    fn decodes_a_handshake_body() {
        match decode(&HANDSHAKE_NO_KEY) {
            Decoded::Success {
                frame, consumed, ..
            } => {
                assert_eq!(consumed, 19, "prefix (4) + body (15)");
                assert_eq!(frame.request_id, 0);
                match frame.body {
                    FrameBody::Handshake(h) => {
                        assert_eq!(h.min_version, 1);
                        assert_eq!(h.max_version, 1);
                        assert_eq!(h.api_key, None);
                        assert_eq!(h.client_capabilities, 1);
                    }
                    other => panic!("expected a handshake body, got {other:?}"),
                }
            }
            other => panic!("expected a frame, got {other:?}"),
        }
    }

    #[test]
    fn an_absent_api_key_differs_from_an_empty_one() {
        // present = 0: no string follows at all
        let absent = HANDSHAKE_NO_KEY;

        // present = 1, length = 0: a string follows, and it is empty
        // body grows by the single length byte
        let empty = [
            0, 0, 0, 16, 0x01, 0x00, 0, 0, 0, 0, 0x00, 0x01, 0x00, 0x01, 0x01, 0x00, 0, 0, 0, 1,
        ];

        let key_of = |buf: &[u8]| match decode(buf) {
            Decoded::Success {
                frame:
                    Frame {
                        body: FrameBody::Handshake(h),
                        ..
                    },
                ..
            } => h.api_key,
            other => panic!("expected a handshake, got {other:?}"),
        };

        assert_eq!(key_of(&absent), None, "absent means there is no key");
        assert_eq!(
            key_of(&empty).as_deref(),
            Some(""),
            "present-but-empty is a key that happens to be empty"
        );
    }

    #[test]
    fn an_api_key_that_is_not_utf8_is_an_error() {
        // present = 1, length = 1, byte 0xFF is not valid UTF-8 on its own
        let buf = [
            0, 0, 0, 17, 0x01, 0x00, 0, 0, 0, 0, 0x00, 0x01, 0x00, 0x01, 0x01, 0x01, 0xFF, 0, 0, 0,
            1,
        ];
        match decode(&buf) {
            Decoded::Invalid(FrameError::Body(BodyError::Handshake(
                HandshakeError::InvalidApiKey(_),
            ))) => {}
            other => panic!(
                "a string that is not UTF-8 is rejected, never silently replaced, got {other:?}"
            ),
        }
    }

    #[test]
    fn an_api_key_longer_than_the_body_does_not_panic() {
        // present = 1, length = 255, but only one byte follows
        let buf = [
            0, 0, 0, 17, 0x01, 0x00, 0, 0, 0, 0, 0x00, 0x01, 0x00, 0x01, 0x01, 0xFF, 0xFF, 0, 0, 0,
            1,
        ];
        // Not just "an error": the numbers say which byte lied and by how much.
        // 255 was declared, 5 bytes of body remained after the length byte.
        assert_eq!(
            decode(&buf),
            Decoded::Invalid(FrameError::Body(BodyError::Handshake(
                HandshakeError::InvalidApiKey(String8Error::Truncated {
                    declared: 255,
                    available: 5,
                })
            )))
        );
    }

    #[test]
    fn a_handshake_frame_round_trips() {
        let frame = Frame {
            request_id: 0,
            body: FrameBody::Handshake(Handshake {
                min_version: 1,
                max_version: 3,
                api_key: Some(String8::try_from("secret").unwrap()),
                client_capabilities: 1,
            }),
        };

        let mut out = [0u8; 64];
        let written = encode(&frame, &mut out).expect("a 64-byte buffer is plenty");

        match decode(&out[..written]) {
            Decoded::Success { frame: decoded, .. } => {
                assert_eq!(decoded.request_id, 0);
                match decoded.body {
                    FrameBody::Handshake(h) => {
                        assert_eq!(h.min_version, 1);
                        assert_eq!(h.max_version, 3, "the two versions do not get swapped");
                        assert_eq!(h.api_key.as_deref(), Some("secret"));
                        assert_eq!(h.client_capabilities, 1);
                    }
                    other => panic!("expected a handshake body, got {other:?}"),
                }
            }
            other => panic!("what we encoded should decode, got {other:?}"),
        }
    }

    #[test]
    fn a_handshake_ok_frame_round_trips() {
        let frame = Frame {
            request_id: 0,
            body: FrameBody::HandshakeOk(HandshakeOk {
                negotiated_version: 1,
                node_id: 0,
                max_frame_size: 0,
                server_capabilities: 1,
            }),
        };

        let mut out = [0u8; 64];
        let written = encode(&frame, &mut out).expect("a 64-byte buffer is plenty");

        match decode(&out[..written]) {
            Decoded::Success { frame: decoded, .. } => match decoded.body {
                FrameBody::HandshakeOk(h) => {
                    assert_eq!(h.negotiated_version, 1);
                    assert_eq!(h.node_id, 0);
                    assert_eq!(h.max_frame_size, 0);
                    assert_eq!(h.server_capabilities, 1);
                }
                other => panic!("expected a handshake-ok body, got {other:?}"),
            },
            other => panic!("what we encoded should decode, got {other:?}"),
        }
    }

    #[test]
    fn an_api_key_declares_its_length_in_one_byte() {
        // protocol.md, "Encoding Primitives": `string8` is `[u8 length][UTF-8 bytes]`.
        // api_key is an `optional<string8>`, so a present key costs one presence byte,
        // one length byte, then the bytes themselves.
        //
        // body = opcode(1) flags(1) request_id(4) min(2) max(2)
        //        present(1) length(1) "k"(1) capabilities(4) = 17
        let buf = [
            0, 0, 0, 17, 0x01, 0x00, 0, 0, 0, 0, 0x00, 0x01, 0x00, 0x01, 0x01, 0x01, b'k', 0, 0, 0,
            1,
        ];

        match decode(&buf) {
            Decoded::Success {
                frame, consumed, ..
            } => {
                assert_eq!(consumed, 21, "prefix (4) + body (17)");
                match frame.body {
                    FrameBody::Handshake(h) => {
                        assert_eq!(h.api_key.as_deref(), Some("k"));
                        assert_eq!(h.client_capabilities, 1, "the field after the string");
                    }
                    other => panic!("expected a handshake body, got {other:?}"),
                }
            }
            other => panic!("expected a frame, got {other:?}"),
        }
    }

    #[test]
    fn the_handshake_carries_a_version_range() {
        // protocol.md, "Handshake (0x01)": the body opens with the highest version the
        // client speaks and the lowest it will accept. Both are fixed at these offsets
        // for the life of the protocol, so a reader can answer a handshake whose layout
        // it does not know.
        //
        // body = opcode(1) flags(1) request_id(4) min(2) max(2)
        //        absent api_key(1) capabilities(4) = 15
        //
        // min_version is 2 and max_version is 3, so neither shares a value with
        // capabilities. A field read at the wrong offset then names which field it
        // actually landed on.
        let buf = [
            0, 0, 0, 15, 0x01, 0x00, 0, 0, 0, 0, 0x00, 0x02, 0x00, 0x03, 0x00, 0, 0, 0, 1,
        ];

        match decode(&buf) {
            Decoded::Success { frame, .. } => match frame.body {
                FrameBody::Handshake(h) => {
                    // Passes today for the wrong reason: a reader that stops after one
                    // version field takes min_version's high byte, which is 0x00, for
                    // this presence flag.
                    assert_eq!(h.api_key, None);
                    assert_eq!(
                        h.client_capabilities,
                        1,
                        "capabilities sits in the last four bytes of the body, but was \
                         {:#010x}. A reader that consumes only one version field starts \
                         every later read two bytes early, so the leading byte here is \
                         max_version's {}.",
                        h.client_capabilities,
                        h.client_capabilities >> 24
                    );
                }
                other => panic!("expected a handshake body, got {other:?}"),
            },
            other => panic!("expected a frame, got {other:?}"),
        }
    }

    #[test]
    fn an_unassigned_capability_bit_is_ignored_not_rejected() {
        // protocol.md, "Capabilities": the active set is the bitwise AND of both
        // bitmaps, and an unassigned bit is ignored. Rejecting one would mean a newer
        // client could never handshake with an older server, which is the case the AND
        // exists to handle. The codec's job is to carry the bitmap intact so a
        // connection can compute that AND.
        //
        // capabilities = bit 31 (unassigned) and bit 0 (CREDIT_FLOW_CONTROL)
        let buf = [
            0, 0, 0, 15, 0x01, 0x00, 0, 0, 0, 0, 0x00, 0x01, 0x00, 0x01, 0x00, 0x80, 0, 0, 0x01,
        ];

        match decode(&buf) {
            Decoded::Success { frame, .. } => match frame.body {
                FrameBody::Handshake(h) => assert_eq!(
                    h.client_capabilities, 0x8000_0001,
                    "the bitmap reaches the caller unaltered"
                ),
                other => panic!("expected a handshake body, got {other:?}"),
            },
            other => panic!("an unassigned bit is not a reason to reject, got {other:?}"),
        }
    }

    /// Body ends right after `max_version`: there is no presence byte.
    /// body = opcode(1) flags(1) request_id(4) min(2) max(2) = 10
    const NO_PRESENCE_BYTE: [u8; 14] =
        [0, 0, 0, 10, 0x01, 0x00, 0, 0, 0, 0, 0x00, 0x01, 0x00, 0x01];

    /// A presence byte saying "present", and then nothing: no length byte.
    /// body = 10 + present(1) = 11
    const NO_LENGTH_BYTE: [u8; 15] = [
        0, 0, 0, 11, 0x01, 0x00, 0, 0, 0, 0, 0x00, 0x01, 0x00, 0x01, 0x01,
    ];

    /// A length of 255 with one byte behind it.
    /// body = 10 + present(1) + length(1) + one byte(1) = 13
    const TRUNCATED_STRING: [u8; 17] = [
        0, 0, 0, 13, 0x01, 0x00, 0, 0, 0, 0, 0x00, 0x01, 0x00, 0x01, 0x01, 0xFF, b'a',
    ];

    #[test]
    fn a_truncated_string_is_not_a_body_that_ran_out() {
        // NO_PRESENCE_BYTE and NO_LENGTH_BYTE are the same fault — the body ended
        // inside the field — so they share an error. TRUNCATED_STRING is not: both
        // bytes arrived, and the length they carry contradicts the body.
        assert_ne!(
            decode(&TRUNCATED_STRING),
            decode(&NO_PRESENCE_BYTE),
            "a length that was read and overruns the body is a different fault from \
             a body that ended before the field did"
        );
        assert_eq!(
            decode(&NO_PRESENCE_BYTE),
            decode(&NO_LENGTH_BYTE),
            "both of these are the body ending inside api_key, which is one fault"
        );
    }

    /// HandshakeOk, request_id 0, negotiated version 1, node 0x0A, frame size 1 MiB,
    /// capabilities 1.
    /// body = opcode(1) flags(1) request_id(4) version(2) node_id(8)
    ///        max_frame_size(4) capabilities(4) = 24
    const HANDSHAKE_OK: [u8; 28] = [
        0, 0, 0, 24, 0x02, 0x00, 0, 0, 0, 0, 0x00, 0x01, 0, 0, 0, 0, 0, 0, 0, 0x0A, 0x00, 0x10,
        0x00, 0x00, 0x00, 0x00, 0x00, 0x01,
    ];

    #[test]
    fn decodes_a_handshake_ok_body() {
        // The round trip cannot catch a field order that encode and decode get wrong
        // the same way: swap node_id and max_frame_size in both and it still passes.
        // Only bytes written by hand pin the layout.
        match decode(&HANDSHAKE_OK) {
            Decoded::Success {
                frame, consumed, ..
            } => {
                assert_eq!(consumed, 28, "prefix (4) + body (24)");
                assert_eq!(frame.request_id, 0);
                match frame.body {
                    FrameBody::HandshakeOk(h) => {
                        assert_eq!(h.negotiated_version, 1);
                        assert_eq!(h.node_id, 0x0A);
                        assert_eq!(h.max_frame_size, 1024 * 1024);
                        assert_eq!(h.server_capabilities, 1);
                    }
                    other => panic!("expected a handshake-ok body, got {other:?}"),
                }
            }
            other => panic!("expected a frame, got {other:?}"),
        }
    }

    #[test]
    fn every_truncation_of_a_handshake_is_incomplete() {
        // The same property as `every_truncation_is_incomplete`, over a body with
        // variable-width fields in it. A short stream is Incomplete at every length,
        // never Invalid: the frame has not arrived yet, so nothing in it can be judged.
        for n in 0..HANDSHAKE_NO_KEY.len() {
            assert_eq!(
                decode(&HANDSHAKE_NO_KEY[..n]),
                Decoded::Incomplete,
                "{n} of {} bytes should be Incomplete",
                HANDSHAKE_NO_KEY.len()
            );
        }
    }

    #[test]
    fn a_handshake_body_carrying_only_a_header_is_invalid() {
        // A legal frame length, and a body that stops before the first field. The
        // frame arrived whole, so this is Invalid rather than Incomplete.
        let buf = [0, 0, 0, 6, 0x01, 0x00, 0, 0, 0, 0];
        assert_eq!(
            decode(&buf),
            Decoded::Invalid(FrameError::Body(BodyError::Handshake(
                HandshakeError::MissingMinProtocolVersion
            )))
        );
    }

    #[test]
    fn a_presence_byte_that_is_neither_0_nor_1_is_invalid() {
        // protocol.md, "Encoding Primitives": `bool` is `0x00` = false, `0x01` = true,
        // and `optional<T>` is `[u8 present][T if present]`. 0x02 is not a value this
        // protocol defines.
        //
        // Guessing costs the rest of the body: the presence byte decides whether the
        // next byte is a length or the first byte of client_capabilities, so reading it
        // wrong misparses every field behind it. That is the same reason a reserved
        // flag bit is rejected rather than ignored — a value that changes how the frame
        // parses cannot be assumed.
        //
        // The body is laid out so that reading 0x02 as "present" *succeeds*: a zero
        // length byte follows, then four bytes of capabilities. A decoder that guesses
        // therefore returns a frame rather than an error, which is what makes this
        // worth a test — nothing downstream can tell the key was never really there.
        //
        // body = opcode(1) flags(1) request_id(4) min(2) max(2)
        //        present(1) length(1) capabilities(4) = 16
        let buf = [
            0, 0, 0, 16, 0x01, 0x00, 0, 0, 0, 0, 0x00, 0x01, 0x00, 0x01, 0x02, 0x00, 0, 0, 0, 1,
        ];

        match decode(&buf) {
            Decoded::Invalid(_) => {}
            other => panic!("0x02 is not a defined presence byte, got {other:?}"),
        }
    }

    #[test]
    fn an_unknown_opcode_is_invalid() {
        // protocol.md, "Handling Unknown Opcodes": a request opcode a server does not
        // know is answered `InvalidFrame` with the connection left open, so the codec
        // has to name the byte it did not recognise rather than guess at a body.
        for opcode in [0x00u8, 0x04, 0x7F, 0xFF] {
            let mut buf = PING;
            buf[4] = opcode;
            assert_eq!(
                decode(&buf),
                Decoded::Invalid(FrameError::Header(HeaderError::Opcode(
                    OpcodeError::Invalid(opcode)
                ))),
                "{opcode:#04x} is not an opcode this version defines"
            );
        }
    }

    #[test]
    fn the_longest_legal_handshake_round_trips() {
        // An api_key of 255 bytes is the largest a string8 can describe, which makes
        // this the largest Handshake version 1 can produce. protocol.md puts it at 275
        // bytes, and a server refusing an unauthenticated peer's first frame leans on
        // that number.
        let mut key = String::new();
        for _ in 0..String8::MAX_LEN {
            key.push('a');
        }

        let frame = Frame {
            request_id: 0,
            body: FrameBody::Handshake(Handshake {
                min_version: 1,
                max_version: 1,
                api_key: Some(String8::try_from(key).expect("255 bytes is a string8")),
                client_capabilities: 1,
            }),
        };

        let mut out = [0u8; 512];
        let written = encode(&frame, &mut out).expect("512 bytes is plenty");
        assert_eq!(
            written, 275,
            "4 prefix + 6 header + 4 versions + 2 presence and length + 255 key + 4 capabilities"
        );
        assert_eq!(out[15], 0xFF, "the length byte holds 255");

        match decode(&out[..written]) {
            Decoded::Success { frame: decoded, .. } => match decoded.body {
                FrameBody::Handshake(h) => {
                    let key = h.api_key.expect("the key survived");
                    assert_eq!(key.len(), 255);
                    assert_eq!(key.wire_len(), 0xFF);
                }
                other => panic!("expected a handshake body, got {other:?}"),
            },
            other => panic!("what we encoded should decode, got {other:?}"),
        }
    }

    #[test]
    fn encode_refuses_a_buffer_too_small_for_a_handshake() {
        // `encode_refuses_a_buffer_that_is_too_small` covers a Ping, whose body is all
        // fixed-width. This reaches the copy that writes a string's bytes, which is a
        // separate place the buffer can run out.
        let frame = Frame {
            request_id: 0,
            body: FrameBody::Handshake(Handshake {
                min_version: 1,
                max_version: 1,
                api_key: Some(String8::try_from("secret").unwrap()),
                client_capabilities: 1,
            }),
        };

        // 4 prefix + 6 header + 4 versions + present(1) + length(1) + "secret"(6) + caps(4)
        const WHOLE: usize = 26;
        let mut backing = [0u8; WHOLE];
        for size in 0..WHOLE {
            assert_eq!(
                encode(&frame, &mut backing[..size]),
                None,
                "a {size}-byte buffer cannot hold a {WHOLE}-byte frame"
            );
        }
        assert_eq!(
            encode(&frame, &mut backing),
            Some(WHOLE),
            "a buffer of exactly the frame's size is enough"
        );
    }

    #[test]
    fn bytes_past_a_handshake_s_fields_are_ignored() {
        // protocol.md, "Adding Fields": a field is added by appending it and letting
        // older readers skip what they do not recognise. This is that rule — the whole
        // evolution story rests on it, and nothing else here tests it.
        //
        // A declared body of 20 where version 1's fields need 15, so five bytes follow
        // capabilities that this version knows nothing about.
        let buf = [
            0, 0, 0, 20, 0x01, 0x00, 0, 0, 0, 0, 0x00, 0x01, 0x00, 0x01, 0x00, 0, 0, 0, 1, 0xDE,
            0xAD, 0xBE, 0xEF, 0x01,
        ];

        match decode(&buf) {
            Decoded::Success {
                frame,
                length,
                consumed,
            } => {
                assert_eq!(length, 20, "the frame declared what it declared");
                assert_eq!(
                    consumed, 24,
                    "consumed follows the declared length, not where the reader stopped"
                );
                match frame.body {
                    FrameBody::Handshake(h) => {
                        assert_eq!(h.min_version, 1);
                        assert_eq!(h.client_capabilities, 1, "read before the unknown bytes");
                    }
                    other => panic!("expected a handshake body, got {other:?}"),
                }
            }
            other => panic!("trailing bytes are tolerated, not rejected, got {other:?}"),
        }
    }

    #[test]
    fn a_short_body_cannot_read_into_the_frame_behind_it() {
        // A handshake declaring 14 bytes, which stops two bytes into capabilities, with
        // a whole Ping sitting behind it. The bytes the handshake wants are physically
        // present in the buffer, so a reader given the whole buffer instead of the
        // declared body would find them, return a frame with capabilities built from
        // the Ping's header, and leave the stream desynchronised with no error anywhere.
        let mut stream = alloc::vec::Vec::new();
        stream.extend_from_slice(&[
            0, 0, 0, 14, 0x01, 0x00, 0, 0, 0, 0, 0x00, 0x01, 0x00, 0x01, 0x00, 0, 0,
        ]);
        stream.extend_from_slice(&PING);

        assert_eq!(
            decode(&stream),
            Decoded::Invalid(FrameError::Body(BodyError::Handshake(
                HandshakeError::MissingClientCapabilities
            ))),
            "a body is bounded by its own length, whatever happens to follow it"
        );
    }

    #[test]
    fn consumed_points_past_a_variable_length_body() {
        // `consumed_points_at_the_next_frame` uses two Pings, whose bodies are all
        // fixed-width. A body whose size depends on its contents is where an off-by-one
        // in the cursor would show up.
        let mut stream = alloc::vec::Vec::new();
        stream.extend_from_slice(&[
            0, 0, 0, 17, 0x01, 0x00, 0, 0, 0, 0, 0x00, 0x01, 0x00, 0x01, 0x01, 0x01, b'k', 0, 0, 0,
            1,
        ]);
        stream.extend_from_slice(&PING);

        let Decoded::Success { consumed, .. } = decode(&stream) else {
            panic!("expected the handshake");
        };
        assert_eq!(consumed, 21, "prefix (4) + body (17)");

        match decode(&stream[consumed..]) {
            Decoded::Success { frame, .. } => assert_eq!(
                frame.opcode(),
                Opcode::Ping,
                "the next frame begins exactly where consumed says"
            ),
            other => panic!("expected the ping behind it, got {other:?}"),
        }
    }
}
