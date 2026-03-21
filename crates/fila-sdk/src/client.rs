use std::collections::HashMap;
use std::time::Duration;

use fila_proto::fila_admin_client::FilaAdminClient;
use fila_proto::fila_service_client::FilaServiceClient;
use fila_proto::{AckRequest, ConsumeRequest, EnqueueRequest, GetServerInfoRequest, NackRequest};
use tokio_stream::{Stream, StreamExt};
use tonic::transport::{Certificate, Channel, ClientTlsConfig, Identity};

use crate::error::{
    ack_status_error, consume_status_error, enqueue_status_error, nack_status_error, status_error,
    AckError, ConnectError, ConsumeError, EnqueueError, NackError, ServerInfoError, StatusError,
};

/// Server metadata returned by `GetServerInfo`.
#[derive(Debug, Clone)]
pub struct ServerInfo {
    pub server_version: String,
    pub proto_version: String,
    pub features: Vec<String>,
}

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

/// Options for connecting to a Fila broker.
#[derive(Debug, Clone, Default)]
pub struct ConnectOptions {
    pub addr: String,
    pub timeout: Option<Duration>,
    /// PEM-encoded CA certificate for verifying the server's certificate.
    /// Required when connecting to a TLS-enabled broker.
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
    channel: Channel,
    /// API key sent as `authorization: Bearer <key>` on every request.
    api_key: Option<String>,
}

impl FilaClient {
    /// Connect to a Fila broker at the given address.
    ///
    /// The address should include the scheme, e.g. `http://localhost:5555`.
    pub async fn connect(addr: impl Into<String>) -> Result<Self, ConnectError> {
        let channel = Channel::from_shared(addr.into())
            .map_err(|e| ConnectError::InvalidArgument(e.to_string()))?
            .connect()
            .await?;
        let inner = FilaServiceClient::new(channel.clone());
        Ok(Self {
            inner,
            channel,
            api_key: None,
        })
    }

    /// Connect to a Fila broker with custom options.
    ///
    /// When TLS fields are set in `options`, the connection uses TLS/mTLS.
    /// The address should use `https://` when TLS is enabled.
    pub async fn connect_with_options(options: ConnectOptions) -> Result<Self, ConnectError> {
        let mut endpoint = Channel::from_shared(options.addr)
            .map_err(|e| ConnectError::InvalidArgument(e.to_string()))?;

        if let Some(timeout) = options.timeout {
            endpoint = endpoint.timeout(timeout);
        }

        // Apply TLS config when CA cert is provided.
        if let Some(ca_pem) = options.tls_ca_cert_pem {
            let mut tls = ClientTlsConfig::new().ca_certificate(Certificate::from_pem(ca_pem));
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
        let inner = FilaServiceClient::new(channel.clone());
        Ok(Self {
            inner,
            channel,
            api_key: options.api_key,
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
        let response = self
            .inner
            .clone()
            .consume(self.request(ConsumeRequest {
                queue: queue.to_string(),
            }))
            .await
            .map_err(consume_status_error)?;

        let stream = response.into_inner().filter_map(|result| match result {
            Ok(consume_response) => {
                // The server may send ConsumeResponse frames with `message: None` as
                // keepalive signals on the streaming connection. These are expected
                // and silently skipped — they are not protocol errors.
                let msg = consume_response.message?;
                let metadata = msg.metadata.unwrap_or_default();
                Some(Ok(ConsumeMessage {
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

    /// Query the broker for server version, proto version, and supported features.
    ///
    /// This RPC does not require authentication. It can be used before
    /// authenticating to check compatibility.
    pub async fn get_server_info(&self) -> Result<ServerInfo, ServerInfoError> {
        let mut admin = FilaAdminClient::new(self.channel.clone());
        let resp = admin
            .get_server_info(GetServerInfoRequest {})
            .await
            .map_err(|s| ServerInfoError::Status(status_error(s)))?;
        let inner = resp.into_inner();
        Ok(ServerInfo {
            server_version: inner.server_version,
            proto_version: inner.proto_version,
            features: inner.features,
        })
    }

    /// Returns the SDK version (compile-time constant).
    pub fn sdk_version() -> &'static str {
        env!("CARGO_PKG_VERSION")
    }
}
