//! FIBP client transport for the Fila SDK.
//!
//! Provides a TCP-based binary transport implementing all Fila operations
//! using the FIBP wire protocol. Connects via raw
//! TCP, performs the 6-byte handshake, optionally authenticates, and then
//! multiplexes request/response frames using correlation IDs.
//!
//! A background I/O task handles reading server frames and writing client
//! frames. Request/response matching is done via correlation IDs with
//! oneshot channels. Consume stream push frames (FLAG_STREAM) are routed
//! to a separate mpsc channel.

use std::collections::HashMap;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::Arc;

use bytes::{Bytes, BytesMut};
use prost::Message as ProstMessage;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;
use tokio::sync::{mpsc, oneshot, Mutex};
use tokio_stream::StreamExt;
use tokio_util::codec::{Encoder, Framed};

use crate::client::{ConsumeMessage, EnqueueMessage};
use crate::error::{AckError, ConnectError, ConsumeError, EnqueueError, NackError, StatusError};
use crate::fibp_codec::{
    self, ack_nack_err, enqueue_err, AckNackResultItem, EnqueueResultItem, EnqueueWireMessage,
    FibpCodec, Frame, FLAG_STREAM, MAGIC, OP_ACK, OP_AUTH, OP_AUTH_CREATE_KEY, OP_AUTH_GET_ACL,
    OP_AUTH_LIST_KEYS, OP_AUTH_REVOKE_KEY, OP_AUTH_SET_ACL, OP_CONFIG_GET, OP_CONFIG_LIST,
    OP_CONFIG_SET, OP_CONSUME, OP_CREATE_QUEUE, OP_DELETE_QUEUE, OP_ENQUEUE, OP_ERROR, OP_FLOW,
    OP_GOAWAY, OP_LIST_QUEUES, OP_NACK, OP_QUEUE_STATS, OP_REDRIVE,
};

/// Maximum frame size for the SDK client (16 MB, matching server default).
const MAX_FRAME_SIZE: u32 = 16_777_216;

/// Initial credits granted to the server when opening a consume stream.
const INITIAL_CREDITS: u32 = 64;

/// Credits to grant per flow-control replenishment.
const FLOW_CREDITS: u32 = 32;

/// Pending request waiting for a response frame.
type PendingRequest = oneshot::Sender<Result<Frame, StatusError>>;

/// Sender for consume push frames.
type ConsumePushSender = mpsc::Sender<Result<ConsumeMessage, StatusError>>;

/// Internal command sent to the writer side of the I/O loop.
enum WriterCommand {
    SendFrame(Frame),
}

/// FIBP client transport.
///
/// Connects to a Fila broker via raw TCP using the FIBP binary protocol.
/// Thread-safe and cloneable via internal `Arc`.
#[derive(Clone)]
pub struct FibpTransport {
    inner: Arc<FibpTransportInner>,
}

/// Connection configuration kept so a leader-redirect can reuse the same
/// TLS and authentication settings as the original connection.
#[derive(Clone)]
struct TransportConnectConfig {
    api_key: Option<String>,
    tls_enabled: bool,
    tls_ca_cert_pem: Option<Vec<u8>>,
    tls_client_cert_pem: Option<Vec<u8>>,
    tls_client_key_pem: Option<Vec<u8>>,
}

struct FibpTransportInner {
    writer_tx: mpsc::Sender<WriterCommand>,
    pending: Arc<Mutex<HashMap<u32, PendingRequest>>>,
    /// Channel for consume push frames, set when consume is active.
    consume_push_tx: Arc<Mutex<Option<ConsumePushSender>>>,
    next_correlation_id: AtomicU32,
    /// Queue name for the active consume session.
    consume_queue: Arc<Mutex<Option<String>>>,
    /// Configuration used to create this transport, preserved for leader redirects.
    connect_config: TransportConnectConfig,
}

impl std::fmt::Debug for FibpTransport {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("FibpTransport").finish_non_exhaustive()
    }
}

