//! Fila's wire protocol: frames in, frames out, and no I/O.
//!
//! See docs/protocol.md. A frame is a `u32` length followed by a body, and the body
//! opens with a 6-byte header naming the operation.

#![no_std]

extern crate alloc;

mod batch;
mod enqueue;
mod field;
mod frame;
mod handshake;
mod headers;
mod payload;
mod reader;
mod string8;
mod writer;

pub use batch::{Batch, TooManyItemsError};
pub use enqueue::{Enqueue, EnqueueError, EnqueueMessage, InvalidHeaderError, MessageField};
pub use field::{FieldError, FieldFault};
pub use frame::{
    BodyError, EncodeError, FlagsError, Frame, FrameBody, FrameError, HeaderError, Opcode,
    OpcodeError,
};
pub use handshake::{
    Handshake, HandshakeError, HandshakeField, HandshakeOk, HandshakeOkError, HandshakeOkField,
};
pub use headers::{DuplicateKeyError, Headers, HeadersError, TooManyError};
pub use payload::{Payload, TooLargeError};
pub use reader::String8DecodeError;
pub use string8::{String8, TooLongError};

pub(crate) use reader::Reader;
pub(crate) use writer::Writer;

/// Bytes of big-endian `u32` length prefix in front of every frame.
pub(crate) const LENGTH_PREFIX_LEN: usize = 4;

/// Bytes of header at the start of every frame body: opcode, flags, request id.
pub(crate) const FRAME_HEADER_LEN: usize = 6;

/// A frame body carries at least its header, so this is the smallest legal prefix.
pub(crate) const MIN_FRAME_BODY_LEN: u32 = FRAME_HEADER_LEN as u32;

/// The frame body size to fall back to until a handshake advertises one.
///
/// Not a bound this crate enforces: [`encode`] is given a maximum and [`decode`] takes
/// none at all.
pub const DEFAULT_MAX_FRAME_BODY_LEN: u32 = 1024 * 1024 * 16;

/// What a buffer held when `decode` looked at it.
#[derive(Debug, PartialEq)]
pub enum Decoded<'a> {
    Success {
        frame: Frame<'a>,
        length: u32,
        consumed: usize,
    },
    /// The declared frame has not arrived in full. More bytes may change this answer.
    Incomplete,
    /// The declared frame arrived and does not add up. More bytes will not help.
    Invalid(FrameError),
}

/// Reads the frame at the front of `buf`.
pub fn decode(buf: &[u8]) -> Decoded<'_> {
    if buf.len() < LENGTH_PREFIX_LEN {
        return Decoded::Incomplete;
    }

    let frame_len = u32::from_be_bytes([buf[0], buf[1], buf[2], buf[3]]);

    if frame_len < MIN_FRAME_BODY_LEN {
        return Decoded::Invalid(FrameError::LengthBelowHeader { length: frame_len });
    }

    if buf.len() < (frame_len as usize) + LENGTH_PREFIX_LEN {
        return Decoded::Incomplete;
    }

    // Bounded by its own declared length, so a body cannot read into the frame behind it.
    let body = &buf[LENGTH_PREFIX_LEN..(frame_len as usize) + LENGTH_PREFIX_LEN];
    let mut reader = Reader::new(body);

    match Frame::read(&mut reader) {
        Ok(frame) => Decoded::Success {
            frame,
            length: frame_len,
            consumed: frame_len as usize + LENGTH_PREFIX_LEN,
        },
        Err(err) => Decoded::Invalid(err),
    }
}

