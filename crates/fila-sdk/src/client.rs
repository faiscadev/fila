use std::collections::HashMap;
use std::time::Duration;

use fila_proto::fila_service_client::FilaServiceClient;
use fila_proto::{AckRequest, BatchEnqueueRequest, ConsumeRequest, EnqueueRequest, NackRequest};
use tokio::sync::{mpsc, oneshot};
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
    /// Client-side batching configuration.
    /// When `linger_ms` is set, `enqueue()` buffers messages and flushes
    /// them via `BatchEnqueue` RPC when the batch is full or the timer fires.
    pub batch_config: Option<BatchConfig>,
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

    /// Enable auto-batching on the client.
    ///
    /// When configured with `linger_ms`, `enqueue()` buffers messages and
    /// flushes via `BatchEnqueue` RPC when either `batch_size` messages
    /// accumulate or `linger_ms` milliseconds elapse.
    pub fn with_batch_config(mut self, config: BatchConfig) -> Self {
        self.batch_config = Some(config);
        self
    }
}

/// An item sent to the background batcher task.
struct BatchItem {
    message: EnqueueMessage,
    result_tx: oneshot::Sender<Result<String, EnqueueError>>,
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
    /// Channel to the background batcher task. Present when auto-batching is enabled.
    batcher_tx: Option<mpsc::Sender<BatchItem>>,
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
            batcher_tx: None,
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

        // Spawn the auto-batcher task if configured with linger_ms.
        let batcher_tx = match &stored_options.batch_config {
            Some(config) if config.linger_ms.is_some() => {
                let (tx, rx) = mpsc::channel::<BatchItem>(config.batch_size * 2);
                let batcher_client = inner.clone();
                let batcher_api_key = api_key.clone();
                let batch_size = config.batch_size;
                let linger_ms = config.linger_ms.unwrap();
                tokio::spawn(run_batcher(
                    rx,
                    batcher_client,
                    batcher_api_key,
                    batch_size,
                    linger_ms,
                ));
                Some(tx)
            }
            _ => None,
        };