impl FibpTransport {
    /// Connect to a Fila broker via FIBP over plain TCP.
    ///
    /// `addr` is a TCP address like `"127.0.0.1:5557"`.
    pub async fn connect(addr: &str, api_key: Option<String>) -> Result<Self, ConnectError> {
        let mut stream = TcpStream::connect(addr)
            .await
            .map_err(|e| ConnectError::InvalidArgument(format!("FIBP connect failed: {e}")))?;

        // Perform the 6-byte handshake: send magic, receive echo.
        stream.write_all(MAGIC).await.map_err(|e| {
            ConnectError::InvalidArgument(format!("FIBP handshake write failed: {e}"))
        })?;

        let mut server_magic = [0u8; 6];
        stream.read_exact(&mut server_magic).await.map_err(|e| {
            ConnectError::InvalidArgument(format!("FIBP handshake read failed: {e}"))
        })?;

        validate_server_magic(&server_magic)?;

        let codec = FibpCodec::new(MAX_FRAME_SIZE);
        let framed = Framed::new(stream, codec);

        let config = TransportConnectConfig {
            api_key,
            tls_enabled: false,
            tls_ca_cert_pem: None,
            tls_client_cert_pem: None,
            tls_client_key_pem: None,
        };
        Self::from_framed(framed, config).await
    }

    /// Connect to a Fila broker via FIBP over TLS.
    pub async fn connect_tls(
        addr: &str,
        api_key: Option<String>,
        tls_ca_cert_pem: Option<&[u8]>,
        tls_client_cert_pem: Option<&[u8]>,
        tls_client_key_pem: Option<&[u8]>,
    ) -> Result<Self, ConnectError> {
        use std::io::BufReader;
        use std::sync::Arc as StdArc;
        use tokio_rustls::rustls;

        let mut root_store = rustls::RootCertStore::empty();
        if let Some(ca_pem) = tls_ca_cert_pem {
            let certs: Vec<_> = rustls_pemfile::certs(&mut BufReader::new(ca_pem))
                .collect::<Result<Vec<_>, _>>()
                .map_err(|e| {
                    ConnectError::InvalidArgument(format!("failed to parse CA PEM: {e}"))
                })?;
            for cert in certs {
                root_store.add(cert).map_err(|e| {
                    ConnectError::InvalidArgument(format!("failed to add CA cert: {e}"))
                })?;
            }
        } else {
            for cert in rustls_native_certs::load_native_certs()
                .into_iter()
                .flatten()
            {
                let _ = root_store.add(cert);
            }
        }

        let config_builder = rustls::ClientConfig::builder().with_root_certificates(root_store);

        let config = if let (Some(cert_pem), Some(key_pem)) =
            (tls_client_cert_pem, tls_client_key_pem)
        {
            let certs: Vec<_> = rustls_pemfile::certs(&mut BufReader::new(cert_pem))
                .collect::<Result<Vec<_>, _>>()
                .map_err(|e| {
                    ConnectError::InvalidArgument(format!("failed to parse client cert PEM: {e}"))
                })?;
            let key = rustls_pemfile::private_key(&mut BufReader::new(key_pem))
                .map_err(|e| {
                    ConnectError::InvalidArgument(format!("failed to parse client key PEM: {e}"))
                })?
                .ok_or_else(|| {
                    ConnectError::InvalidArgument("no private key found in PEM".to_string())
                })?;
            config_builder
                .with_client_auth_cert(certs, key)
                .map_err(|e| {
                    ConnectError::InvalidArgument(format!("failed to build TLS config: {e}"))
                })?
        } else {
            config_builder.with_no_client_auth()
        };

        let connector = tokio_rustls::TlsConnector::from(StdArc::new(config));

        let host = addr.split(':').next().unwrap_or(addr);
        let server_name = rustls::pki_types::ServerName::try_from(host.to_string())
            .map_err(|e| ConnectError::InvalidArgument(format!("invalid server name: {e}")))?;

        let tcp_stream = TcpStream::connect(addr)
            .await
            .map_err(|e| ConnectError::InvalidArgument(format!("FIBP TLS connect failed: {e}")))?;

        let mut tls_stream = connector
            .connect(server_name, tcp_stream)
            .await
            .map_err(|e| ConnectError::InvalidArgument(format!("TLS handshake failed: {e}")))?;

        // FIBP handshake over TLS
        tls_stream.write_all(MAGIC).await.map_err(|e| {
            ConnectError::InvalidArgument(format!("FIBP handshake write failed: {e}"))
        })?;

        let mut server_magic = [0u8; 6];
        tls_stream
            .read_exact(&mut server_magic)
            .await
            .map_err(|e| {
                ConnectError::InvalidArgument(format!("FIBP handshake read failed: {e}"))
            })?;

        validate_server_magic(&server_magic)?;

        let codec = FibpCodec::new(MAX_FRAME_SIZE);
        let framed = Framed::new(tls_stream, codec);

        let config = TransportConnectConfig {
            api_key,
            tls_enabled: true,
            tls_ca_cert_pem: tls_ca_cert_pem.map(|b| b.to_vec()),
            tls_client_cert_pem: tls_client_cert_pem.map(|b| b.to_vec()),
            tls_client_key_pem: tls_client_key_pem.map(|b| b.to_vec()),
        };
        Self::from_framed(framed, config).await
    }

