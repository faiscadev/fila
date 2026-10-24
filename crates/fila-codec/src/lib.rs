#![no_std]

extern crate alloc;

#[derive(Debug)]
pub struct FrameHeader {
    pub opcode: u8,
    pub flags: u8,
    pub request_id: u32,
}

#[derive(Debug)]
pub struct FrameBody {
    pub header: FrameHeader,
}

#[derive(Debug)]
pub struct Frame {
    pub length: u32,
    pub body: FrameBody,
}

#[derive(Debug)]
pub enum FrameError {
    ContinuationUnsupported,
    LengthBelowHeader { length: u32 },
    LengthAboveMaximum { length: u32 },
    ReservedFlagsSet { flags: u8 },
}

#[derive(Debug)]
pub enum Decoded {
    Frame { frame: Frame, consumed: usize },
    Incomplete,
    Invalid(FrameError),
}

fn extract_frame_header(buf: &[u8]) -> FrameHeader {
    let buf_body = &buf[4..];
    let opcode = u8::from_be_bytes([buf_body[0]]);
    let flags = u8::from_be_bytes([buf_body[1]]);

    let request_id = u32::from_be_bytes([buf_body[2], buf_body[3], buf_body[4], buf_body[5]]);
    FrameHeader {
        opcode,
        flags,
        request_id,
    }
}

fn extract_frame_body(buf: &[u8]) -> FrameBody {
    let header = extract_frame_header(buf);
    FrameBody { header }
}

fn extract_frame(buf: &[u8], body_len: u32) -> Frame {
    Frame {
        length: body_len,
        body: extract_frame_body(buf),
    }
}

const MAX_FRAME_BODY_LEN: u32 = 1024 * 1024 * 16; // 16 MiB

fn validate_flags(flags: u8) -> Option<FrameError> {
    if flags == 0b0000_0001 {
        // continuation is not implemented yet
        Some(FrameError::ContinuationUnsupported)
    } else if flags != 0b0000_0000 {
        Some(FrameError::ReservedFlagsSet { flags })
    } else {
        None
    }
}

fn validate_frame(frame: &Frame) -> Option<FrameError> {
    validate_flags(frame.body.header.flags)
}

pub fn decode(buf: &[u8]) -> Decoded {
    if buf.len() < 4 {
        return Decoded::Incomplete;
    }

    let body_len = u32::from_be_bytes([buf[0], buf[1], buf[2], buf[3]]);

    if body_len > MAX_FRAME_BODY_LEN {
        return Decoded::Invalid(FrameError::LengthAboveMaximum { length: body_len });
    }

    if body_len < 6 {
        return Decoded::Invalid(FrameError::LengthBelowHeader { length: body_len });
    }

    if buf.len() < (body_len as usize) + 4 {
        return Decoded::Incomplete;
    }

    let frame = extract_frame(buf, body_len);
    if let Some(err) = validate_frame(&frame) {
        Decoded::Invalid(err)
    } else {
        Decoded::Frame {
            frame,
            consumed: (body_len + 4) as usize,
        }
    }
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
            Decoded::Frame { frame, consumed } => {
                assert_eq!(frame.length, 6);
                assert_eq!(frame.body.header.opcode, 0x03);
                assert_eq!(frame.body.header.flags, 0);
                assert_eq!(frame.body.header.request_id, 1);
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
            Decoded::Frame { frame, .. } => assert_eq!(frame.body.header.request_id, 0x0A0B0C0D),
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
            Decoded::Invalid(FrameError::ContinuationUnsupported)
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
        let Decoded::Frame { consumed, .. } = decode(&stream) else {
            panic!("expected the first frame");
        };
        assert!(
            matches!(decode(&stream[consumed..]), Decoded::Frame { .. }),
            "draining `consumed` bytes should leave the second frame at the front"
        );
    }

    #[test]
    fn trailing_bytes_do_not_affect_the_first_frame() {
        let mut stream = PING.to_vec();
        stream.extend_from_slice(&[0xFF; 3]); // half of something else
        match decode(&stream) {
            Decoded::Frame { consumed, .. } => assert_eq!(consumed, 10),
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
                    Decoded::Invalid(FrameError::ReservedFlagsSet { flags }) if flags == buf[5]
                ),
                "flags = {:#010b} sets reserved bit {bit} and should be rejected",
                buf[5]
            );
        }
    }
}
