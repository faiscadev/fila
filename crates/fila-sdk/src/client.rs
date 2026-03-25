use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Duration;

use fila_proto::fila_service_client::FilaServiceClient;
use fila_proto::{
    AckMessage, AckRequest, ConsumeRequest, EnqueueMessage as ProtoEnqueueMessage, EnqueueRequest,
    NackMessage, NackRequest, StreamEnqueueRequest, StreamEnqueueResponse,
};
use tokio::sync::{mpsc, oneshot, Mutex};
use tokio_stream::{Stream, StreamExt};
use tonic::transport::{Certificate, Channel, ClientTlsConfig, Identity};

use crate::error::{
    ack_status_error, consume_status_error, enqueue_status_error, nack_status_error, status_error,
    AckError, ConnectError, ConsumeError, EnqueueError, NackError, StatusError,
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

/// Controls how the SDK accumulates `enqueue()` calls for throughput.
///
/// The default is [`Auto`](AccumulatorMode::Auto) — Nagle-style adaptive
/// accumulation that requires zero configuration. It sends immediately when
/// idle and buffers while an RPC is in flight, flushing the buffer when the
/// RPC completes. Size tunes itself to the actual throughput/latency ratio.
#[derive(Debug, Clone)]
pub enum AccumulatorMode {
    /// Nagle-style adaptive accumulation (default).
    ///
    /// When no RPC is in flight, sends immediately (zero added latency).
    /// When an RPC is in flight, buffers incoming messages and flushes
    /// them as a single `Enqueue` when the in-flight RPC completes.
    /// Size adapts automatically: higher arrival rate or higher
    /// server latency → bigger flushes.
    Auto {
        /// Safety cap on flush size. Default: 100.
        max_batch_size: usize,
    },
    /// Timer-based accumulation with explicit settings.
    ///
    /// Buffers messages and flushes when either `batch_size` messages
    /// accumulate or `linger_ms` milliseconds elapse — whichever first.
    Linger {
        /// Time threshold in milliseconds before a partial flush is triggered.
        linger_ms: u64,
        /// Maximum messages per flush.
        batch_size: usize,
    },
    /// No accumulation. Each `enqueue()` is a separate single-message RPC.
    Disabled,
}

impl Default for AccumulatorMode {
    fn default() -> Self {
        Self::Auto {
            max_batch_size: 100,
        }
    }
}

/// A single message specification for enqueue operations.
#[derive(Debug, Clone)]
pub struct EnqueueMessage {
    pub queue: String,
    pub headers: HashMap<String, String>,
    pub payload: Vec<u8>,
}

/// Options for connecting to a Fila broker.
#[derive(Debug, Clone, Default)]
pub struct ConnectOptions {
    pub addr: String,
    pub timeout: Option<Duration>,
    /// Enable TLS using the operating system's trust store.
    pub tls: bool,
    /// PEM-encoded CA certificate for verifying the server's certificate.
    pub tls_ca_cert_pem: Option<Vec<u8>>,
    /// PEM-encoded client certificate for mTLS authentication.
    pub tls_client_cert_pem: Option<Vec<u8>>,
    /// PEM-encoded client private key for mTLS authentication.
    pub tls_client_key_pem: Option<Vec<u8>>,
    /// API key for authenticating with the broker.
    pub api_key: Option<String>,
    /// Accumulator mode for `enqueue()` calls.
    /// Default: [`AccumulatorMode::Auto`].
    pub accumulator: AccumulatorMode,
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
    pub fn with_tls(mut self) -> Self {
        self.tls = true;
        self
    }

    /// Set the CA certificate for verifying the server's TLS certificate.
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
    pub fn with_api_key(mut self, api_key: impl Into<String>) -> Self {
        self.api_key = Some(api_key.into());
        self
    }

    /// Set the accumulator mode for `enqueue()` calls.
    pub fn with_accumulator(mut self, mode: AccumulatorMode) -> Self {
        self.accumulator = mode;
        self
    }
}

/// An item sent to the background accumulator task.
struct AccumulatorItem {
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
    api_key: Option<String>,
    connect_options: Option<ConnectOptions>,
    accumulator_tx: Option<mpsc::Sender<AccumulatorItem>>,
}

impl FilaClient {
    /// Connect to a Fila broker at the given address.
    pub async fn connect(addr: impl Into<String>) -> Result<Self, ConnectError> {
        Self::connect_with_options(ConnectOptions::new(addr)).await
    }

    /// Connect to a Fila broker with custom options.
    pub async fn connect_with_options(options: ConnectOptions) -> Result<Self, ConnectError> {
        let stored_options = options.clone();
        let mut endpoint = Channel::from_shared(options.addr)
            .map_err(|e| ConnectError::InvalidArgument(e.to_string()))?;

        if let Some(timeout) = options.timeout {
            endpoint = endpoint.timeout(timeout);
        }

        let has_cert = options.tls_client_cert_pem.is_some();
        let has_key = options.tls_client_key_pem.is_some();
        if has_cert != has_key {
            return Err(ConnectError::InvalidArgument(
                "tls_client_cert_pem and tls_client_key_pem must both be provided for mTLS"
                    .to_string(),
            ));
        }

        let tls_enabled = options.tls || options.tls_ca_cert_pem.is_some() || has_cert;
        if tls_enabled {
            let mut tls = ClientTlsConfig::new();
            if let Some(ca_pem) = options.tls_ca_cert_pem {
                tls = tls.ca_certificate(Certificate::from_pem(ca_pem));
            }
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

        let accumulator_tx = match &stored_options.accumulator {
            AccumulatorMode::Auto { max_batch_size } => {
                let max_batch_size = *max_batch_size;
                let (tx, rx) = mpsc::channel::<AccumulatorItem>(max_batch_size * 2);
                let client = inner.clone();
                let key = api_key.clone();
                tokio::spawn(run_auto_accumulator(rx, client, key, max_batch_size));
                Some(tx)
            }
            AccumulatorMode::Linger {
                linger_ms,
                batch_size,
            } => {
                let (tx, rx) = mpsc::channel::<AccumulatorItem>(*batch_size * 2);
                let client = inner.clone();
                let key = api_key.clone();
                let batch_size = *batch_size;
                let linger_ms = *linger_ms;
                tokio::spawn(run_linger_accumulator(
                    rx, client, key, batch_size, linger_ms,
                ));
                Some(tx)
            }
            AccumulatorMode::Disabled => None,
        };

        Ok(Self {
            inner,
            api_key,
            connect_options: Some(stored_options),
            accumulator_tx,
        })
    }

    /// Build a tonic `Request<T>` with the API key authorization header attached.
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

    /// Enqueue a single message to a queue.
    ///
    /// Returns the broker-assigned message ID (UUIDv7).
    ///
    /// When accumulation is enabled (default), the message is buffered and
    /// flushed when the accumulator decides to send.
    pub async fn enqueue(
        &self,
        queue: &str,
        headers: HashMap<String, String>,
        payload: impl Into<Vec<u8>>,
    ) -> Result<String, EnqueueError> {
        if let Some(ref tx) = self.accumulator_tx {
            let (result_tx, result_rx) = oneshot::channel();
            let item = AccumulatorItem {
                message: EnqueueMessage {
                    queue: queue.to_string(),
                    headers,
                    payload: payload.into(),
                },
                result_tx,
            };
            tx.send(item).await.map_err(|_| {
                EnqueueError::Status(StatusError::Internal(
                    "accumulator task has shut down".to_string(),
                ))
            })?;
            return result_rx.await.map_err(|_| {
                EnqueueError::Status(StatusError::Internal(
                    "accumulator dropped result channel".to_string(),
                ))
            })?;
        }

        // No accumulator — send directly via unified Enqueue RPC.
        let results = self
            .enqueue_many(vec![EnqueueMessage {
                queue: queue.to_string(),
                headers,
                payload: payload.into(),
            }])
            .await;

        results
            .into_iter()
            .next()
            .unwrap_or(Err(EnqueueError::Status(StatusError::Internal(
                "no results in enqueue response".to_string(),
            ))))
    }

    /// Enqueue multiple messages in a single RPC call.
    ///
    /// Each message is independently validated and processed. A failed message
    /// does not affect the others. Returns one `Result` per input message, in
    /// the same order.
    pub async fn enqueue_many(
        &self,
        messages: Vec<EnqueueMessage>,
    ) -> Vec<Result<String, EnqueueError>> {
        let proto_messages: Vec<ProtoEnqueueMessage> = messages
            .into_iter()
            .map(|m| ProtoEnqueueMessage {
                queue: m.queue,
                headers: m.headers,
                payload: bytes::Bytes::from(m.payload),
            })
            .collect();

        let msg_count = proto_messages.len();
        let result = self
            .inner
            .clone()
            .enqueue(self.request(EnqueueRequest {
                messages: proto_messages,
            }))
            .await;

        match result {
            Ok(response) => response
                .into_inner()
                .results
                .into_iter()
                .map(parse_enqueue_result)
                .collect(),
            Err(status) => {
                // RPC-level failure — broadcast the exact error to all items.
                let code = status.code();
                let message = status.message().to_string();
                let count = msg_count.max(1);
                (0..count)
                    .map(|_| {
                        Err(enqueue_status_error(tonic::Status::new(
                            code,
                            message.clone(),
                        )))
                    })
                    .collect()
            }
        }
    }

    /// Open a streaming consumer on a queue.
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
        // reconnect to the hinted leader and retry once.
        let response = match result {
            Ok(resp) => resp,
            Err(status)
                if status.code() == tonic::Code::Unavailable
                    && extract_leader_hint(&status).is_some() =>
            {
                let leader_addr = extract_leader_hint(&status).unwrap();
                let leader_url =
                    if leader_addr.starts_with("http://") || leader_addr.starts_with("https://") {
                        leader_addr
                    } else {
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
                let leader_opts = match self.connect_options {
                    Some(ref opts) => ConnectOptions {
                        addr: leader_url,
                        timeout: opts.timeout,
                        tls: opts.tls,
                        tls_ca_cert_pem: opts.tls_ca_cert_pem.clone(),
                        tls_client_cert_pem: opts.tls_client_cert_pem.clone(),
                        tls_client_key_pem: opts.tls_client_key_pem.clone(),
                        api_key: opts.api_key.clone(),
                        accumulator: AccumulatorMode::Disabled,
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
                        return Err(consume_status_error(status));
                    }
                }
            }
            Err(status) => return Err(consume_status_error(status)),
        };

        let (expand_tx, expand_rx) =
            tokio::sync::mpsc::channel::<Result<ConsumeMessage, StatusError>>(64);

        let mut inner_stream = response.into_inner();
        tokio::spawn(async move {
            loop {
                let result = tokio::select! {
                    r = inner_stream.next() => r,
                    _ = expand_tx.closed() => return,
                };
                match result {
                    Some(Ok(consume_response)) => {
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
    pub async fn ack(&self, queue: &str, message_id: &str) -> Result<(), AckError> {
        let response = self
            .inner
            .clone()
            .ack(self.request(AckRequest {
                messages: vec![AckMessage {
                    queue: queue.to_string(),
                    message_id: message_id.to_string(),
                }],
            }))
            .await
            .map_err(ack_status_error)?;

        let results = response.into_inner().results;
        if let Some(result) = results.into_iter().next() {
            match result.result {
                Some(fila_proto::ack_result::Result::Success(_)) => Ok(()),
                Some(fila_proto::ack_result::Result::Error(err)) => {
                    match fila_proto::AckErrorCode::try_from(err.code) {
                        Ok(fila_proto::AckErrorCode::MessageNotFound) => {
                            Err(AckError::MessageNotFound(err.message))
                        }
                        Ok(fila_proto::AckErrorCode::PermissionDenied) => {
                            Err(AckError::PermissionDenied(err.message))
                        }
                        _ => Err(AckError::Status(StatusError::Internal(err.message))),
                    }
                }
                None => Err(AckError::Status(StatusError::Internal(
                    "no result in ack response".to_string(),
                ))),
            }
        } else {
            Err(AckError::Status(StatusError::Internal(
                "empty ack response".to_string(),
            )))
        }
    }

    /// Negatively acknowledge a message that failed processing.
    pub async fn nack(&self, queue: &str, message_id: &str, error: &str) -> Result<(), NackError> {
        let response = self
            .inner
            .clone()
            .nack(self.request(NackRequest {
                messages: vec![NackMessage {
                    queue: queue.to_string(),
                    message_id: message_id.to_string(),
                    error: error.to_string(),
                }],
            }))
            .await
            .map_err(nack_status_error)?;

        let results = response.into_inner().results;
        if let Some(result) = results.into_iter().next() {
            match result.result {
                Some(fila_proto::nack_result::Result::Success(_)) => Ok(()),
                Some(fila_proto::nack_result::Result::Error(err)) => {
                    match fila_proto::NackErrorCode::try_from(err.code) {
                        Ok(fila_proto::NackErrorCode::MessageNotFound) => {
                            Err(NackError::MessageNotFound(err.message))
                        }
                        Ok(fila_proto::NackErrorCode::PermissionDenied) => {
                            Err(NackError::PermissionDenied(err.message))
                        }
                        _ => Err(NackError::Status(StatusError::Internal(err.message))),
                    }
                }
                None => Err(NackError::Status(StatusError::Internal(
                    "no result in nack response".to_string(),
                ))),
            }
        } else {
            Err(NackError::Status(StatusError::Internal(
                "empty nack response".to_string(),
            )))
        }
    }
}

/// Extract the leader's client address from gRPC error metadata.
fn extract_leader_hint(status: &tonic::Status) -> Option<String> {
    status
        .metadata()
        .get("x-fila-leader-addr")
        .and_then(|v| v.to_str().ok())
        .map(|s| s.to_string())
}

/// Parse a single EnqueueResult proto into a Rust Result.
fn parse_enqueue_result(result: fila_proto::EnqueueResult) -> Result<String, EnqueueError> {
    match result.result {
        Some(fila_proto::enqueue_result::Result::MessageId(id)) => Ok(id),
        Some(fila_proto::enqueue_result::Result::Error(err)) => {
            match fila_proto::EnqueueErrorCode::try_from(err.code) {
                Ok(fila_proto::EnqueueErrorCode::QueueNotFound) => {
                    Err(EnqueueError::QueueNotFound(err.message))
                }
                Ok(fila_proto::EnqueueErrorCode::PermissionDenied) => {
                    Err(EnqueueError::PermissionDenied(err.message))
                }
                _ => Err(EnqueueError::Status(StatusError::Internal(err.message))),
            }
        }
        None => Err(EnqueueError::Status(StatusError::Internal(
            "no result from server".to_string(),
        ))),
    }
}

// --- Streaming enqueue infrastructure ---

type PendingMap = HashMap<u64, oneshot::Sender<Result<String, EnqueueError>>>;

struct ActiveStream {
    sender: mpsc::Sender<StreamEnqueueRequest>,
    pending: Arc<Mutex<PendingMap>>,
    _reader_handle: tokio::task::JoinHandle<()>,
}

struct StreamManager {
    client: FilaServiceClient<Channel>,
    api_key: Option<String>,
    seq: AtomicU64,
    active: Option<ActiveStream>,
    fallback_to_unary: bool,
}

impl StreamManager {
    fn new(client: FilaServiceClient<Channel>, api_key: Option<String>) -> Self {
        Self {
            client,
            api_key,
            seq: AtomicU64::new(1),
            active: None,
            fallback_to_unary: false,
        }
    }

    async fn open_stream(&mut self) -> Result<(), tonic::Status> {
        let (req_tx, req_rx) = mpsc::channel::<StreamEnqueueRequest>(256);
        let req_stream = tokio_stream::wrappers::ReceiverStream::new(req_rx);

        let mut request = tonic::Request::new(req_stream);
        attach_api_key(&mut request, &self.api_key);

        let response = self.client.clone().stream_enqueue(request).await;

        match response {
            Ok(resp) => {
                let mut resp_stream = resp.into_inner();
                let pending: Arc<Mutex<PendingMap>> = Arc::new(Mutex::new(HashMap::new()));
                let pending_clone = Arc::clone(&pending);

                let reader_handle = tokio::spawn(async move {
                    while let Some(result) = resp_stream.next().await {
                        match result {
                            Ok(resp) => {
                                resolve_stream_response(&pending_clone, resp).await;
                            }
                            Err(status) => {
                                error_all_pending(
                                    &pending_clone,
                                    &format!("stream error: {status}"),
                                )
                                .await;
                                return;
                            }
                        }
                    }
                    error_all_pending(&pending_clone, "stream closed by server").await;
                });

                self.active = Some(ActiveStream {
                    sender: req_tx,
                    pending,
                    _reader_handle: reader_handle,
                });
                Ok(())
            }
            Err(status) if status.code() == tonic::Code::Unimplemented => {
                self.fallback_to_unary = true;
                Err(status)
            }
            Err(status) => Err(status),
        }
    }

    async fn ensure_stream(&mut self) -> bool {
        if self.fallback_to_unary {
            return false;
        }
        if self.active.is_some() {
            return true;
        }
        self.open_stream().await.is_ok()
    }

    /// Send a batch of messages through the stream. Each item is sent as a
    /// stream write with one message (batch-within-stream is a future story).
    async fn send_batch(&mut self, items: Vec<AccumulatorItem>) -> Vec<AccumulatorItem> {
        let active = match self.active.as_ref() {
            Some(a) => a,
            None => return items,
        };

        let mut prepared: Vec<(StreamEnqueueRequest, u64)> = Vec::with_capacity(items.len());
        {
            let mut pending = active.pending.lock().await;
            for item in &items {
                let seq = self.seq.fetch_add(1, Ordering::Relaxed);
                prepared.push((
                    StreamEnqueueRequest {
                        messages: vec![ProtoEnqueueMessage {
                            queue: item.message.queue.clone(),
                            headers: item.message.headers.clone(),
                            payload: bytes::Bytes::from(item.message.payload.clone()),
                        }],
                        sequence_number: seq,
                    },
                    seq,
                ));
            }
            let items_vec: Vec<AccumulatorItem> = items.into_iter().collect();
            for (i, item) in items_vec.into_iter().enumerate() {
                let seq = prepared[i].1;
                pending.insert(seq, item.result_tx);
            }
        }

        let mut unsent_seqs: Vec<u64> = Vec::new();
        let mut failed = false;
        for (req, seq) in prepared {
            if failed {
                unsent_seqs.push(seq);
                continue;
            }
            match active.sender.try_send(req) {
                Ok(()) => {}
                Err(mpsc::error::TrySendError::Full(req)) => {
                    if active.sender.send(req).await.is_err() {
                        unsent_seqs.push(seq);
                        failed = true;
                    }
                }
                Err(mpsc::error::TrySendError::Closed(_)) => {
                    unsent_seqs.push(seq);
                    failed = true;
                }
            }
        }

        if !unsent_seqs.is_empty() {
            let mut pending = active.pending.lock().await;
            for seq in unsent_seqs {
                if let Some(tx) = pending.remove(&seq) {
                    let _ = tx.send(Err(EnqueueError::Status(StatusError::Internal(
                        "stream broke during send".to_string(),
                    ))));
                }
            }
            drop(pending);
            self.active = None;
        }

        Vec::new()
    }
}

fn parse_grpc_code(s: &str) -> tonic::Code {
    match s {
        "Ok" => tonic::Code::Ok,
        "Cancelled" => tonic::Code::Cancelled,
        "Unknown" => tonic::Code::Unknown,
        "InvalidArgument" => tonic::Code::InvalidArgument,
        "DeadlineExceeded" => tonic::Code::DeadlineExceeded,
        "NotFound" => tonic::Code::NotFound,
        "AlreadyExists" => tonic::Code::AlreadyExists,
        "PermissionDenied" => tonic::Code::PermissionDenied,
        "ResourceExhausted" => tonic::Code::ResourceExhausted,
        "FailedPrecondition" => tonic::Code::FailedPrecondition,
        "Aborted" => tonic::Code::Aborted,
        "OutOfRange" => tonic::Code::OutOfRange,
        "Unimplemented" => tonic::Code::Unimplemented,
        "Internal" => tonic::Code::Internal,
        "Unavailable" => tonic::Code::Unavailable,
        "DataLoss" => tonic::Code::DataLoss,
        "Unauthenticated" => tonic::Code::Unauthenticated,
        _ => tonic::Code::Unknown,
    }
}

fn parse_stream_error(err: &str) -> EnqueueError {
    if let Some(rest) = err.strip_prefix('[') {
        if let Some(bracket_end) = rest.find(']') {
            let code_str = &rest[..bracket_end];
            let message = rest[bracket_end + 1..].trim_start().to_string();
            let code = parse_grpc_code(code_str);
            let status = tonic::Status::new(code, message);
            return enqueue_status_error(status);
        }
    }
    EnqueueError::Status(StatusError::Internal(err.to_string()))
}

/// Resolve a stream response. Each response carries results for a single
/// stream write (currently one message per write, so one result per response).
async fn resolve_stream_response(
    pending: &Mutex<HashMap<u64, oneshot::Sender<Result<String, EnqueueError>>>>,
    response: StreamEnqueueResponse,
) {
    let seq = response.sequence_number;

    // Take the first result (one message per stream write for now).
    let result = if let Some(first_result) = response.results.into_iter().next() {
        match first_result.result {
            Some(fila_proto::enqueue_result::Result::MessageId(id)) => Ok(id),
            Some(fila_proto::enqueue_result::Result::Error(err)) => {
                Err(parse_stream_error(&format!(
                    "[{}] {}",
                    match fila_proto::EnqueueErrorCode::try_from(err.code) {
                        Ok(fila_proto::EnqueueErrorCode::QueueNotFound) => "NotFound",
                        Ok(fila_proto::EnqueueErrorCode::PermissionDenied) => "PermissionDenied",
                        Ok(fila_proto::EnqueueErrorCode::Storage) => "Internal",
                        Ok(fila_proto::EnqueueErrorCode::Lua) => "Internal",
                        _ => "Internal",
                    },
                    err.message
                )))
            }
            None => Err(EnqueueError::Status(StatusError::Internal(
                "no result in stream response".to_string(),
            ))),
        }
    } else {
        Err(EnqueueError::Status(StatusError::Internal(
            "empty results in stream response".to_string(),
        )))
    };

    let mut map = pending.lock().await;
    if let Some(tx) = map.remove(&seq) {
        let _ = tx.send(result);
    }
}

async fn error_all_pending(
    pending: &Mutex<HashMap<u64, oneshot::Sender<Result<String, EnqueueError>>>>,
    error_msg: &str,
) {
    let mut map = pending.lock().await;
    let entries: Vec<_> = map.drain().collect();
    for (_seq, tx) in entries {
        let _ = tx.send(Err(EnqueueError::Status(StatusError::Internal(
            error_msg.to_string(),
        ))));
    }
}

// --- Accumulator implementations ---

async fn run_auto_accumulator(
    mut rx: mpsc::Receiver<AccumulatorItem>,
    client: FilaServiceClient<Channel>,
    api_key: Option<String>,
    max_batch_size: usize,
) {
    let mut stream_mgr = StreamManager::new(client.clone(), api_key.clone());

    loop {
        let first = match rx.recv().await {
            Some(item) => item,
            None => return,
        };

        let mut batch = Vec::with_capacity(max_batch_size);
        batch.push(first);
        while batch.len() < max_batch_size {
            match rx.try_recv() {
                Ok(item) => batch.push(item),
                Err(_) => break,
            }
        }

        if stream_mgr.ensure_stream().await {
            let unsent = stream_mgr.send_batch(batch).await;
            if !unsent.is_empty() {
                for item in unsent {
                    let _ = item
                        .result_tx
                        .send(Err(EnqueueError::Status(StatusError::Internal(
                            "stream broke during send".to_string(),
                        ))));
                }
            }
        } else {
            let c = client.clone();
            let k = api_key.clone();
            tokio::spawn(async move {
                flush_items(batch, &c, &k).await;
            });
        }
    }
}

async fn run_linger_accumulator(
    mut rx: mpsc::Receiver<AccumulatorItem>,
    client: FilaServiceClient<Channel>,
    api_key: Option<String>,
    batch_size: usize,
    linger_ms: u64,
) {
    let mut stream_mgr = StreamManager::new(client.clone(), api_key.clone());
    let mut buffer: Vec<AccumulatorItem> = Vec::with_capacity(batch_size);

    loop {
        if buffer.is_empty() {
            match rx.recv().await {
                Some(item) => {
                    buffer.push(item);
                    if buffer.len() >= batch_size {
                        flush_with_stream(&mut stream_mgr, &mut buffer, &client, &api_key).await;
                    }
                }
                None => return,
            }
        } else {
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
                                flush_with_stream(
                                    &mut stream_mgr,
                                    &mut buffer,
                                    &client,
                                    &api_key,
                                )
                                .await;
                                break;
                            }
                        }
                        None => {
                            flush_with_stream(
                                &mut stream_mgr,
                                &mut buffer,
                                &client,
                                &api_key,
                            )
                            .await;
                            return;
                        }
                    },
                    _ = &mut sleep => {
                        flush_with_stream(
                            &mut stream_mgr,
                            &mut buffer,
                            &client,
                            &api_key,
                        )
                        .await;
                        break;
                    }
                }
            }
        }
    }
}

async fn flush_with_stream(
    stream_mgr: &mut StreamManager,
    buffer: &mut Vec<AccumulatorItem>,
    client: &FilaServiceClient<Channel>,
    api_key: &Option<String>,
) {
    let items: Vec<AccumulatorItem> = std::mem::take(buffer);
    if items.is_empty() {
        return;
    }

    if stream_mgr.ensure_stream().await {
        let unsent = stream_mgr.send_batch(items).await;
        if !unsent.is_empty() {
            for item in unsent {
                let _ = item
                    .result_tx
                    .send(Err(EnqueueError::Status(StatusError::Internal(
                        "stream broke during send".to_string(),
                    ))));
            }
        }
    } else {
        flush_items(items, client, api_key).await;
    }
}

/// Flush items via the unified Enqueue RPC and fan results back to callers.
async fn flush_items(
    items: Vec<AccumulatorItem>,
    client: &FilaServiceClient<Channel>,
    api_key: &Option<String>,
) {
    if items.is_empty() {
        return;
    }

    let proto_messages: Vec<ProtoEnqueueMessage> = items
        .iter()
        .map(|item| ProtoEnqueueMessage {
            queue: item.message.queue.clone(),
            headers: item.message.headers.clone(),
            payload: bytes::Bytes::from(item.message.payload.clone()),
        })
        .collect();

    let mut req = tonic::Request::new(EnqueueRequest {
        messages: proto_messages,
    });
    attach_api_key(&mut req, api_key);

    let result = client.clone().enqueue(req).await;

    match result {
        Ok(response) => {
            let results = response.into_inner().results;
            let mut result_iter = results.into_iter();
            for item in items {
                let mapped = match result_iter.next() {
                    Some(result) => parse_enqueue_result(result),
                    None => Err(EnqueueError::Status(StatusError::Internal(
                        "server returned fewer results than messages sent".to_string(),
                    ))),
                };
                let _ = item.result_tx.send(mapped);
            }
        }
        Err(status) => {
            let code = status.code();
            let message = status.message().to_string();
            for item in items {
                let _ = item
                    .result_tx
                    .send(Err(enqueue_status_error(tonic::Status::new(
                        code,
                        message.clone(),
                    ))));
            }
        }
    }
}

fn attach_api_key<T>(req: &mut tonic::Request<T>, api_key: &Option<String>) {
    if let Some(ref key) = api_key {
        if let Ok(val) = tonic::metadata::MetadataValue::try_from(format!("Bearer {key}").as_str())
        {
            req.metadata_mut().insert("authorization", val);
        }
    }
}
