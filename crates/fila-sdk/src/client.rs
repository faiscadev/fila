use std::collections::HashMap;
use std::time::Duration;

use fila_proto::fila_service_client::FilaServiceClient;
use fila_proto::{AckRequest, BatchEnqueueRequest, ConsumeRequest, EnqueueRequest, NackRequest};
use tokio_stream::{Stream, StreamExt};
use tonic::transport::{Certificate, Channel, ClientTlsConfig, Identity};

use crate::error::{
    ack_status_error, batch_enqueue_status_error, consume_status_error, enqueue_status_error,
    nack_status_error, status_error, AckError, BatchEnqueueError, ConnectError, ConsumeError,
    EnqueueError, NackError, StatusError,
};

/// A consumed message received from the broker.
#[derive(Debug, Clone)]
pub struct ConsumeMessage {
    pub id: String,
    pub headers: HashMap<String, String>,
    pub payload: Vec<u8>,
    pub fairness_key: String,
    pub attempt_count: u32,
    pub queue: String,
}

/// Configuration for client-side batching.
///
/// When `linger_ms` is `Some`, the SDK accumulates messages in an internal buffer
/// and flushes when either `linger_ms` elapses or `batch_size` is reached.
/// When `linger_ms` is `None` (default), batching is disabled and each `enqueue()`
/// call sends a single message immediately.
#[derive(Debug, Clone)]
pub struct BatchConfig {
    /// Time threshold in milliseconds before a partial batch is flushed.
    /// `None` means batching is disabled (default).
    pub linger_ms: Option<u64>,
    /// Maximum number of messages per batch. Default: 100.
    pub batch_size: usize,
}

impl Default for BatchConfig {
    fn default() -> Self {
        Self {
            linger_ms: None,
            batch_size: 100,
        }
    }
}

/// A single message specification for use with [`FilaClient::batch_enqueue`].
#[derive(Debug, Clone)]
pub struct EnqueueMessage {
    pub queue: String,
    pub headers: HashMap<String, String>,
    pub payload: Vec<u8>,
}

/// The result of a single message within a batch enqueue call.
///
/// Each message in a batch is independently validated and processed.
/// A failed message does not affect the others.
#[derive(Debug, Clone)]
pub enum BatchEnqueueResult {
    /// The message was successfully enqueued. Contains the broker-assigned message ID.
    Success(String),
    /// The message failed to enqueue. Contains the error description.
    Error(String),
}

/// Options for connecting to a Fila broker.
#[derive(Debug, Clone, Default)]
pub struct ConnectOptions {
    pub addr: String,
    pub timeout: Option<Duration>,
    /// Enable TLS using the operating system's trust store.
    /// When true without `tls_ca_cert_pem`, the system root certificates are used
    /// to verify the server's certificate (e.g., Let's Encrypt, corporate CAs).
    pub tls: bool,
    /// PEM-encoded CA certificate for verifying the server's certificate.
    /// When set, implies `tls: true` and overrides the system trust store.
    pub tls_ca_cert_pem: Option<Vec<u8>>,
    /// PEM-encoded client certificate for mTLS authentication.
    pub tls_client_cert_pem: Option<Vec<u8>>,
    /// PEM-encoded client private key for mTLS authentication.
    pub tls_client_key_pem: Option<Vec<u8>>,
    /// API key for authenticating with the broker.
    /// When set, every RPC includes `authorization: Bearer <key>` metadata.
    pub api_key: Option<String>,
}

impl ConnectOptions {
    pub fn new(addr: impl Into<String>) -> Self {
        Self {
            addr: addr.into(),
            ..Default::default()
        }
    }

    pub fn with_timeout(mut self, timeout: Duration) -> Self {
        self.timeout = Some(timeout);
        self
    }

    /// Enable TLS using the operating system's trust store.
    ///
    /// Use this when the Fila server has a certificate signed by a public CA
    /// (e.g., Let's Encrypt) or a corporate CA already in the system trust store.
    /// No CA certificate PEM is needed.
    pub fn with_tls(mut self) -> Self {
        self.tls = true;
        self
    }

