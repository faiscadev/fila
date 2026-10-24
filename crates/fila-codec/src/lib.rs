#![no_std]

extern crate alloc;

#[derive(Debug)]
pub struct FrameHeader {
    pub opcode: u8,
    pub request_id: u32,
}

#[derive(Debug)]
pub struct Frame {
    pub header: FrameHeader,
}

#[derive(Debug)]
pub enum FlagsError {
    ContinuationUnsupported,
    ReservedSet { flags: u8 },
}

#[derive(Debug)]
pub enum FrameError {
    Flags(FlagsError),
    LengthBelowHeader { length: u32 },
    LengthAboveMaximum { length: u32 },
}

#[derive(Debug)]
pub enum Decoded {
    Success {
        frame: Frame,
        length: u32,
        consumed: usize,
    },
    Incomplete,
    Invalid(FrameError),
}

fn validate_flags(flags: u8) -> Option<FlagsError> {
    if flags == 0b0000_0001 {
        // continuation is not implemented yet
        Some(FlagsError::ContinuationUnsupported)
    } else if flags != 0b0000_0000 {
        Some(FlagsError::ReservedSet { flags })
    } else {
        None
    }
}

fn extract_frame_header(buf: &[u8]) -> Result<FrameHeader, FlagsError> {
    let buf_frame = &buf[4..];
    let opcode = u8::from_be_bytes([buf_frame[0]]);
    let flags = u8::from_be_bytes([buf_frame[1]]);

    if let Some(err) = validate_flags(flags) {
        return Err(err);
    }

    let request_id = u32::from_be_bytes([buf_frame[2], buf_frame[3], buf_frame[4], buf_frame[5]]);
    Ok(FrameHeader { opcode, request_id })
}

fn extract_frame(buf: &[u8]) -> Result<Frame, FlagsError> {
    let header = extract_frame_header(buf)?;
    Ok(Frame { header })
}

const MAX_FRAME_BODY_LEN: u32 = 1024 * 1024 * 16; // 16 MiB

pub fn decode(buf: &[u8]) -> Decoded {
    if buf.len() < 4 {
        return Decoded::Incomplete;
    }

    let frame_len = u32::from_be_bytes([buf[0], buf[1], buf[2], buf[3]]);

    if frame_len > MAX_FRAME_BODY_LEN {
        return Decoded::Invalid(FrameError::LengthAboveMaximum { length: frame_len });
    }

    if frame_len < 6 {
        return Decoded::Invalid(FrameError::LengthBelowHeader { length: frame_len });
    }

    if buf.len() < (frame_len as usize) + 4 {
        return Decoded::Incomplete;
    }

    let frame = extract_frame(buf);
    match frame {
        Ok(frame) => Decoded::Success {
            frame,
            length: frame_len,
            consumed: (frame_len + 4) as usize,
        },
        Err(err) => Decoded::Invalid(FrameError::Flags(err)),
    }
}

fn encode_u8(value: u8, buf: &mut [u8]) -> Option<usize> {
    let as_bytes = value.to_be_bytes();
    if buf.len() < as_bytes.len() {
        return None;
    }

    let mut cursor = 0;
    for byte in as_bytes {
        buf[cursor] = byte;
        cursor += 1;
    }

    Some(cursor)
}

fn encode_u32(value: u32, buf: &mut [u8]) -> Option<usize> {
    let as_bytes = value.to_be_bytes();
    if buf.len() < as_bytes.len() {
        return None;
    }

    let mut cursor = 0;
    for byte in as_bytes {
        buf[cursor] = byte;
        cursor += 1;
    }

    Some(cursor)
}

fn encode_frame_header(frame_header: &FrameHeader, buf: &mut [u8]) -> Option<usize> {
    let mut cursor = 0;

    cursor += encode_u8(frame_header.opcode, &mut buf[cursor..])?;

    let flags = 0b0000_0000;
    cursor += encode_u8(flags, &mut buf[cursor..])?;

    cursor += encode_u32(frame_header.request_id, &mut buf[cursor..])?;

    Some(cursor)
}

