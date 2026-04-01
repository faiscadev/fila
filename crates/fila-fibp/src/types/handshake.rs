use bytes::Bytes;

use crate::error::FrameError;
use crate::frame::{PayloadReader, PayloadWriter, RawFrame};
use crate::opcode::Opcode;

use super::ProtocolMessage;

// ---------------------------------------------------------------------------
// Control frames
// ---------------------------------------------------------------------------

#[derive(Debug, Clone)]
pub struct Handshake {
    pub protocol_version: u16,
    pub api_key: Option<String>,
}

#[derive(Debug, Clone)]
pub struct HandshakeOk {
    pub negotiated_version: u16,
    pub node_id: u64,
    pub max_frame_size: u32,
}

impl ProtocolMessage for Handshake {
    fn encode(&self, request_id: u32) -> RawFrame {
        let mut w = PayloadWriter::new();
        w.put_u16(self.protocol_version);
        w.put_optional_string(&self.api_key);
        RawFrame {
            opcode: Opcode::Handshake as u8,
            flags: 0,
            request_id,
            payload: w.finish(),
        }
    }

    fn decode(payload: Bytes) -> Result<Self, FrameError> {
        let mut r = PayloadReader::new(payload);
        let protocol_version = r.read_u16()?;
        let api_key = r.read_optional_string()?;
        Ok(Self {
            protocol_version,
            api_key,
        })
    }
}

impl ProtocolMessage for HandshakeOk {
    fn encode(&self, request_id: u32) -> RawFrame {
        let mut w = PayloadWriter::new();
        w.put_u16(self.negotiated_version);
        w.put_u64(self.node_id);
        w.put_u32(self.max_frame_size);
        RawFrame {
            opcode: Opcode::HandshakeOk as u8,
            flags: 0,
            request_id,
            payload: w.finish(),
        }
    }

    fn decode(payload: Bytes) -> Result<Self, FrameError> {
        let mut r = PayloadReader::new(payload);
        let negotiated_version = r.read_u16()?;
        let node_id = r.read_u64()?;
        let max_frame_size = r.read_u32()?;
        Ok(Self {
            negotiated_version,
            node_id,
            max_frame_size,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn handshake_round_trip() {
        let hs = Handshake {
            protocol_version: 1,
            api_key: Some("secret-key".to_string()),
        };
        let frame = hs.encode(0);
        let decoded = Handshake::decode(frame.payload).unwrap();
        assert_eq!(decoded.protocol_version, 1);
        assert_eq!(decoded.api_key, Some("secret-key".to_string()));
    }

    #[test]
    fn handshake_ok_round_trip() {
        let ok = HandshakeOk {
            negotiated_version: 1,
            node_id: 42,
            max_frame_size: 0,
        };
        let frame = ok.encode(0);
        let decoded = HandshakeOk::decode(frame.payload).unwrap();
        assert_eq!(decoded.negotiated_version, 1);
        assert_eq!(decoded.node_id, 42);
        assert_eq!(decoded.max_frame_size, 0);
    }
}