    /// Set the CA certificate for verifying the server's TLS certificate.
    ///
    /// Implies TLS enabled. Use this for self-signed certificates.
    pub fn with_tls_ca_cert(mut self, ca_cert_pem: Vec<u8>) -> Self {
        self.tls_ca_cert_pem = Some(ca_cert_pem);
        self
    }

    /// Set the client certificate and key for mTLS authentication.
    pub fn with_tls_identity(mut self, cert_pem: Vec<u8>, key_pem: Vec<u8>) -> Self {
        self.tls_client_cert_pem = Some(cert_pem);
        self.tls_client_key_pem = Some(key_pem);
        self
    }

    /// Set an API key for authenticating with the broker.
    ///
    /// When set, every RPC attaches `authorization: Bearer <key>` metadata.
    pub fn with_api_key(mut self, api_key: impl Into<String>) -> Self {
        self.api_key = Some(api_key.into());
        self
    }
}

/// Idiomatic Rust client for the Fila message broker.
///
/// Wraps the hot-path gRPC operations: enqueue, consume, ack, nack.
/// The client is `Clone`, `Send`, and `Sync` — it can be shared across tasks.
#[derive(Debug, Clone)]
pub struct FilaClient {
    inner: FilaServiceClient<Channel>,
    /// API key sent as `authorization: Bearer <key>` on every request.
    api_key: Option<String>,
    /// Stored connection options for transparent leader redirect reconnection.
    /// When the consume call is redirected to a leader node, the SDK reuses
    /// these options (TLS, timeout, auth) for the new connection.
    connect_options: Option<ConnectOptions>,
}

impl FilaClient {
    /// Connect to a Fila broker at the given address.
    ///
    /// The address should include the scheme, e.g. `http://localhost:5555`.
    pub async fn connect(addr: impl Into<String>) -> Result<Self, ConnectError> {
        let addr = addr.into();
        let inner = FilaServiceClient::connect(addr.clone()).await?;
        Ok(Self {
            inner,
            api_key: None,
            connect_options: Some(ConnectOptions::new(addr)),
        })
    }

    /// Connect to a Fila broker with custom options.
    ///
    /// When TLS fields are set in `options`, the connection uses TLS/mTLS.
    /// The address should use `https://` when TLS is enabled.
    pub async fn connect_with_options(options: ConnectOptions) -> Result<Self, ConnectError> {
        // Clone options for storage before consuming fields for TLS setup.
        let stored_options = options.clone();
        let mut endpoint = Channel::from_shared(options.addr)
            .map_err(|e| ConnectError::InvalidArgument(e.to_string()))?;

        if let Some(timeout) = options.timeout {
            endpoint = endpoint.timeout(timeout);
        }

        // Validate partial mTLS: cert and key must both be provided or both absent.
        let has_cert = options.tls_client_cert_pem.is_some();
        let has_key = options.tls_client_key_pem.is_some();
        if has_cert != has_key {
            return Err(ConnectError::InvalidArgument(
                "tls_client_cert_pem and tls_client_key_pem must both be provided for mTLS"
                    .to_string(),
            ));
        }

        // Apply TLS config when explicitly enabled, CA cert is provided, or client identity is set.
        let tls_enabled = options.tls || options.tls_ca_cert_pem.is_some() || has_cert;
        if tls_enabled {
            let mut tls = ClientTlsConfig::new();
            if let Some(ca_pem) = options.tls_ca_cert_pem {
                tls = tls.ca_certificate(Certificate::from_pem(ca_pem));
            }
            // Without ca_certificate, tonic uses the system trust store
            // (via tls-native-roots feature).
            if let (Some(cert_pem), Some(key_pem)) =
                (options.tls_client_cert_pem, options.tls_client_key_pem)
            {
                tls = tls.identity(Identity::from_pem(cert_pem, key_pem));
            }
            endpoint = endpoint
                .tls_config(tls)
                .map_err(|e| ConnectError::InvalidArgument(e.to_string()))?;
        }

        let channel = endpoint.connect().await?;
        let inner = FilaServiceClient::new(channel);
        let api_key = stored_options.api_key.clone();
        Ok(Self {
            inner,
            api_key,
            connect_options: Some(stored_options),
        })
    }