    /// Connect to a new address reusing this transport's TLS and API key config.
    ///
    /// Used when handling leader-hint redirects so the redirect connection
    /// inherits the same security settings as the original connection.
    async fn connect_with_config(&self, addr: &str) -> Result<Self, ConnectError> {
        let cfg = self.inner.connect_config.clone();
        if cfg.tls_enabled {
            Self::connect_tls(
                addr,
                cfg.api_key,
                cfg.tls_ca_cert_pem.as_deref(),
                cfg.tls_client_cert_pem.as_deref(),
                cfg.tls_client_key_pem.as_deref(),
            )
            .await
        } else {
            Self::connect(addr, cfg.api_key).await
        }
    }

    /// Create a transport from an already-handshaked framed stream.
    async fn from_framed<S>(
        framed: Framed<S, FibpCodec>,
        connect_config: TransportConnectConfig,
    ) -> Result<Self, ConnectError>
    where
        S: tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin + Send + 'static,
    {
        let pending: Arc<Mutex<HashMap<u32, PendingRequest>>> =
            Arc::new(Mutex::new(HashMap::new()));
        let consume_push_tx: Arc<Mutex<Option<ConsumePushSender>>> = Arc::new(Mutex::new(None));
        let consume_queue: Arc<Mutex<Option<String>>> = Arc::new(Mutex::new(None));

        let (writer_tx, writer_rx) = mpsc::channel::<WriterCommand>(256);

        spawn_io_loop(
            framed,
            writer_rx,
            Arc::clone(&pending),
            Arc::clone(&consume_push_tx),
            Arc::clone(&consume_queue),
        );

        let api_key = connect_config.api_key.clone();
        let transport = FibpTransport {
            inner: Arc::new(FibpTransportInner {
                writer_tx,
                pending,
                consume_push_tx,
                next_correlation_id: AtomicU32::new(1),
                consume_queue,
                connect_config,
            }),
        };

        // Authenticate if an API key is configured.
        if let Some(ref key) = api_key {
            transport.authenticate(key).await?;
        }

        Ok(transport)
    }

    fn next_id(&self) -> u32 {
        self.inner
            .next_correlation_id
            .fetch_add(1, Ordering::Relaxed)
    }

    /// Send a frame and wait for the response.
    async fn request(&self, frame: Frame) -> Result<Frame, StatusError> {
        let corr_id = frame.correlation_id;

        let (tx, rx) = oneshot::channel();
        {
            let mut pending = self.inner.pending.lock().await;
            pending.insert(corr_id, tx);
        }

        self.inner
            .writer_tx
            .send(WriterCommand::SendFrame(frame))
            .await
            .map_err(|_| StatusError::Internal("FIBP writer task has shut down".to_string()))?;

        let response = rx
            .await
            .map_err(|_| StatusError::Internal("FIBP response channel dropped".to_string()))??;

        if response.op == OP_ERROR {
            let msg = String::from_utf8_lossy(&response.payload).to_string();
            if msg.contains("permission denied") {
                return Err(StatusError::PermissionDenied(msg));
            }
            return Err(StatusError::Internal(msg));
        }

        if response.op == OP_GOAWAY {
            let msg = String::from_utf8_lossy(&response.payload).to_string();
            return Err(StatusError::Unavailable(format!(
                "server sent goaway: {msg}"
            )));
        }

        Ok(response)
    }

    /// Authenticate with the server via OP_AUTH.
    async fn authenticate(&self, api_key: &str) -> Result<(), ConnectError> {
        let corr_id = self.next_id();
        let frame = Frame::new(
            0,
            OP_AUTH,
            corr_id,
            Bytes::copy_from_slice(api_key.as_bytes()),
        );
        self.request(frame)
            .await
            .map_err(|e| ConnectError::InvalidArgument(format!("FIBP auth failed: {e}")))?;
        Ok(())
    }

    // -----------------------------------------------------------------------
    // Data operations
    // -----------------------------------------------------------------------

