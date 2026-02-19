use std::collections::HashMap;
use std::time::Duration;

use fila_proto::fila_service_client::FilaServiceClient;
use fila_proto::{AckRequest, EnqueueRequest, LeaseRequest, NackRequest};
use tokio_stream::{Stream, StreamExt};
use tonic::transport::Channel;

use crate::error::{
    ack_status_error, enqueue_status_error, lease_status_error, nack_status_error, status_error,
    AckError, ConnectError, EnqueueError, LeaseError, NackError, StatusError,
};

/// A leased message received from the broker.
#[derive(Debug, Clone)]
pub struct LeaseMessage {
    pub id: String,
    pub headers: HashMap<String, String>,
    pub payload: Vec<u8>,
    pub fairness_key: String,
    pub attempt_count: u32,
    pub queue: String,
}

/// Options for connecting to a Fila broker.
#[derive(Debug, Clone)]
pub struct ConnectOptions {
    pub addr: String,
    pub timeout: Option<Duration>,
}

impl ConnectOptions {
    pub fn new(addr: impl Into<String>) -> Self {
        Self {
            addr: addr.into(),
            timeout: None,
        }
    }

    pub fn with_timeout(mut self, timeout: Duration) -> Self {
        self.timeout = Some(timeout);
        self
    }
}

/// Idiomatic Rust client for the Fila message broker.
///
/// Wraps the hot-path gRPC operations: enqueue, lease, ack, nack.
/// The client is `Clone`, `Send`, and `Sync` — it can be shared across tasks.
#[derive(Debug, Clone)]
pub struct FilaClient {
    inner: FilaServiceClient<Channel>,
}

impl FilaClient {
    /// Connect to a Fila broker at the given address.
    ///
    /// The address should include the scheme, e.g. `http://localhost:5555`.
    pub async fn connect(addr: impl Into<String>) -> Result<Self, ConnectError> {
        let inner = FilaServiceClient::connect(addr.into()).await?;
        Ok(Self { inner })
    }

    /// Connect to a Fila broker with custom options.
    pub async fn connect_with_options(options: ConnectOptions) -> Result<Self, ConnectError> {
        let mut endpoint = Channel::from_shared(options.addr)
            .map_err(|e| ConnectError::InvalidArgument(e.to_string()))?;

        if let Some(timeout) = options.timeout {
            endpoint = endpoint.timeout(timeout);
        }

        let channel = endpoint.connect().await?;
        let inner = FilaServiceClient::new(channel);
        Ok(Self { inner })
    }

    /// Enqueue a message to a queue.
    ///
    /// Returns the broker-assigned message ID (UUIDv7).
    pub async fn enqueue(
        &self,
        queue: &str,
        headers: HashMap<String, String>,
        payload: impl Into<Vec<u8>>,
    ) -> Result<String, EnqueueError> {
        let response = self
            .inner
            .clone()
            .enqueue(EnqueueRequest {
                queue: queue.to_string(),
                headers,
                payload: payload.into(),
            })
            .await
            .map_err(enqueue_status_error)?;

        Ok(response.into_inner().message_id)
    }

    /// Open a streaming lease on a queue.
    ///
    /// Returns a stream of leased messages. The broker pushes messages as they
    /// become available. The stream remains open until the client drops it or
    /// the broker disconnects.
    ///
    /// The outer `Result` fails with [`LeaseError`] if the queue does not exist
    /// or the stream cannot be established. The inner `Result` on each stream
    /// item uses [`StatusError`] — only transport/server errors, never
    /// queue-not-found.
    pub async fn lease(
        &self,
        queue: &str,
    ) -> Result<impl Stream<Item = Result<LeaseMessage, StatusError>>, LeaseError> {
        let response = self
            .inner
            .clone()
            .lease(LeaseRequest {
                queue: queue.to_string(),
            })
            .await
            .map_err(lease_status_error)?;

        let stream = response.into_inner().filter_map(|result| match result {
            Ok(lease_response) => {
                // The server may send LeaseResponse frames with `message: None` as
                // keepalive signals on the streaming connection. These are expected
                // and silently skipped — they are not protocol errors.
                let msg = lease_response.message?;
                let metadata = msg.metadata.unwrap_or_default();
                Some(Ok(LeaseMessage {
                    id: msg.id,
                    headers: msg.headers,
                    payload: msg.payload,
                    fairness_key: metadata.fairness_key,
                    attempt_count: metadata.attempt_count,
                    queue: metadata.queue_id,
                }))
            }
            Err(status) => Some(Err(status_error(status))),
        });

        Ok(stream)
    }

    /// Acknowledge a successfully processed message.
    ///
    /// The message is permanently removed from the queue.
    pub async fn ack(&self, queue: &str, message_id: &str) -> Result<(), AckError> {
        self.inner
            .clone()
            .ack(AckRequest {
                queue: queue.to_string(),
                message_id: message_id.to_string(),
            })
            .await
            .map_err(ack_status_error)?;

        Ok(())
    }

    /// Negatively acknowledge a message that failed processing.
    ///
    /// The message is requeued for retry or routed to the dead-letter queue
    /// based on the queue's on_failure Lua hook configuration.
    pub async fn nack(&self, queue: &str, message_id: &str, error: &str) -> Result<(), NackError> {
        self.inner
            .clone()
            .nack(NackRequest {
                queue: queue.to_string(),
                message_id: message_id.to_string(),
                error: error.to_string(),
            })
            .await
            .map_err(nack_status_error)?;

        Ok(())
    }
}