/// Writes `frame` into `buf` and returns how many bytes it took.
///
/// `max_body_len` is what the peer agreed to read, so a body above it is refused here
/// rather than written out for the peer to reject.
///
/// The buffer may have been written to before either failure was discovered, so its
/// contents are unspecified unless this returns `Ok`.
pub fn encode(frame: &Frame<'_>, buf: &mut [u8], max_body_len: u32) -> Result<usize, EncodeError> {
    let mut writer = Writer::new(buf);
    frame.write(&mut writer, max_body_len)?;
    Ok(writer.pos())
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
    fn decode_does_not_judge_a_declared_length() {
        // The maximum is per connection - advertised in HandshakeOk, 4 KiB before a
        // handshake completes - so a declared length is only ever "not here yet" to
        // the codec. Whether to wait for it is the caller's, which reads this same
        // prefix to know when a frame is whole.
        for length in [
            DEFAULT_MAX_FRAME_BODY_LEN,
            DEFAULT_MAX_FRAME_BODY_LEN + 1,
            u32::MAX,
        ] {
            assert_eq!(
                decode(&length.to_be_bytes()),
                Decoded::Incomplete,
                "a declared length of {length} is a frame that has not arrived"
            );
        }
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
        let written = encode(&frame, &mut out, DEFAULT_MAX_FRAME_BODY_LEN)
            .expect("a valid frame should encode");
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
        let written = encode(&frame, &mut out, DEFAULT_MAX_FRAME_BODY_LEN)
            .expect("a 64-byte buffer is plenty");
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
                encode(&frame, &mut backing[..size], DEFAULT_MAX_FRAME_BODY_LEN),
                Err(EncodeError::BufferTooSmall),
                "a {size}-byte buffer cannot hold a 10-byte frame"
            );
        }
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

        fn key_of(buf: &[u8]) -> Option<String8<'_>> {
            match decode(buf) {
                Decoded::Success {
                    frame:
                        Frame {
                            body: FrameBody::Handshake(h),
                            ..
                        },
                    ..
                } => h.api_key,
                other => panic!("expected a handshake, got {other:?}"),
            }
        }

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
            Decoded::Invalid(FrameError::Body(BodyError::Handshake(FieldFault {
                field: HandshakeField::ApiKey,
                source: FieldError::String8(String8DecodeError::InvalidUtf8(_)),
            }))) => {}
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
            Decoded::Invalid(FrameError::Body(BodyError::Handshake(FieldFault {
                field: HandshakeField::ApiKey,
                source: FieldError::String8(String8DecodeError::Truncated {
                    expected: 255,
                    found: 5,
                }),
            })))
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
        let written = encode(&frame, &mut out, DEFAULT_MAX_FRAME_BODY_LEN)
            .expect("a 64-byte buffer is plenty");

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
        let written = encode(&frame, &mut out, DEFAULT_MAX_FRAME_BODY_LEN)
            .expect("a 64-byte buffer is plenty");

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
        // protocol.md, "Handshake (0x01)": the body opens with the lowest version the
        // client will accept and then the highest it speaks. Both are fixed at these
        // offsets for the life of the protocol, so a reader can answer a handshake whose
        // layout it does not know.
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
            Decoded::Invalid(FrameError::Body(BodyError::Handshake(FieldFault {
                field: HandshakeField::MinVersion,
                source: FieldError::Ended,
            })))
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
                api_key: Some(String8::try_from(key.as_str()).expect("255 bytes is a string8")),
                client_capabilities: 1,
            }),
        };

        let mut out = [0u8; 512];
        let written =
            encode(&frame, &mut out, DEFAULT_MAX_FRAME_BODY_LEN).expect("512 bytes is plenty");
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
                encode(&frame, &mut backing[..size], DEFAULT_MAX_FRAME_BODY_LEN),
                Err(EncodeError::BufferTooSmall),
                "a {size}-byte buffer cannot hold a {WHOLE}-byte frame"
            );
        }
        assert_eq!(
            encode(&frame, &mut backing, DEFAULT_MAX_FRAME_BODY_LEN),
            Ok(WHOLE),
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
            Decoded::Invalid(FrameError::Body(BodyError::Handshake(FieldFault {
                field: HandshakeField::ClientCapabilities,
                source: FieldError::Ended,
            }))),
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

    #[test]
    fn an_error_reads_as_the_path_to_the_byte_that_failed() {
        // Each level prefixes the one below it, so one line says where in the frame the
        // fault is. This is what a server puts in an `Error` frame's `message`.
        let Decoded::Invalid(err) = decode(&TRUNCATED_STRING) else {
            panic!("expected an invalid frame");
        };
        assert_eq!(
            alloc::format!("{err}"),
            "frame body: handshake: api_key: expected 255 bytes, found 1"
        );
    }

    #[test]
    fn an_error_chains_to_its_cause() {
        // Display carries the whole path, and `source` still exposes each level, so a
        // caller can match on a cause instead of reading a string.
        let Decoded::Invalid(err) = decode(&TRUNCATED_STRING) else {
            panic!("expected an invalid frame");
        };

        let mut depth = 0;
        let mut cause = core::error::Error::source(&err);
        while let Some(err) = cause {
            depth += 1;
            cause = err.source();
        }
        assert_eq!(
            depth, 4,
            "FrameError -> BodyError -> FieldFault -> FieldError -> String8DecodeError"
        );
    }
    /// A `String8` cannot be a literal, and a header list needs four of them a line.
    fn string8(value: &str) -> String8<'_> {
        String8::try_from(value).expect("a test fixture fits a string8")
    }

    /// A batch holds at most 65,535 items, so building one from a vec can fail.
    fn enqueue_batch<'a>(messages: alloc::vec::Vec<EnqueueMessage<'a>>) -> Enqueue<'a> {
        Enqueue::try_from(messages).expect("a test fixture fits a batch")
    }

    /// One message with every field present and no two values alike, so a field read
    /// at the wrong offset reports the wrong number rather than passing by coincidence.
    fn an_enqueue() -> Frame<'static> {
        Frame {
            request_id: 11,
            body: FrameBody::Enqueue(enqueue_batch(alloc::vec![EnqueueMessage {
                queue: string8("orders"),
                headers: Headers::try_from(alloc::vec![
                    (string8("a-traceparent"), string8("00-abc-def-01")),
                    (string8("b-tenant"), string8("acme")),
                ])
                .expect("a test fixture fits a header map"),
                payload: Payload::try_from(&[0xDEu8, 0xAD, 0xBE, 0xEF][..])
                    .expect("a test fixture is a payload"),
                fairness_key: Some(string8("tenant-7")),
                weight: Some(5),
                delay_ms: Some(60_000),
                idempotency_key: Some(string8("order-42")),
            }])),
        }
    }

    /// The same frame with nothing optional set: no headers, no absent-field values.
    fn a_bare_enqueue() -> Frame<'static> {
        Frame {
            request_id: 12,
            body: FrameBody::Enqueue(enqueue_batch(alloc::vec![EnqueueMessage {
                queue: string8("orders"),
                headers: Headers::default(),
                payload: Payload::default(),
                fairness_key: None,
                weight: None,
                delay_ms: None,
                idempotency_key: None,
            }])),
        }
    }

    /// A frame carrying `headers` on its one message and nothing else optional.
    fn an_enqueue_with_headers<'a>(request_id: u32, headers: &[(&'a str, &'a str)]) -> Frame<'a> {
        let mut pairs = alloc::vec::Vec::new();
        for (key, value) in headers {
            pairs.push((string8(key), string8(value)));
        }

        Frame {
            request_id,
            body: FrameBody::Enqueue(enqueue_batch(alloc::vec![EnqueueMessage {
                queue: string8("orders"),
                headers: Headers::try_from(pairs).expect("a test fixture fits a header map"),
                payload: Payload::default(),
                fairness_key: None,
                weight: None,
                delay_ms: None,
                idempotency_key: None,
            }])),
        }
    }

    #[test]
    fn a_frame_built_from_owned_values_outlives_them() {
        // The point of the owned half: a producer assembling a batch at runtime has no
        // buffer to borrow from, and the frame must outlive the strings it was built
        // from rather than pin them.
        let frame: Frame<'static> = {
            let queue = alloc::string::String::from("orders");
            let body = alloc::vec![0xDEu8, 0xAD];

            Frame {
                request_id: 21,
                body: FrameBody::Enqueue(enqueue_batch(alloc::vec![EnqueueMessage {
                    queue: String8::try_from(queue).expect("6 bytes is a string8"),
                    headers: Headers::default(),
                    payload: Payload::try_from(body).expect("2 bytes is a payload"),
                    fairness_key: None,
                    weight: None,
                    delay_ms: None,
                    idempotency_key: None,
                }])),
            }
        };

        let mut out = [0u8; 64];
        let written = encode(&frame, &mut out, DEFAULT_MAX_FRAME_BODY_LEN)
            .expect("a 64-byte buffer is plenty");

        match decode(&out[..written]) {
            Decoded::Success { frame: decoded, .. } => assert_eq!(decoded, frame),
            other => panic!("what we encoded should decode, got {other:?}"),
        }
    }

    #[test]
    fn an_enqueue_frame_round_trips() {
        let frame = an_enqueue();

        let mut out = [0u8; 128];
        let written = encode(&frame, &mut out, DEFAULT_MAX_FRAME_BODY_LEN)
            .expect("a 128-byte buffer is plenty");

        match decode(&out[..written]) {
            Decoded::Success {
                frame: decoded,
                consumed,
                ..
            } => {
                assert_eq!(consumed, written, "decode consumed what encode wrote");
                assert_eq!(decoded, frame);
            }
            other => panic!("what we encoded should decode, got {other:?}"),
        }
    }

    #[test]
    fn an_enqueue_with_nothing_optional_set_round_trips() {
        // No presence flag on the header map, so "no headers" is a count of zero.
        let frame = a_bare_enqueue();

        let mut out = [0u8; 64];
        let written = encode(&frame, &mut out, DEFAULT_MAX_FRAME_BODY_LEN)
            .expect("a 64-byte buffer is plenty");

        match decode(&out[..written]) {
            Decoded::Success { frame: decoded, .. } => assert_eq!(decoded, frame),
            other => panic!("what we encoded should decode, got {other:?}"),
        }
    }

    #[test]
    fn several_messages_in_one_enqueue_keep_their_own_fields() {
        // protocol.md, "Enqueue (0x10)": each message names its own queue and shares
        // nothing with the others.
        let frame = Frame {
            request_id: 13,
            body: FrameBody::Enqueue(enqueue_batch(alloc::vec![
                EnqueueMessage {
                    queue: string8("orders"),
                    headers: Headers::try_from(alloc::vec![(string8("a"), string8("1"))])
                        .expect("a test fixture fits a header map"),
                    payload: Payload::default(),
                    fairness_key: Some(string8("tenant-1")),
                    weight: None,
                    delay_ms: None,
                    idempotency_key: None,
                },
                EnqueueMessage {
                    queue: string8("emails"),
                    headers: Headers::default(),
                    payload: Payload::try_from(&[0x00u8][..]).expect("a test fixture is a payload"),
                    fairness_key: None,
                    weight: Some(9),
                    delay_ms: Some(1),
                    idempotency_key: None,
                },
                EnqueueMessage {
                    queue: string8("webhooks"),
                    headers: Headers::try_from(alloc::vec![
                        (string8("b"), string8("2")),
                        (string8("c"), string8("3"))
                    ])
                    .expect("a test fixture fits a header map"),
                    payload: Payload::try_from(&[0xFFu8, 0x00, 0x7F][..])
                        .expect("a test fixture is a payload"),
                    fairness_key: None,
                    weight: None,
                    delay_ms: None,
                    idempotency_key: Some(string8("hook-7")),
                },
            ])),
        };

        let mut out = [0u8; 256];
        let written = encode(&frame, &mut out, DEFAULT_MAX_FRAME_BODY_LEN)
            .expect("a 256-byte buffer is plenty");

        match decode(&out[..written]) {
            Decoded::Success { frame: decoded, .. } => assert_eq!(decoded, frame),
            other => panic!("what we encoded should decode, got {other:?}"),
        }
    }

    #[test]
    fn every_header_in_the_map_survives_in_order() {
        // Ascending keys, so the fixture holds whatever the map is represented by.
        let frame = an_enqueue_with_headers(14, &[("a", "1"), ("b", "2"), ("c", "3")]);

        let mut out = [0u8; 64];
        let written = encode(&frame, &mut out, DEFAULT_MAX_FRAME_BODY_LEN)
            .expect("a 64-byte buffer is plenty");

        let Decoded::Success { frame: decoded, .. } = decode(&out[..written]) else {
            panic!("what we encoded should decode");
        };
        let FrameBody::Enqueue(decoded) = decoded.body else {
            panic!("expected an enqueue body");
        };
        let headers = &decoded[0].headers;

        assert_eq!(headers.len(), 3);
        for (i, (key, value)) in headers.iter().enumerate() {
            assert_eq!(key.as_str(), ["a", "b", "c"][i]);
            assert_eq!(value.as_str(), ["1", "2", "3"][i]);
        }
    }

    #[test]
    fn every_truncation_of_an_enqueue_body_is_invalid() {
        // A body is bounded by its own prefix, so ending inside a field is Invalid,
        // never Incomplete, and never a panic.
        let frame = an_enqueue();
        let mut out = [0u8; 128];
        let written = encode(&frame, &mut out, DEFAULT_MAX_FRAME_BODY_LEN)
            .expect("a 128-byte buffer is plenty");
        let body = &out[LENGTH_PREFIX_LEN..written];

        for len in FRAME_HEADER_LEN..body.len() {
            let mut buf = alloc::vec::Vec::new();
            buf.extend_from_slice(&(len as u32).to_be_bytes());
            buf.extend_from_slice(&body[..len]);

            match decode(&buf) {
                Decoded::Invalid(_) => {}
                other => panic!(
                    "a {len}-byte body stops inside a field of a {}-byte one, got {other:?}",
                    body.len()
                ),
            }
        }
    }

    #[test]
    fn an_enqueue_presence_flag_that_is_neither_0_nor_1_is_invalid() {
        // protocol.md, "Encoding Primitives": only 0 and 1 are defined. The four
        // optionals end a message, so absent they are the body's last four bytes.
        let frame = a_bare_enqueue();
        let mut out = [0u8; 64];
        let written = encode(&frame, &mut out, DEFAULT_MAX_FRAME_BODY_LEN)
            .expect("a 64-byte buffer is plenty");

        for (from_end, field) in [
            (4, MessageField::FairnessKey),
            (3, MessageField::Weight),
            (2, MessageField::DelayMs),
            (1, MessageField::IdempotencyKey),
        ] {
            let mut buf = out[..written].to_vec();
            buf[written - from_end] = 0x02;

            assert_eq!(
                decode(&buf),
                Decoded::Invalid(FrameError::Body(BodyError::Enqueue(
                    EnqueueError::Message {
                        index: 0,
                        fault: FieldFault {
                            field,
                            source: FieldError::PresenceFlag(2),
                        },
                    }
                ))),
                "0x02 is not a presence byte, {from_end} bytes from the end of the body"
            );
        }
    }

    #[test]
    fn a_header_value_that_is_not_utf8_is_invalid() {
        // Replacing a byte rather than appending one keeps every length honest, so
        // the only fault is the encoding of one value.
        let frame = an_enqueue_with_headers(15, &[("k", "v")]);
        let mut out = [0u8; 64];
        let written = encode(&frame, &mut out, DEFAULT_MAX_FRAME_BODY_LEN)
            .expect("a 64-byte buffer is plenty");

        // "orders" and "k" hold no 'v', so this is the header value's only byte.
        let value = out[..written]
            .iter()
            .position(|&b| b == b'v')
            .expect("the header value is in there");
        out[value] = 0xFF;

        match decode(&out[..written]) {
            Decoded::Invalid(FrameError::Body(BodyError::Enqueue(EnqueueError::Message {
                fault:
                    FieldFault {
                        field: MessageField::Headers,
                        source:
                            FieldError::Headers(InvalidHeaderError::InvalidValue(
                                String8DecodeError::InvalidUtf8(_),
                            )),
                    },
                ..
            }))) => {}
            other => panic!("0xFF is not UTF-8, so this header value cannot decode: {other:?}"),
        }
    }

    #[test]
    fn an_enqueue_error_reads_as_the_path_to_the_byte_that_failed() {
        // The deepest chain in the codec: frame, body, message, field, entry, fault.
        let frame = an_enqueue_with_headers(16, &[("k", "v")]);
        let mut out = [0u8; 64];
        let written = encode(&frame, &mut out, DEFAULT_MAX_FRAME_BODY_LEN)
            .expect("a 64-byte buffer is plenty");
        let value = out[..written]
            .iter()
            .position(|&b| b == b'v')
            .expect("the header value is in there");
        out[value] = 0xFF;

        let Decoded::Invalid(err) = decode(&out[..written]) else {
            panic!("expected an invalid frame");
        };
        assert_eq!(
            alloc::format!("{err}"),
            "frame body: enqueue: message 0: headers: value: not utf-8: \
             invalid utf-8 sequence of 1 bytes from index 0"
        );
    }

    #[test]
    fn trailing_bytes_inside_an_enqueue_body_are_tolerated() {
        // protocol.md, "What needs a new version": "a reader tolerates trailing bytes
        // it does not understand". Still consumed, or the next frame starts inside it.
        let frame = an_enqueue();
        let mut out = [0u8; 128];
        let written = encode(&frame, &mut out, DEFAULT_MAX_FRAME_BODY_LEN)
            .expect("a 128-byte buffer is plenty");

        let mut buf = out[..written].to_vec();
        buf.extend_from_slice(&[0xAB, 0xCD]); // a field some later version appended
        let body_len = (buf.len() - LENGTH_PREFIX_LEN) as u32;
        buf[..LENGTH_PREFIX_LEN].copy_from_slice(&body_len.to_be_bytes());

        match decode(&buf) {
            Decoded::Success {
                frame: decoded,
                consumed,
                ..
            } => {
                assert_eq!(decoded, frame, "the fields this version knows still decode");
                assert_eq!(consumed, buf.len(), "and the bytes it does not are skipped");
            }
            other => panic!("expected a frame, got {other:?}"),
        }
    }

    #[test]
    fn an_enqueue_body_opens_with_its_message_count() {
        // protocol.md, "Enqueue (0x10)": the body opens with `[u32 message_count]`,
        // which lands at prefix (4) + header (6) = offset 10.
        let frame = an_enqueue();
        let mut out = [0u8; 128];
        let written = encode(&frame, &mut out, DEFAULT_MAX_FRAME_BODY_LEN)
            .expect("a 128-byte buffer is plenty");
        assert!(written > 12, "a body with a count is at least 8 bytes");

        assert_eq!(
            &out[10..12],
            &1u16.to_be_bytes(),
            "one message is a batch of one, so the body still opens with a count, but \
             these four bytes are {:?}",
            &out[10..12]
        );
    }

    #[test]
    fn a_spec_shaped_enqueue_body_round_trips_byte_for_byte() {
        // Two messages in the layout protocol.md gives for Enqueue (0x10). The only
        // fixture here not produced by `encode`, so the only one that catches both
        // halves of the codec agreeing on the wrong layout.
        const SPEC_BODY: [u8; 58] = [
            0, 0, 0, 54, // a 54-byte body
            0x10, 0x00, 0, 0, 0, 9, // opcode = Enqueue, no flags, request_id = 9
            0, 2, // message_count = 2
            // message 1
            1, b'a', // queue = "a"
            0, 1, 1, b'k', 1, b'v', // headers = one entry, "k" -> "v"
            0, 0, 0, 2, 0xFF, 0x00, // payload = two bytes, and not UTF-8
            0,    // fairness_key absent
            0,    // weight absent
            0,    // delay_ms absent
            0,    // idempotency_key absent
            // message 2
            1, b'b', // queue = "b"
            0, 0, // no headers
            0, 0, 0, 0, // an empty payload is a length of zero, not an absent field
            1, 1, b'f', // fairness_key = "f"
            1, 0, 0, 0, 5, // weight = 5
            1, 0, 0, 0, 0, 0, 0, 0, 7, // delay_ms = 7
            1, 1, b'i', // idempotency_key = "i"
        ];

        let Decoded::Success {
            frame, consumed, ..
        } = decode(&SPEC_BODY)
        else {
            panic!(
                "a two-message Enqueue is what the spec defines, got {:?}",
                decode(&SPEC_BODY)
            );
        };
        assert_eq!(consumed, SPEC_BODY.len(), "prefix (4) + body (54)");

        let mut out = [0u8; 64];
        let written = encode(&frame, &mut out, DEFAULT_MAX_FRAME_BODY_LEN)
            .expect("a 64-byte buffer is plenty");
        assert_eq!(
            &out[..written],
            &SPEC_BODY[..],
            "re-encoding what we decoded should give the bytes back"
        );
    }

    #[test]
    fn an_enqueue_presence_flag_error_names_the_byte_it_rejected() {
        // The error a server puts in an `Error` frame's `message` has to say which
        // byte it refused. "is 0, which is neither 0 nor 1" names the message index
        // twice and contradicts itself.
        let frame = a_bare_enqueue();
        let mut out = [0u8; 64];
        let written = encode(&frame, &mut out, DEFAULT_MAX_FRAME_BODY_LEN)
            .expect("a 64-byte buffer is plenty");
        out[written - 2] = 0x02; // the delay_ms presence flag

        let Decoded::Invalid(err) = decode(&out[..written]) else {
            panic!("expected an invalid frame");
        };
        assert_eq!(
            alloc::format!("{err}"),
            "frame body: enqueue: message 0: delay_ms: presence flag is 2, \
             which is neither 0 nor 1"
        );
    }

    #[test]
    fn a_payload_of_arbitrary_bytes_survives_a_round_trip() {
        // protocol.md, "Encoding Primitives": a payload is `bytes`, not a string - a
        // `u32` length and then raw bytes. Nothing may be validated, replaced or
        // trimmed: a producer sending protobuf, a gzip stream or an interior NUL gets
        // back what it sent.
        static PAYLOAD: [u8; 7] = [0x00, 0xFF, 0x80, 0xC0, 0x00, 0x1B, 0x7F];
        let frame = Frame {
            request_id: 19,
            body: FrameBody::Enqueue(enqueue_batch(alloc::vec![EnqueueMessage {
                queue: string8("orders"),
                headers: Headers::default(),
                payload: Payload::try_from(&PAYLOAD[..]).expect("seven bytes is a payload"),
                fairness_key: None,
                weight: None,
                delay_ms: None,
                idempotency_key: None,
            }])),
        };

        let mut out = [0u8; 64];
        let written = encode(&frame, &mut out, DEFAULT_MAX_FRAME_BODY_LEN)
            .expect("a 64-byte buffer is plenty");

        let Decoded::Success { frame: decoded, .. } = decode(&out[..written]) else {
            panic!("what we encoded should decode");
        };
        let FrameBody::Enqueue(decoded) = decoded.body else {
            panic!("expected an enqueue body");
        };
        assert_eq!(decoded[0].payload.as_slice(), &PAYLOAD[..]);
    }

    #[test]
    fn a_payload_shorter_than_its_declared_length_is_invalid() {
        // In a one-message frame with no headers and nothing optional set, the payload
        // length is the four bytes before the four presence flags.
        let frame = a_bare_enqueue();
        let mut out = [0u8; 64];
        let written = encode(&frame, &mut out, DEFAULT_MAX_FRAME_BODY_LEN)
            .expect("a 64-byte buffer is plenty");

        let length = written - 8;
        out[length..length + 4].copy_from_slice(&5u32.to_be_bytes());

        assert_eq!(
            decode(&out[..written]),
            Decoded::Invalid(FrameError::Body(BodyError::Enqueue(
                EnqueueError::Message {
                    index: 0,
                    fault: FieldFault {
                        field: MessageField::Payload,
                        source: FieldError::Truncated {
                            expected: 5,
                            found: 4,
                        },
                    },
                }
            ))),
            "a payload declaring 5 bytes with 4 left in the body cannot decode"
        );
    }

    #[test]
    fn a_payload_declaring_u32_max_is_invalid() {
        // The same frame on a 32-bit target panics instead: `Reader::bytes` adds the
        // declared length to its cursor before checking it. CI builds this crate for
        // wasm32 but never runs the tests there, so only the arithmetic is at issue.
        const FRAME: [u8; 20] = [
            0, 0, 0, 16, // a 16-byte body
            0x10, 0x00, 0, 0, 0, 7, // opcode = Enqueue, no flags, request_id = 7
            0, 1, // message_count = 1
            1, b'q', // queue = "q"
            0, 0, // no headers
            0xFF, 0xFF, 0xFF, 0xFF, // payload declares 4,294,967,295 bytes
        ];

        assert_eq!(
            decode(&FRAME),
            Decoded::Invalid(FrameError::Body(BodyError::Enqueue(
                EnqueueError::Message {
                    index: 0,
                    fault: FieldFault {
                        field: MessageField::Payload,
                        source: FieldError::Truncated {
                            expected: u32::MAX as usize,
                            found: 0,
                        },
                    },
                }
            )))
        );
    }

    #[test]
    fn encode_refuses_a_body_above_the_maximum_it_was_given() {
        // A payload is the first body big enough to cross the maximum, and writing it
        // out would mean handing a peer a frame it is obliged to reject.
        let payload = alloc::vec![0u8; DEFAULT_MAX_FRAME_BODY_LEN as usize];
        let frame = Frame {
            request_id: 31,
            body: FrameBody::Enqueue(enqueue_batch(alloc::vec![EnqueueMessage {
                queue: string8("orders"),
                headers: Headers::default(),
                payload: Payload::try_from(payload).expect("16 MiB is a payload"),
                fairness_key: None,
                weight: None,
                delay_ms: None,
                idempotency_key: None,
            }])),
        };

        let mut out = alloc::vec![0u8; DEFAULT_MAX_FRAME_BODY_LEN as usize + 64];

        assert_eq!(
            encode(&frame, &mut out, DEFAULT_MAX_FRAME_BODY_LEN),
            Err(EncodeError::BodyAboveMaximum {
                len: 16_777_241,
                max: DEFAULT_MAX_FRAME_BODY_LEN,
            }),
            "a 16 MiB payload does not fit a 16 MiB body: the frame header and the \
             message's own fields are in there too"
        );
    }

    #[test]
    fn a_body_of_exactly_the_maximum_is_written() {
        // The maximum is the largest legal body, not the first illegal one.
        let payload = alloc::vec![0u8; DEFAULT_MAX_FRAME_BODY_LEN as usize - 25];
        let frame = Frame {
            request_id: 32,
            body: FrameBody::Enqueue(enqueue_batch(alloc::vec![EnqueueMessage {
                queue: string8("orders"),
                headers: Headers::default(),
                payload: Payload::try_from(payload).expect("a payload"),
                fairness_key: None,
                weight: None,
                delay_ms: None,
                idempotency_key: None,
            }])),
        };

        let mut out = alloc::vec![0u8; DEFAULT_MAX_FRAME_BODY_LEN as usize + 64];
        let written = encode(&frame, &mut out, DEFAULT_MAX_FRAME_BODY_LEN)
            .expect("a body of exactly the maximum is legal");

        assert_eq!(
            written - LENGTH_PREFIX_LEN,
            DEFAULT_MAX_FRAME_BODY_LEN as usize
        );
        assert!(matches!(decode(&out[..written]), Decoded::Success { .. }));
    }

    #[test]
    fn a_repeated_header_key_is_invalid() {
        // protocol.md types the field `map<string8,string8>`: a key appears once.
        //
        // one message, whose header map declares two entries under the same key
        const DUPLICATE_KEY: [u8; 32] = [
            0, 0, 0, 28, // a 28-byte body
            0x10, 0x00, 0, 0, 0, 1, // opcode = Enqueue, no flags, request_id = 1
            0, 1, // message_count = 1
            1, b'a', // queue = "a"
            0, 2, // headers declares two entries
            1, b'k', 1, b'v', // "k" -> "v"
            1, b'k', 1, b'w', // "k" again, with a different value
            0, 0, 0, 0, // an empty payload
            0, 0, 0, 0, // nothing optional set
        ];

        match decode(&DUPLICATE_KEY) {
            Decoded::Invalid(FrameError::Body(BodyError::Enqueue(EnqueueError::Message {
                fault:
                    FieldFault {
                        field: MessageField::Headers,
                        source: FieldError::Headers(InvalidHeaderError::DuplicateKey(_)),
                    },
                ..
            }))) => {}
            other => panic!(
                "a map cannot carry one key twice, and whichever value won would be a \
                 choice every SDK has to make the same way: {other:?}"
            ),
        }
    }

    #[test]
    fn a_repeated_key_is_refused_before_it_reaches_the_wire() {
        // The same invariant on the way out, where the alternative is a silently
        // dropped header.
        let duplicated = alloc::vec![
            (string8("tenant"), string8("acme")),
            (string8("tenant"), string8("other")),
        ];

        match Headers::try_from(duplicated) {
            Err(HeadersError::DuplicateKey(_)) => {}
            other => panic!(
                "a repeated key has no honest encoding - the count would say two and \
                 the map would hold one - so this cannot be {other:?}"
            ),
        }
    }

    #[test]
    fn a_header_map_decodes_in_any_order_and_encodes_in_one() {
        // Order is not significant on the wire, but `Key::all_headers()` hashes every
        // header, so the decoded map has exactly one order: ascending by key.
        //
        // the same message, "b" before "a" on the wire
        const UNSORTED: [u8; 32] = [
            0, 0, 0, 28, // a 28-byte body
            0x10, 0x00, 0, 0, 0, 1, // opcode = Enqueue, no flags, request_id = 1
            0, 1, // message_count = 1
            1, b'a', // queue = "a"
            0, 2, // two entries
            1, b'b', 1, b'2', // "b" -> "2"
            1, b'a', 1, b'1', // "a" -> "1"
            0, 0, 0, 0, // an empty payload
            0, 0, 0, 0, // nothing optional set
        ];

        let Decoded::Success { frame, .. } = decode(&UNSORTED) else {
            panic!(
                "an unsorted map is a legal map, got {:?}",
                decode(&UNSORTED)
            );
        };

        let mut out = [0u8; 64];
        let written = encode(&frame, &mut out, DEFAULT_MAX_FRAME_BODY_LEN)
            .expect("a 64-byte buffer is plenty");

        let mut sorted = UNSORTED;
        sorted[16..24].copy_from_slice(&[1, b'a', 1, b'1', 1, b'b', 1, b'2']);
        assert_eq!(
            &out[..written],
            &sorted[..],
            "the entries come back ascending, whatever order they arrived in"
        );
    }

    #[test]
    fn an_empty_batch_round_trips() {
        // A count of zero is a legal frame. Whether a server wants one is its business.
        let frame = Frame {
            request_id: 20,
            body: FrameBody::Enqueue(enqueue_batch(alloc::vec::Vec::new())),
        };

        let mut out = [0u8; 32];
        let written = encode(&frame, &mut out, DEFAULT_MAX_FRAME_BODY_LEN)
            .expect("a 32-byte buffer is plenty");
        assert_eq!(written, 12, "prefix (4) + header (6) + count (2)");

        match decode(&out[..written]) {
            Decoded::Success { frame: decoded, .. } => assert_eq!(decoded, frame),
            other => panic!("what we encoded should decode, got {other:?}"),
        }
    }

    #[test]
    fn a_message_count_larger_than_the_body_names_the_missing_message() {
        let frame = an_enqueue();
        let mut out = [0u8; 128];
        let written = encode(&frame, &mut out, DEFAULT_MAX_FRAME_BODY_LEN)
            .expect("a 128-byte buffer is plenty");
        out[10..12].copy_from_slice(&3u16.to_be_bytes());

        assert_eq!(
            decode(&out[..written]),
            Decoded::Invalid(FrameError::Body(BodyError::Enqueue(
                EnqueueError::Message {
                    index: 1,
                    fault: FieldFault {
                        field: MessageField::Queue,
                        source: FieldError::Ended,
                    },
                }
            ))),
            "message 0 is all the body held, so the fault belongs to message 1"
        );
    }

    #[test]
    fn an_absurd_message_count_fails_before_it_allocates() {
        // 65,535 messages in an 8-byte body. Reading entry by entry means this costs
        // one failed read; reserving for the declared count would mean 65,535 structs,
        // which is 9 MB of EnqueueMessage.
        let mut buf = alloc::vec::Vec::new();
        buf.extend_from_slice(&8u32.to_be_bytes());
        buf.extend_from_slice(&[0x10, 0x00, 0, 0, 0, 1]);
        buf.extend_from_slice(&u16::MAX.to_be_bytes());

        assert_eq!(
            decode(&buf),
            Decoded::Invalid(FrameError::Body(BodyError::Enqueue(
                EnqueueError::Message {
                    index: 0,
                    fault: FieldFault {
                        field: MessageField::Queue,
                        source: FieldError::Ended,
                    },
                }
            )))
        );
    }

    #[test]
    fn every_truncation_of_an_enqueue_is_incomplete() {
        // The prefix is left alone here, so each cut is a frame still arriving - the
        // opposite of `every_truncation_of_an_enqueue_body_is_invalid`.
        let frame = an_enqueue();
        let mut out = [0u8; 128];
        let written = encode(&frame, &mut out, DEFAULT_MAX_FRAME_BODY_LEN)
            .expect("a 128-byte buffer is plenty");

        for n in 0..written {
            assert_eq!(
                decode(&out[..n]),
                Decoded::Incomplete,
                "{n} of {written} bytes have arrived"
            );
        }
    }

    #[test]
    fn encode_refuses_every_buffer_too_small_for_an_enqueue() {
        // A message writes a payload through `Writer::bytes`, which is the one write
        // whose length comes from the value rather than from its type.
        let frame = an_enqueue();
        let mut out = [0u8; 128];
        let written = encode(&frame, &mut out, DEFAULT_MAX_FRAME_BODY_LEN)
            .expect("a 128-byte buffer is plenty");

        let mut backing = alloc::vec![0u8; written];
        for size in 0..written {
            assert_eq!(
                encode(&frame, &mut backing[..size], DEFAULT_MAX_FRAME_BODY_LEN),
                Err(EncodeError::BufferTooSmall),
                "a {size}-byte buffer cannot hold a {written}-byte frame"
            );
        }
        assert_eq!(
            encode(&frame, &mut backing, DEFAULT_MAX_FRAME_BODY_LEN),
            Ok(written)
        );
    }

    #[test]
    fn an_empty_optional_string_is_not_an_absent_one() {
        // `optional<string8>` is a presence byte and then a length, so an empty string
        // is present with a length of zero. Two bytes on the wire against one.
        let with_empty = |fairness_key| Frame {
            request_id: 21,
            body: FrameBody::Enqueue(enqueue_batch(alloc::vec![EnqueueMessage {
                queue: string8("orders"),
                headers: Headers::default(),
                payload: Payload::default(),
                fairness_key,
                weight: None,
                delay_ms: None,
                idempotency_key: None,
            }])),
        };

        let empty = with_empty(Some(string8("")));
        let absent = with_empty(None);

        let mut out = [0u8; 64];
        let empty_len = encode(&empty, &mut out, DEFAULT_MAX_FRAME_BODY_LEN)
            .expect("a 64-byte buffer is plenty");
        let Decoded::Success { frame: decoded, .. } = decode(&out[..empty_len]) else {
            panic!("what we encoded should decode");
        };
        assert_eq!(decoded, empty);

        let absent_len = encode(&absent, &mut out, DEFAULT_MAX_FRAME_BODY_LEN)
            .expect("a 64-byte buffer is plenty");
        assert_eq!(
            empty_len,
            absent_len + 1,
            "the empty string costs its length byte"
        );
    }

    #[test]
    fn a_zero_weight_is_not_an_absent_weight() {
        // protocol.md: an absent weight means the queue's default, which is 1. A weight
        // of 0 is a value the producer chose, so the two cannot collapse.
        let frame = Frame {
            request_id: 22,
            body: FrameBody::Enqueue(enqueue_batch(alloc::vec![EnqueueMessage {
                queue: string8("orders"),
                headers: Headers::default(),
                payload: Payload::default(),
                fairness_key: None,
                weight: Some(0),
                delay_ms: Some(0),
                idempotency_key: None,
            }])),
        };

        let mut out = [0u8; 64];
        let written = encode(&frame, &mut out, DEFAULT_MAX_FRAME_BODY_LEN)
            .expect("a 64-byte buffer is plenty");

        let Decoded::Success { frame: decoded, .. } = decode(&out[..written]) else {
            panic!("what we encoded should decode");
        };
        let FrameBody::Enqueue(decoded) = decoded.body else {
            panic!("expected an enqueue body");
        };
        assert_eq!(decoded[0].weight, Some(0));
        assert_eq!(decoded[0].delay_ms, Some(0));
    }

    #[test]
    fn more_headers_than_a_u8_can_count() {
        // 300 entries needs both bytes of the count. A `u8` would read 44.
        let mut keys = alloc::vec::Vec::new();
        for i in 0..300u32 {
            keys.push(alloc::format!("k{i}"));
        }
        let mut entries = alloc::vec::Vec::new();
        for key in &keys {
            entries.push((string8(key), string8("v")));
        }
        let frame = Frame {
            request_id: 23,
            body: FrameBody::Enqueue(enqueue_batch(alloc::vec![EnqueueMessage {
                queue: string8("orders"),
                headers: Headers::try_from(entries).expect("300 entries is legal"),
                payload: Payload::default(),
                fairness_key: None,
                weight: None,
                delay_ms: None,
                idempotency_key: None,
            }])),
        };

        let mut out = alloc::vec![0u8; 4096];
        let written =
            encode(&frame, &mut out, DEFAULT_MAX_FRAME_BODY_LEN).expect("4 KiB is plenty");

        match decode(&out[..written]) {
            Decoded::Success { frame: decoded, .. } => assert_eq!(decoded, frame),
            other => panic!("what we encoded should decode, got {other:?}"),
        }
    }

    #[test]
    fn a_queue_name_that_is_not_utf8_names_the_queue() {
        let frame = Frame {
            request_id: 24,
            body: FrameBody::Enqueue(enqueue_batch(alloc::vec![EnqueueMessage {
                queue: string8("qq"),
                headers: Headers::default(),
                payload: Payload::default(),
                fairness_key: None,
                weight: None,
                delay_ms: None,
                idempotency_key: None,
            }])),
        };

        let mut out = [0u8; 64];
        let written = encode(&frame, &mut out, DEFAULT_MAX_FRAME_BODY_LEN)
            .expect("a 64-byte buffer is plenty");

        // prefix (4) + header (6) + count (2) + length byte (1)
        out[13] = 0xFF;

        match decode(&out[..written]) {
            Decoded::Invalid(FrameError::Body(BodyError::Enqueue(EnqueueError::Message {
                fault:
                    FieldFault {
                        field: MessageField::Queue,
                        source: FieldError::String8(String8DecodeError::InvalidUtf8(_)),
                    },
                ..
            }))) => {}
            other => panic!("expected an invalid queue name, got {other:?}"),
        }
    }

    #[test]
    fn one_messages_headers_do_not_reach_another() {
        // Each message carries its own map, so the same key in two of them is not a
        // duplicate.
        let message = |value| EnqueueMessage {
            queue: string8("orders"),
            headers: Headers::try_from(alloc::vec![(string8("tenant"), string8(value))])
                .expect("one entry is legal"),
            payload: Payload::default(),
            fairness_key: None,
            weight: None,
            delay_ms: None,
            idempotency_key: None,
        };
        let frame = Frame {
            request_id: 25,
            body: FrameBody::Enqueue(enqueue_batch(alloc::vec![
                message("acme"),
                message("other")
            ])),
        };

        let mut out = [0u8; 128];
        let written = encode(&frame, &mut out, DEFAULT_MAX_FRAME_BODY_LEN)
            .expect("a 128-byte buffer is plenty");

        match decode(&out[..written]) {
            Decoded::Success { frame: decoded, .. } => assert_eq!(decoded, frame),
            other => panic!("two messages may share a header key, got {other:?}"),
        }
    }

    #[test]
    fn field_names_are_the_names_protocol_md_uses() {
        // These strings reach an SDK inside an `Error` frame's message, so a rename of
        // a Rust variant must not rename a protocol field.
        for (field, name) in [
            (MessageField::Queue, "queue"),
            (MessageField::Headers, "headers"),
            (MessageField::Payload, "payload"),
            (MessageField::FairnessKey, "fairness_key"),
            (MessageField::Weight, "weight"),
            (MessageField::DelayMs, "delay_ms"),
            (MessageField::IdempotencyKey, "idempotency_key"),
        ] {
            assert_eq!(alloc::format!("{field}"), name);
        }

        for (field, name) in [
            (HandshakeField::MinVersion, "min_version"),
            (HandshakeField::MaxVersion, "max_version"),
            (HandshakeField::ApiKey, "api_key"),
            (HandshakeField::ClientCapabilities, "client_capabilities"),
        ] {
            assert_eq!(alloc::format!("{field}"), name);
        }

        for (field, name) in [
            (HandshakeOkField::NegotiatedVersion, "negotiated_version"),
            (HandshakeOkField::NodeId, "node_id"),
            (HandshakeOkField::MaxFrameSize, "max_frame_size"),
            (HandshakeOkField::ServerCapabilities, "server_capabilities"),
        ] {
            assert_eq!(alloc::format!("{field}"), name);
        }
    }

    #[test]
    fn no_single_byte_mutation_of_an_enqueue_can_panic() {
        // Every byte of a valid frame, replaced by every interesting value. A decoder
        // reads lengths, counts and presence flags out of these bytes, so the answer may
        // be anything - but it has to be an answer.
        let frame = an_enqueue();
        let mut out = [0u8; 128];
        let written = encode(&frame, &mut out, DEFAULT_MAX_FRAME_BODY_LEN)
            .expect("a 128-byte buffer is plenty");

        for i in 0..written {
            for byte in [0x00, 0x01, 0x02, 0x7F, 0x80, 0xFE, 0xFF] {
                let mut buf = out[..written].to_vec();
                buf[i] = byte;
                let _ = decode(&buf);
            }
        }
    }

    #[test]
    fn a_decoded_payload_borrows_the_buffer_it_was_decoded_from() {
        // The point of the whole lifetime: a 16 MiB message costs no copy to decode.
        let frame = an_enqueue();
        let mut out = [0u8; 128];
        let written = encode(&frame, &mut out, DEFAULT_MAX_FRAME_BODY_LEN)
            .expect("a 128-byte buffer is plenty");

        let Decoded::Success { frame: decoded, .. } = decode(&out[..written]) else {
            panic!("what we encoded should decode");
        };
        let FrameBody::Enqueue(decoded) = decoded.body else {
            panic!("expected an enqueue body");
        };

        let within = |bytes: &[u8]| {
            let start = out.as_ptr() as usize;
            let at = bytes.as_ptr() as usize;
            at >= start && at + bytes.len() <= start + out.len()
        };

        assert!(
            within(decoded[0].payload.as_slice()),
            "the payload points into the frame's own bytes"
        );
        assert!(
            within(decoded[0].queue.as_str().as_bytes()),
            "and so does the queue name"
        );
        let (key, value) = decoded[0].headers.iter().next().expect("two headers");
        assert!(
            within(key.as_str().as_bytes()) && within(value.as_str().as_bytes()),
            "and both halves of every header"
        );
    }
}