    /// Enqueue messages via FIBP.
    pub async fn enqueue_many(
        &self,
        messages: Vec<EnqueueMessage>,
    ) -> Vec<Result<String, EnqueueError>> {
        // Group messages by queue (FIBP enqueue request is per-queue).
        let mut by_queue: Vec<(String, Vec<(usize, EnqueueMessage)>)> = Vec::new();
        let mut queue_index: HashMap<String, usize> = HashMap::new();

        for (i, msg) in messages.iter().enumerate() {
            if let Some(&idx) = queue_index.get(&msg.queue) {
                by_queue[idx].1.push((i, msg.clone()));
            } else {
                queue_index.insert(msg.queue.clone(), by_queue.len());
                by_queue.push((msg.queue.clone(), vec![(i, msg.clone())]));
            }
        }

        let total = messages.len();
        let mut results: Vec<Option<Result<String, EnqueueError>>> =
            (0..total).map(|_| None).collect();

        for (queue, msgs) in by_queue {
            let wire_msgs: Vec<EnqueueWireMessage> = msgs
                .iter()
                .map(|(_, m)| EnqueueWireMessage {
                    headers: m.headers.clone(),
                    payload: m.payload.clone(),
                })
                .collect();

            let payload = fibp_codec::encode_enqueue_request(&queue, &wire_msgs);
            let corr_id = self.next_id();
            let frame = Frame::new(0, OP_ENQUEUE, corr_id, payload);

            match self.request(frame).await {
                Ok(response) => match fibp_codec::decode_enqueue_response(response.payload) {
                    Ok(items) => {
                        for (j, (original_idx, _)) in msgs.iter().enumerate() {
                            let result = match items.get(j) {
                                Some(EnqueueResultItem::Ok { msg_id }) => Ok(msg_id.clone()),
                                Some(EnqueueResultItem::Err { code, message }) => match *code {
                                    enqueue_err::QUEUE_NOT_FOUND => {
                                        Err(EnqueueError::QueueNotFound(message.clone()))
                                    }
                                    _ => Err(EnqueueError::Status(StatusError::Internal(
                                        message.clone(),
                                    ))),
                                },
                                None => Err(EnqueueError::Status(StatusError::Internal(
                                    "server returned fewer results than messages sent".to_string(),
                                ))),
                            };
                            results[*original_idx] = Some(result);
                        }
                    }
                    Err(e) => {
                        for (original_idx, _) in &msgs {
                            results[*original_idx] =
                                Some(Err(EnqueueError::Status(StatusError::Internal(format!(
                                    "failed to decode enqueue response: {e}"
                                )))));
                        }
                    }
                },
                Err(e) => {
                    for (original_idx, _) in &msgs {
                        let err = match &e {
                            StatusError::PermissionDenied(msg) => {
                                EnqueueError::PermissionDenied(msg.clone())
                            }
                            other => EnqueueError::Status(StatusError::Internal(format!(
                                "FIBP enqueue failed: {other}"
                            ))),
                        };
                        results[*original_idx] = Some(Err(err));
                    }
                }
            }
        }

        results
            .into_iter()
            .map(|r| {
                r.unwrap_or(Err(EnqueueError::Status(StatusError::Internal(
                    "missing result".to_string(),
                ))))
            })
            .collect()
    }

    /// Open a consume stream via FIBP.
    ///
    /// In cluster mode, if the connected node is not the leader for the
    /// requested queue, the server returns a `leader_hint:<addr>` error.
    /// This method transparently opens a new connection to the leader and
    /// returns a consume stream from that connection.
    pub async fn consume(
        &self,
        queue: &str,
    ) -> Result<mpsc::Receiver<Result<ConsumeMessage, StatusError>>, ConsumeError> {
        let payload = fibp_codec::encode_consume_request(queue, INITIAL_CREDITS);
        let corr_id = self.next_id();
        let frame = Frame::new(0, OP_CONSUME, corr_id, payload);

        match self.request(frame).await {
            Ok(_response) => {
                // Successfully opened consume on this node — set up push channel.
                self.setup_consume_channel(queue).await
            }
            Err(StatusError::Internal(ref msg)) if msg.starts_with("leader_hint:") => {
                // Server told us the leader address — connect there instead,
                // reusing the same TLS and API key configuration.
                let leader_addr = msg["leader_hint:".len()..].to_string();
                let leader_transport =
                    self.connect_with_config(&leader_addr).await.map_err(|e| {
                        ConsumeError::Status(StatusError::Internal(format!(
                            "failed to connect to leader at {leader_addr}: {e}"
                        )))
                    })?;

                // Send consume request to the leader (no recursion — single hop).
                let payload2 = fibp_codec::encode_consume_request(queue, INITIAL_CREDITS);
                let corr_id2 = leader_transport.next_id();
                let frame2 = Frame::new(0, OP_CONSUME, corr_id2, payload2);

                leader_transport
                    .request(frame2)
                    .await
                    .map_err(|e| match e {
                        StatusError::Internal(ref msg)
                            if msg.contains("not found") || msg.contains("queue") =>
                        {
                            ConsumeError::QueueNotFound(msg.clone())
                        }
                        other => ConsumeError::Status(other),
                    })?;

                leader_transport.setup_consume_channel(queue).await
            }
            Err(StatusError::Internal(ref msg))
                if msg.contains("not found") || msg.contains("queue") =>
            {
                Err(ConsumeError::QueueNotFound(msg.clone()))
            }
            Err(other) => Err(ConsumeError::Status(other)),
        }
    }

