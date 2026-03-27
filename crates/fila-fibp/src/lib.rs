//! FIBP -- Fila Binary Protocol wire format.
//!
//! Shared codec and wire encoding used by both the server (`fila-core`) and
//! the client SDK (`fila-sdk`). All FIBP frame framing, op codes, and
//! payload encode/decode functions live here so there is exactly one
//! definition of the wire format.

pub mod codec;
pub mod error;
pub mod wire;

pub use codec::{
    FibpCodec, Frame, FLAG_STREAM, OP_ACK, OP_AUTH, OP_AUTH_CREATE_KEY, OP_AUTH_GET_ACL,
    OP_AUTH_LIST_KEYS, OP_AUTH_REVOKE_KEY, OP_AUTH_SET_ACL, OP_CONFIG_GET, OP_CONFIG_LIST,
    OP_CONFIG_SET, OP_CONSUME, OP_CREATE_QUEUE, OP_DELETE_QUEUE, OP_ENQUEUE, OP_ERROR, OP_FLOW,
    OP_GOAWAY, OP_HEARTBEAT, OP_LIST_QUEUES, OP_NACK, OP_QUEUE_STATS, OP_REDRIVE,
};
pub use error::FibpCodecError;

/// Protocol magic bytes: `FIBP` followed by major version 1, minor version 0.
pub const MAGIC: &[u8; 6] = b"FIBP\x01\x00";

/// Length of the frame header following the 4-byte length prefix:
/// flags (1) + op (1) + correlation_id (4) = 6 bytes.
pub const FRAME_HEADER_LEN: usize = 6;
