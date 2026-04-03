use bytes::Bytes;

use crate::error::FrameError;
use crate::frame::{PayloadReader, PayloadWriter, RawFrame};
use crate::opcode::Opcode;

use super::ProtocolMessage;

// ---------------------------------------------------------------------------
// Cluster frames
// ---------------------------------------------------------------------------

/// Raft RPC request (AppendEntries, Vote, InstallSnapshot).
/// The `data` field carries protobuf-serialized openraft request bytes.
#[derive(Debug, Clone)]
pub struct ClusterRaftRequest {
    pub group_id: String,
    pub data: Vec<u8>,
}

/// Raft RPC response.
/// The `data` field carries protobuf-serialized openraft response bytes.
#[derive(Debug, Clone)]
pub struct ClusterRaftResponse {
    pub data: Vec<u8>,
}

/// Client write forwarding request (leader forwarding).
#[derive(Debug, Clone)]
pub struct ClusterClientWriteRequest {
    pub group_id: String,
    pub data: Vec<u8>,
}

/// Client write forwarding response.
#[derive(Debug, Clone)]
pub struct ClusterClientWriteResponse {
    pub data: Vec<u8>,
}

/// Add a node to the cluster.
#[derive(Debug, Clone)]
pub struct ClusterAddNodeRequest {
    pub node_id: u64,
    pub addr: String,
    pub client_addr: String,
}

/// Result of adding a node.
#[derive(Debug, Clone)]
pub struct ClusterAddNodeResponse {
    pub success: bool,
    pub error: String,
    pub leader_addr: String,
}

/// Remove a node from the cluster.
#[derive(Debug, Clone)]
pub struct ClusterRemoveNodeRequest {
    pub node_id: u64,
}

/// Result of removing a node.
#[derive(Debug, Clone)]
pub struct ClusterRemoveNodeResponse {
    pub success: bool,
    pub error: String,
    pub leader_addr: String,
}

/// Request for node info (empty payload — just the opcode).
#[derive(Debug, Clone)]
pub struct ClusterGetNodeInfoRequest;

/// Response with node info.
#[derive(Debug, Clone)]
pub struct ClusterGetNodeInfoResponse {
    pub node_id: u64,
    pub client_addr: String,
}

// ---------------------------------------------------------------------------
// Cluster encode/decode implementations
// ---------------------------------------------------------------------------

// ClusterRaftRequest and ClusterRaftResponse use inherent methods instead of
// the ProtocolMessage trait because they are used with multiple opcodes
// (AppendEntries, Vote, InstallSnapshot).

impl ClusterRaftRequest {
    pub fn encode(&self, request_id: u32, opcode: Opcode) -> RawFrame {
        let mut w = PayloadWriter::with_capacity(self.data.len() + 64);
        w.put_string(&self.group_id);
        w.put_bytes(&self.data);
        RawFrame {
            opcode: opcode as u8,
            flags: 0,
            request_id,
            payload: w.finish(),
        }
    }

    pub fn decode(payload: Bytes) -> Result<Self, FrameError> {
        let mut r = PayloadReader::new(payload);
        let group_id = r.read_string()?;
        let data = r.read_bytes()?;
        Ok(Self { group_id, data })
    }
}

impl ClusterRaftResponse {
    pub fn encode(&self, request_id: u32, opcode: Opcode) -> RawFrame {
        let mut w = PayloadWriter::with_capacity(self.data.len() + 8);
        w.put_bytes(&self.data);
        RawFrame {
            opcode: opcode as u8,
            flags: 0,
            request_id,
            payload: w.finish(),
        }
    }

    pub fn decode(payload: Bytes) -> Result<Self, FrameError> {
        let mut r = PayloadReader::new(payload);
        let data = r.read_bytes()?;
        Ok(Self { data })
    }
}

impl ProtocolMessage for ClusterClientWriteRequest {
    fn encode(&self, request_id: u32) -> RawFrame {
        let mut w = PayloadWriter::with_capacity(self.data.len() + 64);
        w.put_string(&self.group_id);
        w.put_bytes(&self.data);
        RawFrame {
            opcode: Opcode::ClusterClientWrite as u8,
            flags: 0,
            request_id,
            payload: w.finish(),
        }
    }

    fn decode(payload: Bytes) -> Result<Self, FrameError> {
        let mut r = PayloadReader::new(payload);
        let group_id = r.read_string()?;
        let data = r.read_bytes()?;
        Ok(Self { group_id, data })
    }
}