    /// Build a tonic `Request<T>` with the API key authorization header attached,
    /// if an API key was configured.
    fn request<T>(&self, body: T) -> tonic::Request<T> {
        let mut req = tonic::Request::new(body);
        if let Some(ref key) = self.api_key {
            if let Ok(val) =
                tonic::metadata::MetadataValue::try_from(format!("Bearer {key}").as_str())
            {
                req.metadata_mut().insert("authorization", val);
            }
        }
        req
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
            .enqueue(self.request(EnqueueRequest {
                queue: queue.to_string(),
                headers,
                payload: payload.into(),
            }))
            .await
            .map_err(enqueue_status_error)?;

        Ok(response.into_inner().message_id)
    }

    /// Enqueue a batch of messages in a single RPC call.
    ///
    /// Each message is independently validated and processed. A failed message
    /// does not affect the others in the batch. Returns a [`Vec<BatchEnqueueResult>`]
    /// with one result per input message, in the same order.
    ///
    /// This is more efficient than calling [`enqueue`](Self::enqueue) in a loop
    /// because it amortizes the RPC overhead across all messages in the batch.
    pub async fn batch_enqueue(
        &self,
        messages: Vec<EnqueueMessage>,
    ) -> Result<Vec<BatchEnqueueResult>, BatchEnqueueError> {
        let proto_messages = messages
            .into_iter()
            .map(|m| EnqueueRequest {
                queue: m.queue,
                headers: m.headers,
                payload: m.payload,
            })
            .collect();

        let response = self
            .inner
            .clone()
            .batch_enqueue(self.request(BatchEnqueueRequest {
                messages: proto_messages,
            }))
            .await
            .map_err(batch_enqueue_status_error)?;

        let results = response
            .into_inner()
            .results
            .into_iter()
            .map(|r| match r.result {
                Some(fila_proto::batch_enqueue_result::Result::Success(resp)) => {
                    BatchEnqueueResult::Success(resp.message_id)
                }
                Some(fila_proto::batch_enqueue_result::Result::Error(err)) => {
                    BatchEnqueueResult::Error(err)
                }
                None => BatchEnqueueResult::Error("no result from server".to_string()),
            })
            .collect();

        Ok(results)
    }

