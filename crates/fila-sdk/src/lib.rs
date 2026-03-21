mod client;
mod error;

pub use client::{ConnectOptions, ConsumeMessage, FilaClient, ServerInfo};
pub use error::{
    AckError, ConnectError, ConsumeError, EnqueueError, NackError, ServerInfoError, StatusError,
};

/// Re-export the proto types for advanced usage.
pub use fila_proto as proto;