// return None if buf cant fit frame
pub fn encode(frame: &Frame, buf: &mut [u8]) -> Option<usize> {
    // starts at 4 to offset the length
    let mut cursor = 4;
    if buf.len() < cursor {
        return None;
    }

    cursor += encode_frame_header(&frame.header, &mut buf[cursor..])?;

    let frame_length = (cursor - 4) as u32;
    encode_u32(frame_length, &mut buf[..])?;

    Some(cursor)
}

#[cfg(test)]
mod tests {
    use super::*;

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
                assert_eq!(frame.header.opcode, 0x03);
                assert_eq!(frame.header.request_id, 1);
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
            Decoded::Success { frame, .. } => assert_eq!(frame.header.request_id, 0x0A0B0C0D),
            other => panic!("expected a frame, got {other:?}"),
        }
    }

    #[test]
    fn every_truncation_is_incomplete() {
        for n in 0..PING.len() {
            assert!(
                matches!(decode(&PING[..n]), Decoded::Incomplete),
                "{n} of {} bytes should be Incomplete",
                PING.len()
            );
        }
    }

    #[test]
    fn continuation_flag_is_rejected_for_now() {
        let mut buf = PING;
        buf[5] = 0b0000_0001;
        assert!(matches!(
            decode(&buf),
            Decoded::Invalid(FrameError::Flags(FlagsError::ContinuationUnsupported))
        ));
    }

    #[test]
    fn body_shorter_than_a_header_is_invalid() {
        // prefix says 5, but a body must carry the 6-byte header
        let buf = [0, 0, 0, 5, 0x03, 0x00, 0, 0, 0];
        assert!(matches!(
            decode(&buf),
            Decoded::Invalid(FrameError::LengthBelowHeader { length: 5 })
        ));
    }

    #[test]
    fn consumed_points_at_the_next_frame() {
        let mut stream = PING.to_vec();
        stream.extend_from_slice(&PING);
        let Decoded::Success { consumed, .. } = decode(&stream) else {
            panic!("expected the first frame");
        };
        assert!(
            matches!(decode(&stream[consumed..]), Decoded::Success { .. }),
            "draining `consumed` bytes should leave the second frame at the front"
        );
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
        assert!(matches!(
            decode(&buf),
            Decoded::Invalid(FrameError::LengthAboveMaximum { length: 16_777_217 })
        ));
    }

    #[test]
    fn a_reserved_flag_bit_is_invalid() {
        // protocol.md, "Frame Header": bits 1-7 are reserved and must be 0.
        // Accepting them silently means a peer speaking a future version looks
        // fine while its extra semantics are ignored.
        for bit in 1..8 {
            let mut buf = PING;
            buf[5] = 1 << bit;
            assert!(
                matches!(
                    decode(&buf),
                    Decoded::Invalid(FrameError::Flags(FlagsError::ReservedSet { flags })) if flags == buf[5]
                ),
                "flags = {:#010b} sets reserved bit {bit} and should be rejected",
                buf[5]
            );
        }
    }

    #[test]
    fn a_frame_round_trips() {
        let frame = Frame {
            header: FrameHeader {
                opcode: 0x03,
                request_id: 1,
            },
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
                assert_eq!(decoded.header.opcode, frame.header.opcode);
                assert_eq!(decoded.header.request_id, frame.header.request_id);
            }
            other => panic!("what we encoded should decode, got {other:?}"),
        }
    }

    #[test]
    fn encode_never_emits_a_frame_decode_rejects() {
        // `length` is derived from the body, but encode takes it as input and
        // writes it verbatim, so a caller can hand over a prefix that disagrees
        // with the bytes that follow.
        let frame = Frame {
            header: FrameHeader {
                opcode: 0x03,
                request_id: 1,
            },
        };

        let mut out = [0u8; 64];
        if let Some(written) = encode(&frame, &mut out) {
            assert!(
                matches!(decode(&out[..written]), Decoded::Success { .. }),
                "encode emitted {written} bytes that its own decode will not accept"
            );
        }
    }

    #[test]
    fn encode_refuses_a_buffer_that_is_too_small() {
        let frame = Frame {
            header: FrameHeader {
                opcode: 0x03,
                request_id: 1,
            },
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
        assert!(
            matches!(decode(&buf), Decoded::Incomplete),
            "a body of exactly the maximum is legal, so decode should ask for more bytes"
        );
    }
}
