mod client;
mod error;
pub mod fibp_transport;

pub use client::{AccumulatorMode, ConnectOptions, ConsumeMessage, EnqueueMessage, FilaClient};
pub use error::{AckError, ConnectError, ConsumeError, EnqueueError, NackError, StatusError};
pub use fibp_transport::{ConsumeStream, FibpTransport};

/// Re-export the wire types for advanced admin usage (CLI, e2e tests).
pub use fila_fibp as wire;
