mod client;
mod error;

pub use client::{ConnectOptions, ConsumeMessage, FilaClient};
pub use error::{AckError, ConnectError, ConsumeError, EnqueueError, NackError, StatusError};

/// Re-export the proto types for advanced usage.
pub use fila_proto as proto;
