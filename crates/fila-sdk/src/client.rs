use std::collections::HashMap;
use std::time::Duration;

use crate::error::{AckError, ConnectError, ConsumeError, EnqueueError, NackError, StatusError};
use crate::fibp_transport::{ConsumeStream, FibpTransport};

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
/// idle and buffers while a request is in flight, flushing the buffer when the
/// request completes. Size tunes itself to the actual throughput/latency ratio.
#[derive(Debug, Clone)]
pub enum AccumulatorMode {
    /// Nagle-style adaptive accumulation (default).
    ///
    /// When no request is in flight, sends immediately (zero added latency).
    /// When a request is in flight, buffers incoming messages and flushes
    /// them as a single `Enqueue` when the in-flight request completes.
    /// Size adapts automatically: higher arrival rate or higher
    /// server latency -> bigger flushes.
    Auto {
        /// Safety cap on flush size. Default: 100.
        max_batch_size: usize,
    },
    /// Timer-based accumulation with explicit settings.
    ///
    /// Buffers messages and flushes when either `batch_size` messages
    /// accumulate or `linger_ms` milliseconds elapse -- whichever first.
    /// Default: `linger_ms=5`, `batch_size=100` (matching Kafka's `linger.ms`).
    Linger {
        /// Time threshold in milliseconds before a partial flush is triggered.
        /// Default: 5 (matching Kafka's `linger.ms=5`).
        linger_ms: u64,
        /// Maximum messages per flush. Default: 100.
        batch_size: usize,
    },
    /// No accumulation. Each `enqueue()` is a separate single-message request.
    Disabled,
}

impl AccumulatorMode {
    /// Create a Linger accumulator with default settings (5ms, batch size 100).
    pub fn linger() -> Self {
        Self::Linger {
            linger_ms: 5,
            batch_size: 100,
        }
    }
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

/// Idiomatic Rust client for the Fila message broker.
///
/// Wraps the hot-path operations: enqueue, consume, ack, nack.
/// The client is `Clone`, `Send`, and `Sync` — it can be shared across tasks.
///
/// Uses the FIBP binary protocol over TCP.
#[derive(Debug, Clone)]
pub struct FilaClient {
    fibp: FibpTransport,
}

impl FilaClient {
    /// Connect to a Fila broker at the given address.
    pub async fn connect(addr: impl Into<String>) -> Result<Self, ConnectError> {
        Self::connect_with_options(ConnectOptions::new(addr)).await
    }

    /// Connect to a Fila broker with custom options.
    pub async fn connect_with_options(options: ConnectOptions) -> Result<Self, ConnectError> {
        let has_cert = options.tls_client_cert_pem.is_some();
        let has_key = options.tls_client_key_pem.is_some();
        if has_cert != has_key {
            return Err(ConnectError::InvalidArgument(
                "tls_client_cert_pem and tls_client_key_pem must both be provided for mTLS"
                    .to_string(),
            ));
        }

        let tls_enabled = options.tls || options.tls_ca_cert_pem.is_some() || has_cert;

        // For FIBP, the address is a raw TCP address (not a URL).
        // Strip any http:// or https:// prefix if present.
        let addr = options
            .addr
            .strip_prefix("http://")
            .or_else(|| options.addr.strip_prefix("https://"))
            .unwrap_or(&options.addr);

        let fibp = if tls_enabled {
            FibpTransport::connect_tls(
                addr,
                options.api_key.clone(),
                options.tls_ca_cert_pem.as_deref(),
                options.tls_client_cert_pem.as_deref(),
                options.tls_client_key_pem.as_deref(),
            )
            .await?
        } else {
            FibpTransport::connect(addr, options.api_key.clone()).await?
        };

        Ok(Self { fibp })
    }

    /// Enqueue a single message to a queue.
    ///
    /// Returns the broker-assigned message ID (UUIDv7).
    pub async fn enqueue(
        &self,
        queue: &str,
        headers: HashMap<String, String>,
        payload: impl Into<Vec<u8>>,
    ) -> Result<String, EnqueueError> {
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

    /// Enqueue multiple messages in a single request.
    ///
    /// Each message is independently validated and processed. A failed message
    /// does not affect the others. Returns one `Result` per input message, in
    /// the same order.
    pub async fn enqueue_many(
        &self,
        messages: Vec<EnqueueMessage>,
    ) -> Vec<Result<String, EnqueueError>> {
        self.fibp.enqueue_many(messages).await
    }

    /// Open a streaming consumer on a queue.
    pub async fn consume(&self, queue: &str) -> Result<ConsumeStream, ConsumeError> {
        self.fibp.consume(queue).await
    }

    /// Acknowledge a successfully processed message.
    pub async fn ack(&self, queue: &str, message_id: &str) -> Result<(), AckError> {
        self.fibp.ack(queue, message_id).await
    }

    /// Negatively acknowledge a message that failed processing.
    pub async fn nack(&self, queue: &str, message_id: &str, error: &str) -> Result<(), NackError> {
        self.fibp.nack(queue, message_id, error).await
    }
}
