//! FIBP -- Fila Binary Protocol.
//!
//! A binary TCP transport designed for high-throughput message
//! operations. FIBP is Fila's sole transport protocol.
//!
//! The FIBP module dispatches data operations (enqueue, consume, ack, nack)
//! to the scheduler via `Arc<Broker>`, injected through `FibpListener::start()`.
//!
//! The wire format (codec, frame framing, op codes, payload encoding) lives in
//! the shared `fila_fibp` crate so both server and client use the same code.

mod connection;
mod dispatch;
mod error;
mod listener;

// Re-export everything from fila_fibp that the rest of fila-core needs,
// preserving the same public API that callers used via `super::codec::*`
// and `super::wire::*`.
pub use fila_fibp::codec::{
    FibpCodec, Frame, FLAG_STREAM, OP_ACK, OP_AUTH, OP_AUTH_CREATE_KEY, OP_AUTH_GET_ACL,
    OP_AUTH_LIST_KEYS, OP_AUTH_REVOKE_KEY, OP_AUTH_SET_ACL, OP_CONFIG_GET, OP_CONFIG_LIST,
    OP_CONFIG_SET, OP_CONSUME, OP_CREATE_QUEUE, OP_DELETE_QUEUE, OP_ENQUEUE, OP_ERROR, OP_FLOW,
    OP_GOAWAY, OP_HEARTBEAT, OP_LIST_QUEUES, OP_NACK, OP_QUEUE_STATS, OP_REDRIVE,
};
pub use fila_fibp::wire;
pub use fila_fibp::FRAME_HEADER_LEN;
pub use fila_fibp::MAGIC;

pub use connection::FibpConnection;
pub use error::FibpError;
pub use listener::FibpListener;