        Ok(Self {
            inner,
            api_key,
            connect_options: Some(stored_options),
            batcher_tx,
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
    ///
    /// When auto-batching is enabled (via [`ConnectOptions::with_batch_config`]),
    /// the message is buffered and flushed via `BatchEnqueue` RPC when either
    /// `batch_size` messages accumulate or `linger_ms` milliseconds elapse.
    /// The returned future resolves when the batch containing this message
    /// is flushed and acknowledged.
    pub async fn enqueue(
        &self,
        queue: &str,
        headers: HashMap<String, String>,
        payload: impl Into<Vec<u8>>,
    ) -> Result<String, EnqueueError> {
        // Route through the batcher when auto-batching is enabled.
        if let Some(ref tx) = self.batcher_tx {
            let (result_tx, result_rx) = oneshot::channel();
            let item = BatchItem {
                message: EnqueueMessage {
                    queue: queue.to_string(),
                    headers,
                    payload: payload.into(),
                },
                result_tx,
            };
            tx.send(item).await.map_err(|_| {
                EnqueueError::Status(StatusError::Internal(
                    "auto-batcher task has shut down".to_string(),
                ))
            })?;
            return result_rx.await.map_err(|_| {
                EnqueueError::Status(StatusError::Internal(
                    "auto-batcher dropped result channel".to_string(),
                ))
            })?;
        }

        let response = self
            .inner
            .clone()
            .enqueue(self.request(EnqueueRequest {
                queue: queue.to_string(),
                headers,
                payload: bytes::Bytes::from(payload.into()),
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
                payload: bytes::Bytes::from(m.payload),
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
                        batch_config: None,
                    },
                    None => ConnectOptions::new(leader_url),
                };
                // Connect to leader without auto-batching (consume doesn't need it).
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
            loop {
                // Race the server stream against consumer-side closure so the
                // gRPC stream is cancelled promptly when the consumer drops.
                let result = tokio::select! {
                    r = inner_stream.next() => r,
                    _ = expand_tx.closed() => return,
                };
                match result {
                    Some(Ok(consume_response)) => {
                        // Prefer the batched `messages` field when non-empty.
                        if !consume_response.messages.is_empty() {
                            for msg in consume_response.messages {
                                let metadata = msg.metadata.unwrap_or_default();
                                let cm = ConsumeMessage {
                                    id: msg.id,
                                    headers: msg.headers,
                                    payload: msg.payload.to_vec(),
                                    fairness_key: metadata.fairness_key,
                                    attempt_count: metadata.attempt_count,
                                    queue: metadata.queue_id,
                                };
                                if expand_tx.send(Ok(cm)).await.is_err() {
                                    return;
                                }
                            }
                        } else if let Some(msg) = consume_response.message {
                            let metadata = msg.metadata.unwrap_or_default();
                            let cm = ConsumeMessage {
                                id: msg.id,
                                headers: msg.headers,
                                payload: msg.payload.to_vec(),
                                fairness_key: metadata.fairness_key,
                                attempt_count: metadata.attempt_count,
                                queue: metadata.queue_id,
                            };
                            if expand_tx.send(Ok(cm)).await.is_err() {
                                return;
                            }
                        }
                    }
                    Some(Err(status)) => {
                        let _ = expand_tx.send(Err(status_error(status))).await;
                        return;
                    }
                    None => return,
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

/// Background task that accumulates enqueue requests and flushes them in batches.
///
/// Flushes when either `batch_size` messages accumulate or `linger_ms` elapses
/// since the first message in the current batch. When all senders are dropped
/// (client dropped), flushes any remaining messages before exiting.
async fn run_batcher(
    mut rx: mpsc::Receiver<BatchItem>,
    client: FilaServiceClient<Channel>,
    api_key: Option<String>,
    batch_size: usize,
    linger_ms: u64,
) {
    let mut buffer: Vec<BatchItem> = Vec::with_capacity(batch_size);

    loop {
        if buffer.is_empty() {
            // No pending items — block until the next message arrives.
            match rx.recv().await {
                Some(item) => {
                    buffer.push(item);
                    if buffer.len() >= batch_size {
                        flush_batch(&mut buffer, &client, &api_key).await;
                    }
                }
                // All senders dropped — nothing to flush.
                None => return,
            }
        } else {
            // Buffer has items — race between more messages and the linger timer.
            let deadline = tokio::time::Instant::now() + Duration::from_millis(linger_ms);
            let sleep = tokio::time::sleep_until(deadline);
            tokio::pin!(sleep);

            loop {
                tokio::select! {
                    biased;
                    msg = rx.recv() => match msg {
                        Some(item) => {
                            buffer.push(item);
                            if buffer.len() >= batch_size {
                                flush_batch(&mut buffer, &client, &api_key).await;
                                break; // Back to outer loop (buffer now empty).
                            }
                        }
                        None => {
                            // All senders dropped — flush remaining and exit.
                            flush_batch(&mut buffer, &client, &api_key).await;
                            return;
                        }
                    },
                    _ = &mut sleep => {
                        flush_batch(&mut buffer, &client, &api_key).await;
                        break; // Back to outer loop.
                    }
                }
            }
        }
    }
}

/// Flush the buffered messages via BatchEnqueue RPC and fan results back
/// to individual callers via their oneshot senders.
async fn flush_batch(
    buffer: &mut Vec<BatchItem>,
    client: &FilaServiceClient<Channel>,
    api_key: &Option<String>,
) {
    let items: Vec<BatchItem> = std::mem::take(buffer);
    if items.is_empty() {
        return;
    }

    let proto_messages: Vec<EnqueueRequest> = items
        .iter()
        .map(|item| EnqueueRequest {
            queue: item.message.queue.clone(),
            headers: item.message.headers.clone(),
            payload: bytes::Bytes::from(item.message.payload.clone()),
        })
        .collect();

    let mut req = tonic::Request::new(BatchEnqueueRequest {
        messages: proto_messages,
    });
    if let Some(ref key) = api_key {
        if let Ok(val) = tonic::metadata::MetadataValue::try_from(format!("Bearer {key}").as_str())
        {
            req.metadata_mut().insert("authorization", val);
        }
    }

    let result = client.clone().batch_enqueue(req).await;

    match result {
        Ok(response) => {
            let results = response.into_inner().results;
            for (item, result) in items.into_iter().zip(results.into_iter()) {
                let mapped = match result.result {
                    Some(fila_proto::batch_enqueue_result::Result::Success(resp)) => {
                        Ok(resp.message_id)
                    }
                    Some(fila_proto::batch_enqueue_result::Result::Error(err)) => {
                        Err(EnqueueError::Status(StatusError::Internal(err)))
                    }
                    None => Err(EnqueueError::Status(StatusError::Internal(
                        "no result from server".to_string(),
                    ))),
                };
                let _ = item.result_tx.send(mapped);
            }
        }
        Err(status) => {
            // Transport-level failure — all messages in this batch get the error.
            let err = batch_enqueue_status_error(status);
            let msg = err.to_string();
            for item in items {
                let _ = item
                    .result_tx
                    .send(Err(EnqueueError::Status(StatusError::Internal(
                        msg.clone(),
                    ))));
            }
        }
    }
}
