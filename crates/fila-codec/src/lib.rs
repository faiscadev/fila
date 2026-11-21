//! Fila's wire protocol: frames in, frames out, and no I/O.
//!
//! See docs/protocol.md. A frame is a `u32` length followed by a body, and the body
//! opens with a 6-byte header naming the operation.

#![no_std]

extern crate alloc;

mod frame;
mod handshake;
mod reader;
mod string8;
mod writer;

pub use frame::{
    BodyError, FlagsError, Frame, FrameBody, FrameError, HeaderError, Opcode, OpcodeError,
};
pub use handshake::{Handshake, HandshakeError, HandshakeOk, HandshakeOkError};
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

// TODO(#172): this limit is per connection, not per protocol - default 16 MiB,
// configurable, advertised in HandshakeOk, and 4 KiB before a handshake completes, so it
// belongs to the caller. Note that `encode` does not check it at all today: once a frame
// can carry a payload, it will be able to emit one `decode` rejects.
pub(crate) const MAX_FRAME_BODY_LEN: u32 = 1024 * 1024 * 16;

/// What a buffer held when `decode` looked at it.
#[derive(Debug, PartialEq)]
pub enum Decoded {
    Success {
        frame: Frame,
        length: u32,
        consumed: usize,
    },
    /// The declared frame has not arrived in full. More bytes may change this answer.
    Incomplete,
    /// The declared frame arrived and does not add up. More bytes will not help.
    Invalid(FrameError),
}

/// Reads the frame at the front of `buf`.
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
/// `None` means `buf` could not fit the frame. The buffer may have been written to
/// before that was discovered, so its contents are unspecified unless this returns
/// `Some`.
pub fn encode(frame: &Frame, buf: &mut [u8]) -> Option<usize> {
    let mut writer = Writer::new(buf);
    frame.write(&mut writer)?;
    Some(writer.pos())
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
                HandshakeError::InvalidApiKey(String8DecodeError::Truncated {
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
            Decoded::Invalid(FrameError::Body(BodyError::Handshake(
                HandshakeError::MissingMinVersion
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

    #[test]
    fn an_error_reads_as_the_path_to_the_byte_that_failed() {
        // Each level prefixes the one below it, so one line says where in the frame the
        // fault is. This is what a server puts in an `Error` frame's `message`.
        let Decoded::Invalid(err) = decode(&TRUNCATED_STRING) else {
            panic!("expected an invalid frame");
        };
        assert_eq!(
            alloc::format!("{err}"),
            "frame body: handshake: api key: declares 255 bytes with 1 left in the body"
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
            depth, 3,
            "FrameError -> BodyError -> HandshakeError -> String8DecodeError"
        );
    }
}