    /// Set up the consume push channel after a successful OP_CONSUME response.
    async fn setup_consume_channel(
        &self,
        queue: &str,
    ) -> Result<mpsc::Receiver<Result<ConsumeMessage, StatusError>>, ConsumeError> {
        let (push_tx, push_rx) = mpsc::channel::<Result<ConsumeMessage, StatusError>>(64);

        {
            let mut cpt = self.inner.consume_push_tx.lock().await;
            *cpt = Some(push_tx);
        }
        {
            let mut cq = self.inner.consume_queue.lock().await;
            *cq = Some(queue.to_string());
        }

        // Spawn a flow-control replenishment task.
        let writer_tx = self.inner.writer_tx.clone();
        let inner = Arc::clone(&self.inner);
        tokio::spawn(async move {
            let mut interval = tokio::time::interval(std::time::Duration::from_millis(100));
            loop {
                interval.tick().await;
                let corr = inner.next_correlation_id.fetch_add(1, Ordering::Relaxed);
                let flow_payload = fibp_codec::encode_flow(FLOW_CREDITS);
                let flow_frame = Frame::new(0, OP_FLOW, corr, flow_payload);
                if writer_tx
                    .send(WriterCommand::SendFrame(flow_frame))
                    .await
                    .is_err()
                {
                    break;
                }
            }
        });

        Ok(push_rx)
    }

    /// Acknowledge a message via FIBP.
    pub async fn ack(&self, queue: &str, message_id: &str) -> Result<(), AckError> {
        let payload = fibp_codec::encode_ack_request(&[(queue, message_id)]);
        let corr_id = self.next_id();
        let frame = Frame::new(0, OP_ACK, corr_id, payload);

        let response = self.request(frame).await.map_err(|e| match e {
            StatusError::PermissionDenied(msg) => AckError::PermissionDenied(msg),
            other => AckError::Status(other),
        })?;

        let results =
            fibp_codec::decode_ack_nack_response(response.payload).map_err(AckError::Status)?;

        match results.into_iter().next() {
            Some(AckNackResultItem::Ok) => Ok(()),
            Some(AckNackResultItem::Err { code, message }) => match code {
                ack_nack_err::MESSAGE_NOT_FOUND => Err(AckError::MessageNotFound(message)),
                _ => Err(AckError::Status(StatusError::Internal(message))),
            },
            None => Err(AckError::Status(StatusError::Internal(
                "empty ack response".to_string(),
            ))),
        }
    }

    /// Negatively acknowledge a message via FIBP.
    pub async fn nack(&self, queue: &str, message_id: &str, error: &str) -> Result<(), NackError> {
        let payload = fibp_codec::encode_nack_request(&[(queue, message_id, error)]);
        let corr_id = self.next_id();
        let frame = Frame::new(0, OP_NACK, corr_id, payload);

        let response = self.request(frame).await.map_err(|e| match e {
            StatusError::PermissionDenied(msg) => NackError::PermissionDenied(msg),
            other => NackError::Status(other),
        })?;

        let results =
            fibp_codec::decode_ack_nack_response(response.payload).map_err(NackError::Status)?;

        match results.into_iter().next() {
            Some(AckNackResultItem::Ok) => Ok(()),
            Some(AckNackResultItem::Err { code, message }) => match code {
                ack_nack_err::MESSAGE_NOT_FOUND => Err(NackError::MessageNotFound(message)),
                _ => Err(NackError::Status(StatusError::Internal(message))),
            },
            None => Err(NackError::Status(StatusError::Internal(
                "empty nack response".to_string(),
            ))),
        }
    }

