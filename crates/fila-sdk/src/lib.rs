mod client;
mod error;
pub(crate) mod fibp_codec;
pub mod fibp_transport;

pub use client::{AccumulatorMode, ConnectOptions, ConsumeMessage, EnqueueMessage, FilaClient};
pub use error::{AckError, ConnectError, ConsumeError, EnqueueError, NackError, StatusError};
pub use fibp_transport::FibpTransport;

/// Re-export the proto types for advanced usage.
pub use fila_proto as proto;
