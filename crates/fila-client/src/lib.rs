mod client;
mod error;

pub use client::{ConnectOptions, FilaClient, LeaseMessage};
pub use error::ClientError;

/// Re-export the proto types for advanced usage.
pub use fila_proto as proto;