    // -----------------------------------------------------------------------
    // Admin operations (protobuf-encoded payloads)
    // -----------------------------------------------------------------------

    /// Create a queue via FIBP.
    pub async fn create_queue(
        &self,
        name: &str,
        config: Option<fila_proto::QueueConfig>,
    ) -> Result<String, StatusError> {
        let req = fila_proto::CreateQueueRequest {
            name: name.to_string(),
            config,
        };
        let payload = Bytes::from(req.encode_to_vec());
        let corr_id = self.next_id();
        let frame = Frame::new(0, OP_CREATE_QUEUE, corr_id, payload);

        let response = self.request(frame).await?;
        let resp = fila_proto::CreateQueueResponse::decode(response.payload).map_err(|e| {
            StatusError::Internal(format!("failed to decode create queue response: {e}"))
        })?;
        Ok(resp.queue_id)
    }

    /// Delete a queue via FIBP.
    pub async fn delete_queue(&self, queue: &str) -> Result<(), StatusError> {
        let req = fila_proto::DeleteQueueRequest {
            queue: queue.to_string(),
        };
        let payload = Bytes::from(req.encode_to_vec());
        let corr_id = self.next_id();
        let frame = Frame::new(0, OP_DELETE_QUEUE, corr_id, payload);

        self.request(frame).await?;
        Ok(())
    }

    /// Get queue statistics via FIBP.
    pub async fn queue_stats(
        &self,
        queue: &str,
    ) -> Result<fila_proto::GetStatsResponse, StatusError> {
        let req = fila_proto::GetStatsRequest {
            queue: queue.to_string(),
        };
        let payload = Bytes::from(req.encode_to_vec());
        let corr_id = self.next_id();
        let frame = Frame::new(0, OP_QUEUE_STATS, corr_id, payload);

        let response = self.request(frame).await?;
        fila_proto::GetStatsResponse::decode(response.payload)
            .map_err(|e| StatusError::Internal(format!("failed to decode stats response: {e}")))
    }

    /// List queues via FIBP.
    pub async fn list_queues(&self) -> Result<fila_proto::ListQueuesResponse, StatusError> {
        let req = fila_proto::ListQueuesRequest {};
        let payload = Bytes::from(req.encode_to_vec());
        let corr_id = self.next_id();
        let frame = Frame::new(0, OP_LIST_QUEUES, corr_id, payload);

        let response = self.request(frame).await?;
        fila_proto::ListQueuesResponse::decode(response.payload).map_err(|e| {
            StatusError::Internal(format!("failed to decode list queues response: {e}"))
        })
    }

    /// Redrive messages from a DLQ via FIBP.
    pub async fn redrive(&self, dlq_queue: &str, count: u64) -> Result<u64, StatusError> {
        let req = fila_proto::RedriveRequest {
            dlq_queue: dlq_queue.to_string(),
            count,
        };
        let payload = Bytes::from(req.encode_to_vec());
        let corr_id = self.next_id();
        let frame = Frame::new(0, OP_REDRIVE, corr_id, payload);

        let response = self.request(frame).await?;
        let resp = fila_proto::RedriveResponse::decode(response.payload).map_err(|e| {
            StatusError::Internal(format!("failed to decode redrive response: {e}"))
        })?;
        Ok(resp.redriven)
    }

    // -----------------------------------------------------------------------
    // Config operations
    // -----------------------------------------------------------------------

    /// Set a runtime config value via FIBP.
    pub async fn set_config(&self, key: &str, value: &str) -> Result<(), StatusError> {
        let req = fila_proto::SetConfigRequest {
            key: key.to_string(),
            value: value.to_string(),
        };
        let payload = Bytes::from(req.encode_to_vec());
        let corr_id = self.next_id();
        let frame = Frame::new(0, OP_CONFIG_SET, corr_id, payload);

        self.request(frame).await?;
        Ok(())
    }

