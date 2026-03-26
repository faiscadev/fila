//! FIBP — Fila Binary Protocol.
//!
//! A custom binary TCP transport designed for high-throughput message
//! operations. Runs alongside gRPC when the `[fibp]` config section is present.
//!
//! The FIBP module dispatches data operations (enqueue, consume, ack, nack)
//! to the scheduler via `Arc<Broker>`, injected through `FibpListener::start()`.

mod codec;
mod connection;
mod dispatch;
mod error;
mod listener;
pub mod wire;

pub use codec::{
    FibpCodec, Frame, FLAG_STREAM, OP_ACK, OP_AUTH, OP_AUTH_CREATE_KEY, OP_AUTH_GET_ACL,
    OP_AUTH_LIST_KEYS, OP_AUTH_REVOKE_KEY, OP_AUTH_SET_ACL, OP_CONFIG_GET, OP_CONFIG_LIST,
    OP_CONFIG_SET, OP_CONSUME, OP_CREATE_QUEUE, OP_DELETE_QUEUE, OP_ENQUEUE, OP_ERROR, OP_FLOW,
    OP_GOAWAY, OP_HEARTBEAT, OP_LIST_QUEUES, OP_NACK, OP_PAUSE_QUEUE, OP_QUEUE_STATS, OP_REDRIVE,
    OP_RESUME_QUEUE,
};
pub use connection::FibpConnection;
pub use error::FibpError;
pub use listener::FibpListener;

/// Protocol magic bytes: `FIBP` followed by major version 1, minor version 0.
pub const MAGIC: &[u8; 6] = b"FIBP\x01\x00";

/// Length of the frame header following the 4-byte length prefix:
/// flags (1) + op (1) + correlation_id (4) = 6 bytes.
pub const FRAME_HEADER_LEN: usize = 6;
