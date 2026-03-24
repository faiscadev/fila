mod client;
mod error;

pub use client::{
    BatchConfig, BatchEnqueueResult, ConnectOptions, ConsumeMessage, EnqueueMessage, FilaClient,
};
pub use error::{
    AckError, BatchEnqueueError, ConnectError, ConsumeError, EnqueueError, NackError, StatusError,
};

/// Re-export the proto types for advanced usage.
pub use fila_proto as proto;