    /// Get a runtime config value via FIBP.
    pub async fn get_config(&self, key: &str) -> Result<String, StatusError> {
        let req = fila_proto::GetConfigRequest {
            key: key.to_string(),
        };
        let payload = Bytes::from(req.encode_to_vec());
        let corr_id = self.next_id();
        let frame = Frame::new(0, OP_CONFIG_GET, corr_id, payload);

        let response = self.request(frame).await?;
        let resp = fila_proto::GetConfigResponse::decode(response.payload).map_err(|e| {
            StatusError::Internal(format!("failed to decode get config response: {e}"))
        })?;
        Ok(resp.value)
    }

    /// List runtime config entries via FIBP.
    pub async fn list_config(
        &self,
        prefix: &str,
    ) -> Result<fila_proto::ListConfigResponse, StatusError> {
        let req = fila_proto::ListConfigRequest {
            prefix: prefix.to_string(),
        };
        let payload = Bytes::from(req.encode_to_vec());
        let corr_id = self.next_id();
        let frame = Frame::new(0, OP_CONFIG_LIST, corr_id, payload);

        let response = self.request(frame).await?;
        fila_proto::ListConfigResponse::decode(response.payload).map_err(|e| {
            StatusError::Internal(format!("failed to decode list config response: {e}"))
        })
    }

    // -----------------------------------------------------------------------
    // Auth CRUD operations
    // -----------------------------------------------------------------------

    /// Create a new API key via FIBP.
    pub async fn create_api_key(
        &self,
        name: &str,
        expires_at_ms: Option<u64>,
        is_superadmin: bool,
    ) -> Result<fila_proto::CreateApiKeyResponse, StatusError> {
        let req = fila_proto::CreateApiKeyRequest {
            name: name.to_string(),
            expires_at_ms: expires_at_ms.unwrap_or(0),
            is_superadmin,
        };
        let payload = Bytes::from(req.encode_to_vec());
        let corr_id = self.next_id();
        let frame = Frame::new(0, OP_AUTH_CREATE_KEY, corr_id, payload);

        let response = self.request(frame).await?;
        fila_proto::CreateApiKeyResponse::decode(response.payload).map_err(|e| {
            StatusError::Internal(format!("failed to decode create api key response: {e}"))
        })
    }

    /// Revoke an API key via FIBP.
    pub async fn revoke_api_key(&self, key_id: &str) -> Result<(), StatusError> {
        let req = fila_proto::RevokeApiKeyRequest {
            key_id: key_id.to_string(),
        };
        let payload = Bytes::from(req.encode_to_vec());
        let corr_id = self.next_id();
        let frame = Frame::new(0, OP_AUTH_REVOKE_KEY, corr_id, payload);

        self.request(frame).await?;
        Ok(())
    }

    /// List all API keys via FIBP.
    pub async fn list_api_keys(&self) -> Result<fila_proto::ListApiKeysResponse, StatusError> {
        let req = fila_proto::ListApiKeysRequest {};
        let payload = Bytes::from(req.encode_to_vec());
        let corr_id = self.next_id();
        let frame = Frame::new(0, OP_AUTH_LIST_KEYS, corr_id, payload);

        let response = self.request(frame).await?;
        fila_proto::ListApiKeysResponse::decode(response.payload).map_err(|e| {
            StatusError::Internal(format!("failed to decode list api keys response: {e}"))
        })
    }

    /// Set ACL permissions for an API key via FIBP.
    pub async fn set_acl(
        &self,
        key_id: &str,
        permissions: Vec<fila_proto::AclPermission>,
    ) -> Result<(), StatusError> {
        let req = fila_proto::SetAclRequest {
            key_id: key_id.to_string(),
            permissions,
        };
        let payload = Bytes::from(req.encode_to_vec());
        let corr_id = self.next_id();
        let frame = Frame::new(0, OP_AUTH_SET_ACL, corr_id, payload);

        self.request(frame).await?;
        Ok(())
    }

    /// Get ACL permissions for an API key via FIBP.
    pub async fn get_acl(&self, key_id: &str) -> Result<fila_proto::GetAclResponse, StatusError> {
        let req = fila_proto::GetAclRequest {
            key_id: key_id.to_string(),
        };
        let payload = Bytes::from(req.encode_to_vec());
        let corr_id = self.next_id();
        let frame = Frame::new(0, OP_AUTH_GET_ACL, corr_id, payload);

        let response = self.request(frame).await?;
        fila_proto::GetAclResponse::decode(response.payload)
            .map_err(|e| StatusError::Internal(format!("failed to decode get acl response: {e}")))
    }
}

// ---------------------------------------------------------------------------
// Server magic validation
// ---------------------------------------------------------------------------