impl ProtocolMessage for ClusterClientWriteResponse {
    fn encode(&self, request_id: u32) -> RawFrame {
        let mut w = PayloadWriter::with_capacity(self.data.len() + 8);
        w.put_bytes(&self.data);
        RawFrame {
            opcode: Opcode::ClusterClientWriteResult as u8,
            flags: 0,
            request_id,
            payload: w.finish(),
        }
    }

    fn decode(payload: Bytes) -> Result<Self, FrameError> {
        let mut r = PayloadReader::new(payload);
        let data = r.read_bytes()?;
        Ok(Self { data })
    }
}

impl ProtocolMessage for ClusterAddNodeRequest {
    fn encode(&self, request_id: u32) -> RawFrame {
        let mut w = PayloadWriter::new();
        w.put_u64(self.node_id);
        w.put_string(&self.addr);
        w.put_string(&self.client_addr);
        RawFrame {
            opcode: Opcode::ClusterAddNode as u8,
            flags: 0,
            request_id,
            payload: w.finish(),
        }
    }

    fn decode(payload: Bytes) -> Result<Self, FrameError> {
        let mut r = PayloadReader::new(payload);
        let node_id = r.read_u64()?;
        let addr = r.read_string()?;
        let client_addr = r.read_string()?;
        Ok(Self {
            node_id,
            addr,
            client_addr,
        })
    }
}

impl ProtocolMessage for ClusterAddNodeResponse {
    fn encode(&self, request_id: u32) -> RawFrame {
        let mut w = PayloadWriter::new();
        w.put_bool(self.success);
        w.put_string(&self.error);
        w.put_string(&self.leader_addr);
        RawFrame {
            opcode: Opcode::ClusterAddNodeResult as u8,
            flags: 0,
            request_id,
            payload: w.finish(),
        }
    }

    fn decode(payload: Bytes) -> Result<Self, FrameError> {
        let mut r = PayloadReader::new(payload);
        let success = r.read_bool()?;
        let error = r.read_string()?;
        let leader_addr = r.read_string()?;
        Ok(Self {
            success,
            error,
            leader_addr,
        })
    }
}

impl ProtocolMessage for ClusterRemoveNodeRequest {
    fn encode(&self, request_id: u32) -> RawFrame {
        let mut w = PayloadWriter::new();
        w.put_u64(self.node_id);
        RawFrame {
            opcode: Opcode::ClusterRemoveNode as u8,
            flags: 0,
            request_id,
            payload: w.finish(),
        }
    }

    fn decode(payload: Bytes) -> Result<Self, FrameError> {
        let mut r = PayloadReader::new(payload);
        let node_id = r.read_u64()?;
        Ok(Self { node_id })
    }
}

impl ProtocolMessage for ClusterRemoveNodeResponse {
    fn encode(&self, request_id: u32) -> RawFrame {
        let mut w = PayloadWriter::new();
        w.put_bool(self.success);
        w.put_string(&self.error);
        w.put_string(&self.leader_addr);
        RawFrame {
            opcode: Opcode::ClusterRemoveNodeResult as u8,
            flags: 0,
            request_id,
            payload: w.finish(),
        }
    }

    fn decode(payload: Bytes) -> Result<Self, FrameError> {
        let mut r = PayloadReader::new(payload);
        let success = r.read_bool()?;
        let error = r.read_string()?;
        let leader_addr = r.read_string()?;
        Ok(Self {
            success,
            error,
            leader_addr,
        })
    }
}

impl ProtocolMessage for ClusterGetNodeInfoRequest {
    fn encode(&self, request_id: u32) -> RawFrame {
        RawFrame {
            opcode: Opcode::ClusterGetNodeInfo as u8,
            flags: 0,
            request_id,
            payload: Bytes::new(),
        }
    }

    fn decode(_payload: Bytes) -> Result<Self, FrameError> {
        Ok(Self)
    }
}

impl ProtocolMessage for ClusterGetNodeInfoResponse {
    fn encode(&self, request_id: u32) -> RawFrame {
        let mut w = PayloadWriter::new();
        w.put_u64(self.node_id);
        w.put_string(&self.client_addr);
        RawFrame {
            opcode: Opcode::ClusterGetNodeInfoResult as u8,
            flags: 0,
            request_id,
            payload: w.finish(),
        }
    }

    fn decode(payload: Bytes) -> Result<Self, FrameError> {
        let mut r = PayloadReader::new(payload);
        let node_id = r.read_u64()?;
        let client_addr = r.read_string()?;
        Ok(Self {
            node_id,
            client_addr,
        })
    }
}