    /// Open a streaming consumer on a queue.
    ///
    /// Returns a stream of consumed messages. The broker pushes messages as they
    /// become available. The stream remains open until the client drops it or
    /// the broker disconnects.
    ///
    /// The outer `Result` fails with [`ConsumeError`] if the queue does not exist
    /// or the stream cannot be established. The inner `Result` on each stream
    /// item uses [`StatusError`] — only transport/server errors, never
    /// queue-not-found.
    pub async fn consume(
        &self,
        queue: &str,
    ) -> Result<impl Stream<Item = Result<ConsumeMessage, StatusError>>, ConsumeError> {
        let consume_req = ConsumeRequest {
            queue: queue.to_string(),
        };

        let result = self
            .inner
            .clone()
            .consume(self.request(consume_req.clone()))
            .await;

        // If the server returns UNAVAILABLE with a leader hint, transparently
        // reconnect to the hinted leader and retry once. This handles the
        // cluster case where consumers must connect to the queue's Raft leader.
        let response = match result {
            Ok(resp) => resp,
            Err(status)
                if status.code() == tonic::Code::Unavailable
                    && extract_leader_hint(&status).is_some() =>
            {
                let leader_addr = extract_leader_hint(&status).unwrap();
                // Connect to the hinted leader, reusing the same TLS/auth
                // config as the original connection.
                let leader_url =
                    if leader_addr.starts_with("http://") || leader_addr.starts_with("https://") {
                        leader_addr
                    } else {
                        // Preserve the original scheme (http vs https).
                        let scheme = self
                            .connect_options
                            .as_ref()
                            .map(|o| {
                                if o.tls || o.tls_ca_cert_pem.is_some() {
                                    "https"
                                } else {
                                    "http"
                                }
                            })
                            .unwrap_or("http");
                        format!("{scheme}://{leader_addr}")
                    };
                // Build connection with same TLS/timeout options.
                let leader_opts = match self.connect_options {
                    Some(ref opts) => ConnectOptions {
                        addr: leader_url,
                        timeout: opts.timeout,
                        tls: opts.tls,
                        tls_ca_cert_pem: opts.tls_ca_cert_pem.clone(),
                        tls_client_cert_pem: opts.tls_client_cert_pem.clone(),
                        tls_client_key_pem: opts.tls_client_key_pem.clone(),
                        api_key: opts.api_key.clone(),
                    },
                    None => ConnectOptions::new(leader_url),
                };
                match Self::connect_with_options(leader_opts).await {
                    Ok(leader_client) => leader_client
                        .inner
                        .clone()
                        .consume(leader_client.request(consume_req))
                        .await
                        .map_err(consume_status_error)?,
                    Err(_) => {
                        // Leader connection failed — return the original error.
                        return Err(consume_status_error(status));
                    }
                }
            }
            Err(status) => return Err(consume_status_error(status)),
        };

        // Bridge channel expands batched ConsumeResponse frames into individual
        // ConsumeMessage items, maintaining the same stream interface for callers.
        let (expand_tx, expand_rx) =
            tokio::sync::mpsc::channel::<Result<ConsumeMessage, StatusError>>(64);

        let mut inner_stream = response.into_inner();
        tokio::spawn(async move {
            while let Some(result) = inner_stream.next().await {
                match result {
                    Ok(consume_response) => {
                        // Prefer the batched `messages` field when non-empty.
                        if !consume_response.messages.is_empty() {
                            for msg in consume_response.messages {
                                let metadata = msg.metadata.unwrap_or_default();
                                let cm = ConsumeMessage {
                                    id: msg.id,
                                    headers: msg.headers,
                                    payload: msg.payload,
                                    fairness_key: metadata.fairness_key,
                                    attempt_count: metadata.attempt_count,
                                    queue: metadata.queue_id,
                                };
                                if expand_tx.send(Ok(cm)).await.is_err() {
                                    return; // Consumer dropped the stream
                                }
                            }
                        } else if let Some(msg) = consume_response.message {
                            // Single message (backward compatible with older servers).
                            let metadata = msg.metadata.unwrap_or_default();
                            let cm = ConsumeMessage {
                                id: msg.id,
                                headers: msg.headers,
                                payload: msg.payload,
                                fairness_key: metadata.fairness_key,
                                attempt_count: metadata.attempt_count,
                                queue: metadata.queue_id,
                            };
                            if expand_tx.send(Ok(cm)).await.is_err() {
                                return;
                            }
                        }
                        // Neither field populated → keepalive frame, skip.
                    }
                    Err(status) => {
                        let _ = expand_tx.send(Err(status_error(status))).await;
                        return;
                    }
                }
            }
        });

        let stream = tokio_stream::wrappers::ReceiverStream::new(expand_rx);

        Ok(stream)
    }

    /// Acknowledge a successfully processed message.
    ///
    /// The message is permanently removed from the queue.
    pub async fn ack(&self, queue: &str, message_id: &str) -> Result<(), AckError> {
        self.inner
            .clone()
            .ack(self.request(AckRequest {
                queue: queue.to_string(),
                message_id: message_id.to_string(),
            }))
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
            .nack(self.request(NackRequest {
                queue: queue.to_string(),
                message_id: message_id.to_string(),
                error: error.to_string(),
            }))
            .await
            .map_err(nack_status_error)?;

        Ok(())
    }
}

/// Extract the leader's client address from gRPC error metadata.
/// Returns `Some(addr)` if the `x-fila-leader-addr` key is present.
fn extract_leader_hint(status: &tonic::Status) -> Option<String> {
    status
        .metadata()
        .get("x-fila-leader-addr")
        .and_then(|v| v.to_str().ok())
        .map(|s| s.to_string())
}
