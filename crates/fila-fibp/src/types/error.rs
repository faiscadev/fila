use std::collections::HashMap;

use bytes::Bytes;

use crate::error::FrameError;
use crate::error_code::ErrorCode;
use crate::frame::{PayloadReader, PayloadWriter, RawFrame};
use crate::opcode::Opcode;

use super::ProtocolMessage;

// ---------------------------------------------------------------------------
// Error frame
// ---------------------------------------------------------------------------

#[derive(Debug, Clone)]
pub struct ErrorFrame {
    pub error_code: ErrorCode,
    pub message: String,
    pub metadata: HashMap<String, String>,
}

impl ProtocolMessage for ErrorFrame {
    fn encode(&self, request_id: u32) -> RawFrame {
        let mut w = PayloadWriter::new();
        w.put_u8(self.error_code as u8);
        w.put_string(&self.message);
        w.put_string_map(&self.metadata);
        RawFrame {
            opcode: Opcode::Error as u8,
            flags: 0,
            request_id,
            payload: w.finish(),
        }
    }

    fn decode(payload: Bytes) -> Result<Self, FrameError> {
        let mut r = PayloadReader::new(payload);
        let error_code = ErrorCode::from_u8(r.read_u8()?);
        let message = r.read_string()?;
        let metadata = r.read_string_map()?;
        Ok(Self {
            error_code,
            message,
            metadata,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn error_frame_round_trip() {
        let mut metadata = HashMap::new();
        metadata.insert("leader_addr".to_string(), "127.0.0.1:5555".to_string());

        let err = ErrorFrame {
            error_code: ErrorCode::NotLeader,
            message: "not the leader".to_string(),
            metadata,
        };
        let frame = err.encode(99);
        let decoded = ErrorFrame::decode(frame.payload).unwrap();
        assert_eq!(decoded.error_code, ErrorCode::NotLeader);
        assert_eq!(decoded.message, "not the leader");
        assert_eq!(
            decoded.metadata.get("leader_addr").unwrap(),
            "127.0.0.1:5555"
        );
    }
}
