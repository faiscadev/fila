mod client;
mod error;

pub use client::{AccumulatorMode, ConnectOptions, ConsumeMessage, EnqueueMessage, FilaClient};
pub use error::{AckError, ConnectError, ConsumeError, EnqueueError, NackError, StatusError};

/// Re-export the proto types for advanced usage.
pub use fila_proto as proto;