fn validate_server_magic(server_magic: &[u8; 6]) -> Result<(), ConnectError> {
    if server_magic[..4] != MAGIC[..4] {
        return Err(ConnectError::InvalidArgument(
            "server returned invalid FIBP magic".to_string(),
        ));
    }
    if server_magic[4] != MAGIC[4] {
        return Err(ConnectError::InvalidArgument(format!(
            "server FIBP version mismatch: server={}.x, client={}.x",
            server_magic[4], MAGIC[4]
        )));
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Background I/O loop
// ---------------------------------------------------------------------------

/// Spawn the background I/O loop for a FIBP connection.
///
/// Uses `select!` to interleave reading server frames and writing client
/// frames in a single task, avoiding the need to split the Framed stream.
fn spawn_io_loop<S>(
    mut framed: Framed<S, FibpCodec>,
    mut writer_rx: mpsc::Receiver<WriterCommand>,
    pending: Arc<Mutex<HashMap<u32, PendingRequest>>>,
    consume_push_tx: Arc<Mutex<Option<ConsumePushSender>>>,
    consume_queue: Arc<Mutex<Option<String>>>,
) -> tokio::task::JoinHandle<()>
where
    S: tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin + Send + 'static,
{
    tokio::spawn(async move {
        let mut write_buf = BytesMut::new();
        let mut encoder = FibpCodec::new(MAX_FRAME_SIZE);

        loop {
            tokio::select! {
                biased;

                // Read frames from the server.
                frame_result = framed.next() => {
                    match frame_result {
                        Some(Ok(frame)) => {
                            dispatch_frame(
                                frame,
                                &pending,
                                &consume_push_tx,
                                &consume_queue,
                            )
                            .await;
                        }
                        Some(Err(e)) => {
                            fail_all_pending(
                                &pending,
                                &format!("FIBP connection error: {e}"),
                            )
                            .await;
                            break;
                        }
                        None => {
                            fail_all_pending(&pending, "FIBP connection closed").await;
                            break;
                        }
                    }
                }

                // Write frames to the server.
                cmd = writer_rx.recv() => {
                    match cmd {
                        Some(WriterCommand::SendFrame(frame)) => {
                            write_buf.clear();
                            if encoder.encode(frame, &mut write_buf).is_err() {
                                continue;
                            }
                            let stream = framed.get_mut();
                            if stream.write_all(&write_buf).await.is_err() {
                                break;
                            }
                            if stream.flush().await.is_err() {
                                break;
                            }
                        }
                        None => break,
                    }
                }
            }
        }
    })
}

/// Dispatch a received frame to the appropriate handler.
async fn dispatch_frame(
    frame: Frame,
    pending: &Mutex<HashMap<u32, PendingRequest>>,
    consume_push_tx: &Mutex<Option<mpsc::Sender<Result<ConsumeMessage, StatusError>>>>,
    consume_queue: &Mutex<Option<String>>,
) {
    // Stream push frames go to the consume channel.
    if frame.flags & FLAG_STREAM != 0 {
        let cpt = consume_push_tx.lock().await;
        if let Some(ref tx) = *cpt {
            let cq = consume_queue.lock().await;
            let queue_name = cq.clone().unwrap_or_default();
            drop(cq);

            match fibp_codec::decode_consume_push(frame.payload) {
                Ok(messages) => {
                    for msg in messages {
                        let cm = ConsumeMessage {
                            id: msg.msg_id,
                            headers: msg.headers,
                            payload: msg.payload,
                            fairness_key: msg.fairness_key,
                            attempt_count: msg.attempt_count,
                            queue: queue_name.clone(),
                        };
                        if tx.send(Ok(cm)).await.is_err() {
                            break;
                        }
                    }
                }
                Err(e) => {
                    let _ = tx.send(Err(e)).await;
                }
            }
        }
        return;
    }

    // Regular response frame — dispatch by correlation ID.
    let corr_id = frame.correlation_id;
    let mut map = pending.lock().await;
    if let Some(sender) = map.remove(&corr_id) {
        let _ = sender.send(Ok(frame));
    }
}

/// Fail all pending requests with an error message.
async fn fail_all_pending(pending: &Mutex<HashMap<u32, PendingRequest>>, msg: &str) {
    let mut map = pending.lock().await;
    let entries: Vec<_> = map.drain().collect();
    for (_, sender) in entries {
        let _ = sender.send(Err(StatusError::Unavailable(msg.to_string())));
    }
}
