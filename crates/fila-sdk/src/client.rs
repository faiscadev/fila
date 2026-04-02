use std::collections::HashMap;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::Arc;
use std::time::Duration;

use bytes::BytesMut;
use fila_fibp::{
    AckRequest as FibpAckRequest, AckResponse as FibpAckResponse, ConsumeRequest as FibpConsumeReq,
    DeliveryBatch, EnqueueMessage, EnqueueRequest as FibpEnqueueReq,
    EnqueueResponse as FibpEnqueueResp, ErrorCode, ErrorFrame, Handshake, HandshakeOk,
    NackRequest as FibpNackRequest, NackResponse as FibpNackResponse, Opcode, ProtocolMessage,
    RawFrame,
};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;
use tokio::sync::{mpsc, oneshot, Mutex};
use tokio_stream::Stream;
use tokio_util::sync::CancellationToken;

use crate::error::{
    ack_error_from_code, consume_error_from_code, enqueue_error_from_code, nack_error_from_code,
    AckError, ConnectError, ConsumeError, EnqueueError, NackError, StatusError,
};

/// Stream of consumed messages, returned by [`FilaClient::consume`].
///
/// When dropped, sends a `CancelConsume` frame to the server so it
/// unregisters the consumer immediately instead of waiting for EOF.
pub struct ConsumeStream {
    inner: std::pin::Pin<Box<dyn Stream<Item = Result<ConsumeMessage, StatusError>> + Send>>,
    request_id: u32,
    writer: Arc<Mutex<FrameWriter>>,
    state: Arc<Mutex<ConnectionInner>>,
}

impl Stream for ConsumeStream {
    type Item = Result<ConsumeMessage, StatusError>;

    fn poll_next(
        mut self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<Option<Self::Item>> {
        self.inner.as_mut().poll_next(cx)
    }
}

impl Drop for ConsumeStream {
    fn drop(&mut self) {
        let request_id = self.request_id;
        let writer = Arc::clone(&self.writer);
        let state = Arc::clone(&self.state);

        // Spawn a task to send CancelConsume and clean up state.
        // We can't do async work in Drop directly.
        tokio::spawn(async move {
            // Clear the delivery channel so the background reader stops
            // routing deliveries to the dropped receiver.
            {
                let mut guard = state.lock().await;
                guard.delivery_tx = None;
                guard.consume_request_id = None;
            }

            // Send CancelConsume to the server.
            let cancel = RawFrame {
                opcode: Opcode::CancelConsume as u8,
                flags: 0,
                request_id,
                payload: bytes::Bytes::new(),
            };
            let _ = writer.lock().await.send_frame(&cancel).await;
        });
    }
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
    /// When set, the key is sent in the Handshake frame during connection.
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
    /// When set, the key is sent during the handshake with the broker.
    pub fn with_api_key(mut self, api_key: impl Into<String>) -> Self {
        self.api_key = Some(api_key.into());
        self
    }
}

/// Strip URL scheme prefixes so addresses like "http://host:port" work.
fn normalize_addr(addr: &str) -> &str {
    addr.strip_prefix("http://")
        .or_else(|| addr.strip_prefix("https://"))
        .unwrap_or(addr)
}

/// IO stream abstraction for plain TCP and TLS.
///
/// Implements `AsyncRead + AsyncWrite` so it can be split via `tokio::io::split()`
/// into generic read/write halves without separate ReadStream/WriteStream enums.
enum IoStream {
    Plain(TcpStream),
    Tls(Box<tokio_rustls::client::TlsStream<TcpStream>>),
}

impl tokio::io::AsyncRead for IoStream {
    fn poll_read(
        self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
        buf: &mut tokio::io::ReadBuf<'_>,
    ) -> std::task::Poll<std::io::Result<()>> {
        match self.get_mut() {
            IoStream::Plain(s) => std::pin::Pin::new(s).poll_read(cx, buf),
            IoStream::Tls(s) => std::pin::Pin::new(s).poll_read(cx, buf),
        }
    }
}

impl tokio::io::AsyncWrite for IoStream {
    fn poll_write(
        self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
        buf: &[u8],
    ) -> std::task::Poll<std::io::Result<usize>> {
        match self.get_mut() {
            IoStream::Plain(s) => std::pin::Pin::new(s).poll_write(cx, buf),
            IoStream::Tls(s) => std::pin::Pin::new(s).poll_write(cx, buf),
        }
    }

    fn poll_flush(
        self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<std::io::Result<()>> {
        match self.get_mut() {
            IoStream::Plain(s) => std::pin::Pin::new(s).poll_flush(cx),
            IoStream::Tls(s) => std::pin::Pin::new(s).poll_flush(cx),
        }
    }

    fn poll_shutdown(
        self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<std::io::Result<()>> {
        match self.get_mut() {
            IoStream::Plain(s) => std::pin::Pin::new(s).poll_shutdown(cx),
            IoStream::Tls(s) => std::pin::Pin::new(s).poll_shutdown(cx),
        }
    }
}

/// Frame-level response from the background reader, dispatched to the waiting
/// request via its request_id.
enum ResponseFrame {
    /// A response frame matching a request_id.
    Response(RawFrame),
    /// The connection was closed or errored.
    Disconnected(String),
}

/// Shared state for the connection.
struct ConnectionInner {
    /// Pending request-response correlation: request_id -> oneshot sender.
    pending: HashMap<u32, oneshot::Sender<ResponseFrame>>,
    /// Channel for delivery messages received during consume.
    delivery_tx: Option<mpsc::Sender<Result<ConsumeMessage, StatusError>>>,
    /// The request_id used for the active consume subscription.
    consume_request_id: Option<u32>,
}

/// Shared internals of a [`FilaClient`]. A single `Arc<FilaClientInner>` is
/// shared by every clone — but NOT by the background reader task. When the
/// last client clone drops, [`Drop`] fires the cancellation token so the
/// background reader exits and sends a Disconnect frame.
struct FilaClientInner {
    writer: Arc<Mutex<FrameWriter>>,
    state: Arc<Mutex<ConnectionInner>>,
    next_request_id: AtomicU32,
    cancel: CancellationToken,
}

impl Drop for FilaClientInner {
    fn drop(&mut self) {
        self.cancel.cancel();
    }
}

/// Idiomatic Rust client for the Fila message broker.
///
/// Wraps the hot-path binary protocol operations: enqueue, consume, ack, nack.
/// The client is `Clone`, `Send`, and `Sync` — it can be shared across tasks.
#[derive(Clone)]
pub struct FilaClient {
    inner: Arc<FilaClientInner>,
    /// Stored connection options for transparent leader redirect reconnection.
    connect_options: Option<ConnectOptions>,
}

impl std::fmt::Debug for FilaClient {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("FilaClient")
            .field("addr", &self.connect_options.as_ref().map(|o| &o.addr))
            .finish()
    }
}

/// Write half of the connection.
struct FrameWriter {
    buf: BytesMut,
    stream: tokio::io::WriteHalf<IoStream>,
}

impl FrameWriter {
    async fn send_frame(&mut self, frame: &RawFrame) -> Result<(), std::io::Error> {
        self.buf.clear();
        frame.encode(&mut self.buf);
        self.stream.write_all(&self.buf).await?;
        self.stream.flush().await?;
        Ok(())
    }
}

impl FilaClient {
    /// Connect to a Fila broker at the given address.
    ///
    /// The address can include a scheme prefix (e.g. `http://localhost:5555`)
    /// which will be stripped, or be a plain `host:port`.
    pub async fn connect(addr: impl Into<String>) -> Result<Self, ConnectError> {
        let addr = addr.into();
        let options = ConnectOptions::new(addr);
        Self::connect_with_options(options).await
    }

    /// Connect to a Fila broker with custom options.
    ///
    /// When TLS fields are set in `options`, the connection uses TLS/mTLS.
    pub async fn connect_with_options(options: ConnectOptions) -> Result<Self, ConnectError> {
        let stored_options = options.clone();
        let host_port = normalize_addr(&options.addr).to_string();
        let timeout = options.timeout;

        // Validate partial mTLS: cert and key must both be provided or both absent.
        let has_cert = options.tls_client_cert_pem.is_some();
        let has_key = options.tls_client_key_pem.is_some();
        if has_cert != has_key {
            return Err(ConnectError::InvalidArgument(
                "tls_client_cert_pem and tls_client_key_pem must both be provided for mTLS"
                    .to_string(),
            ));
        }

        let tls_enabled = options.tls || options.tls_ca_cert_pem.is_some() || has_cert;

        // Establish TCP connection, applying timeout if configured.
        let tcp_future = TcpStream::connect(host_port.as_str());
        let tcp = if let Some(t) = timeout {
            tokio::time::timeout(t, tcp_future)
                .await
                .map_err(|_| ConnectError::TimedOut)?
                .map_err(ConnectError::Transport)?
        } else {
            tcp_future.await.map_err(ConnectError::Transport)?
        };

        let connect_future = async {
            let (io_stream, api_key) = if tls_enabled {
                let tls_stream = Self::tls_connect(tcp, &host_port, &options).await?;
                (IoStream::Tls(Box::new(tls_stream)), options.api_key)
            } else {
                (IoStream::Plain(tcp), options.api_key)
            };

            Self::setup_connection(io_stream, api_key, Some(stored_options)).await
        };

        // Apply timeout to TLS handshake + FIBP handshake as well.
        if let Some(t) = timeout {
            tokio::time::timeout(t, connect_future)
                .await
                .map_err(|_| ConnectError::TimedOut)?
        } else {
            connect_future.await
        }
    }

    /// Perform TLS handshake on an established TCP connection.
    async fn tls_connect(
        tcp: TcpStream,
        host_port: &str,
        options: &ConnectOptions,
    ) -> Result<tokio_rustls::client::TlsStream<TcpStream>, ConnectError> {
        use rustls::pki_types::ServerName;

        let mut root_store = rustls::RootCertStore::empty();

        if let Some(ref ca_pem) = options.tls_ca_cert_pem {
            let certs: Vec<_> = rustls_pemfile::certs(&mut std::io::Cursor::new(ca_pem))
                .collect::<Result<_, _>>()
                .map_err(|e| {
                    ConnectError::TlsConfig(format!("failed to parse CA certificate: {e}"))
                })?;
            for cert in certs {
                root_store.add(cert).map_err(|e| {
                    ConnectError::TlsConfig(format!("failed to add CA certificate: {e}"))
                })?;
            }
        } else {
            // No custom CA provided — use the Mozilla root certificates so
            // server certs signed by public CAs are trusted out of the box.
            root_store.extend(webpki_roots::TLS_SERVER_ROOTS.iter().cloned());
        }

        let tls_config = if let (Some(ref cert_pem), Some(ref key_pem)) =
            (&options.tls_client_cert_pem, &options.tls_client_key_pem)
        {
            let certs: Vec<_> = rustls_pemfile::certs(&mut std::io::Cursor::new(cert_pem))
                .collect::<Result<_, _>>()
                .map_err(|e| {
                    ConnectError::TlsConfig(format!("failed to parse client cert: {e}"))
                })?;
            let key = rustls_pemfile::private_key(&mut std::io::Cursor::new(key_pem))
                .map_err(|e| ConnectError::TlsConfig(format!("failed to parse client key: {e}")))?
                .ok_or_else(|| {
                    ConnectError::TlsConfig("no private key found in client key PEM".to_string())
                })?;
            rustls::ClientConfig::builder()
                .with_root_certificates(root_store)
                .with_client_auth_cert(certs, key)
                .map_err(|e| {
                    ConnectError::TlsConfig(format!("failed to configure client auth: {e}"))
                })?
        } else {
            rustls::ClientConfig::builder()
                .with_root_certificates(root_store)
                .with_no_client_auth()
        };

        let connector = tokio_rustls::TlsConnector::from(Arc::new(tls_config));

        // Extract hostname from host:port for SNI.
        let hostname = host_port.split(':').next().unwrap_or("localhost");
        let server_name = ServerName::try_from(hostname.to_string())
            .map_err(|e| ConnectError::TlsConfig(format!("invalid server name: {e}")))?;

        connector
            .connect(server_name, tcp)
            .await
            .map_err(ConnectError::Transport)
    }

    /// Set up the connection: perform handshake, split stream, start reader.
    async fn setup_connection(
        stream: IoStream,
        api_key: Option<String>,
        connect_options: Option<ConnectOptions>,
    ) -> Result<Self, ConnectError> {
        let (reader, writer) = tokio::io::split(stream);
        Self::handshake_and_start(reader, writer, api_key, connect_options).await
    }

    /// Perform the FIBP handshake then start the background reader task.
    async fn handshake_and_start(
        mut read: tokio::io::ReadHalf<IoStream>,
        mut write_stream: tokio::io::WriteHalf<IoStream>,
        api_key: Option<String>,
        connect_options: Option<ConnectOptions>,
    ) -> Result<Self, ConnectError> {
        let handshake = Handshake {
            protocol_version: 1,
            api_key,
        };
        let frame = handshake.encode(0);
        let mut write_buf = BytesMut::new();
        frame.encode(&mut write_buf);
        write_stream
            .write_all(&write_buf)
            .await
            .map_err(ConnectError::Transport)?;
        write_stream
            .flush()
            .await
            .map_err(ConnectError::Transport)?;

        // Read handshake response.
        let response_frame = read_one_frame(&mut read)
            .await
            .map_err(ConnectError::Transport)?
            .ok_or_else(|| {
                ConnectError::HandshakeFailed("connection closed during handshake".to_string())
            })?;

        let opcode = Opcode::from_u8(response_frame.opcode);
        match opcode {
            Some(Opcode::HandshakeOk) => {
                let _handshake_ok = HandshakeOk::decode(response_frame.payload)
                    .map_err(|e| ConnectError::HandshakeFailed(format!("decode error: {e}")))?;
            }
            Some(Opcode::Error) => {
                let err = ErrorFrame::decode(response_frame.payload)
                    .map_err(|e| ConnectError::HandshakeFailed(format!("decode error: {e}")))?;
                return Err(ConnectError::HandshakeFailed(format!(
                    "{:?}: {}",
                    err.error_code, err.message
                )));
            }
            _ => {
                return Err(ConnectError::HandshakeFailed(format!(
                    "unexpected opcode: 0x{:02x}",
                    response_frame.opcode
                )));
            }
        }

        // Set up shared state and start background reader.
        let cancel = CancellationToken::new();
        let state = Arc::new(Mutex::new(ConnectionInner {
            pending: HashMap::new(),
            delivery_tx: None,
            consume_request_id: None,
        }));

        let writer = Arc::new(Mutex::new(FrameWriter {
            buf: BytesMut::with_capacity(1024),
            stream: write_stream,
        }));

        let inner = Arc::new(FilaClientInner {
            writer: Arc::clone(&writer),
            state: Arc::clone(&state),
            next_request_id: AtomicU32::new(1),
            cancel: cancel.clone(),
        });

        // The background reader holds its own Arcs to state and writer, plus
        // the cancel token — NOT an Arc<FilaClientInner>. This way, when all
        // FilaClient clones drop, FilaClientInner::drop fires and cancels the
        // token even though the reader task is still alive.
        tokio::spawn(async move {
            background_reader(read, state, cancel, writer).await;
        });

        Ok(Self {
            inner,
            connect_options,
        })
    }

    /// Allocate a unique request ID and register a response channel.
    async fn start_request(&self) -> (u32, oneshot::Receiver<ResponseFrame>) {
        let request_id = self.inner.next_request_id.fetch_add(1, Ordering::Relaxed);
        let (tx, rx) = oneshot::channel();
        self.inner.state.lock().await.pending.insert(request_id, tx);
        (request_id, rx)
    }

    /// Send a frame and wait for the response.
    async fn send_and_recv(&self, frame: RawFrame) -> Result<RawFrame, StatusError> {
        let request_id = frame.request_id;
        let rx = {
            let (tx, rx) = oneshot::channel();
            self.inner.state.lock().await.pending.insert(request_id, tx);
            rx
        };

        // Send the frame.
        self.inner
            .writer
            .lock()
            .await
            .send_frame(&frame)
            .await
            .map_err(StatusError::Transport)?;

        // Wait for response.
        match rx.await {
            Ok(ResponseFrame::Response(resp)) => Ok(resp),
            Ok(ResponseFrame::Disconnected(msg)) => {
                Err(StatusError::Unavailable(format!("connection lost: {msg}")))
            }
            Err(_) => Err(StatusError::Unavailable("connection closed".to_string())),
        }
    }

    /// Check if a response frame is an Error frame and convert it.
    fn check_error(frame: &RawFrame) -> Option<(ErrorCode, String, HashMap<String, String>)> {
        if Opcode::from_u8(frame.opcode) == Some(Opcode::Error) {
            if let Ok(err) = ErrorFrame::decode(frame.payload.clone()) {
                return Some((err.error_code, err.message, err.metadata));
            }
        }
        None
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
        let (request_id, _) = self.start_request().await;

        let req = FibpEnqueueReq {
            messages: vec![EnqueueMessage {
                queue: queue.to_string(),
                headers,
                payload: payload.into(),
            }],
        };

        let resp = self
            .send_and_recv(req.encode(request_id))
            .await
            .map_err(EnqueueError::Status)?;

        if let Some((code, message, _)) = Self::check_error(&resp) {
            return Err(enqueue_error_from_code(code, message));
        }

        let enqueue_resp = FibpEnqueueResp::decode(resp.payload).map_err(|e| {
            EnqueueError::Status(StatusError::Protocol(format!(
                "failed to decode enqueue response: {e}"
            )))
        })?;

        if enqueue_resp.results.is_empty() {
            return Err(EnqueueError::Status(StatusError::Protocol(
                "empty enqueue response".to_string(),
            )));
        }

        let item = &enqueue_resp.results[0];
        if item.error_code != ErrorCode::Ok {
            return Err(enqueue_error_from_code(
                item.error_code,
                item.message_id.clone(),
            ));
        }

        Ok(item.message_id.clone())
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
    pub async fn consume(&self, queue: &str) -> Result<ConsumeStream, ConsumeError> {
        let (request_id, rx) = self.start_request().await;

        let req = FibpConsumeReq {
            queue: queue.to_string(),
        };

        // Send consume frame.
        self.inner
            .writer
            .lock()
            .await
            .send_frame(&req.encode(request_id))
            .await
            .map_err(|e| ConsumeError::Status(StatusError::Transport(e)))?;

        // Wait for the initial response which may be an Error (queue not found,
        // not leader, etc.) or ConsumeOk.
        let initial = match rx.await {
            Ok(ResponseFrame::Response(frame)) => frame,
            Ok(ResponseFrame::Disconnected(msg)) => {
                return Err(ConsumeError::Status(StatusError::Unavailable(format!(
                    "connection lost: {msg}"
                ))));
            }
            Err(_) => {
                return Err(ConsumeError::Status(StatusError::Unavailable(
                    "connection closed".to_string(),
                )));
            }
        };

        // Check for error response.
        if let Some((code, message, metadata)) = Self::check_error(&initial) {
            if code == ErrorCode::NotLeader {
                if let Some(leader_addr) = metadata.get("leader_addr") {
                    return self.redirect_consume(leader_addr, queue).await;
                }
            }
            return Err(consume_error_from_code(code, message));
        }

        // The initial frame should be a ConsumeOk confirmation.
        match Opcode::from_u8(initial.opcode) {
            Some(Opcode::ConsumeOk) => {
                // Subscription confirmed. The server will now push Delivery frames.
            }
            other => {
                return Err(ConsumeError::Status(StatusError::Protocol(format!(
                    "expected ConsumeOk, got opcode: {:?} (0x{:02x})",
                    other, initial.opcode
                ))));
            }
        }

        // Register the delivery channel AFTER ConsumeOk succeeds. This avoids
        // overwriting a previous active consumer's channel on failure/redirect
        // paths. The race with early deliveries is minimal: the server sends
        // ConsumeOk before any Delivery frames, so by the time deliveries
        // arrive we've registered the channel.
        let (delivery_tx, delivery_rx) = mpsc::channel(256);
        {
            let mut state = self.inner.state.lock().await;
            state.delivery_tx = Some(delivery_tx);
            state.consume_request_id = Some(request_id);
        }

        Ok(ConsumeStream {
            inner: Box::pin(tokio_stream::wrappers::ReceiverStream::new(delivery_rx)),
            request_id,
            writer: Arc::clone(&self.inner.writer),
            state: Arc::clone(&self.inner.state),
        })
    }

    /// Handle transparent leader redirect for consume.
    fn redirect_consume(
        &self,
        leader_addr: &str,
        queue: &str,
    ) -> std::pin::Pin<
        Box<dyn std::future::Future<Output = Result<ConsumeStream, ConsumeError>> + Send + '_>,
    > {
        let leader_addr = leader_addr.to_string();
        let queue = queue.to_string();
        Box::pin(async move {
            let leader_opts = match self.connect_options {
                Some(ref opts) => ConnectOptions {
                    addr: leader_addr.clone(),
                    timeout: opts.timeout,
                    tls: opts.tls,
                    tls_ca_cert_pem: opts.tls_ca_cert_pem.clone(),
                    tls_client_cert_pem: opts.tls_client_cert_pem.clone(),
                    tls_client_key_pem: opts.tls_client_key_pem.clone(),
                    api_key: opts.api_key.clone(),
                },
                None => ConnectOptions::new(&leader_addr),
            };
            match Self::connect_with_options(leader_opts).await {
                Ok(leader_client) => leader_client.consume(&queue).await,
                Err(_) => Err(ConsumeError::Status(StatusError::Unavailable(format!(
                    "failed to connect to leader at {leader_addr}"
                )))),
            }
        })
    }

    /// Acknowledge a successfully processed message.
    ///
    /// The message is permanently removed from the queue.
    pub async fn ack(&self, queue: &str, message_id: &str) -> Result<(), AckError> {
        let (request_id, _) = self.start_request().await;

        let req = FibpAckRequest {
            items: vec![fila_fibp::AckItem {
                queue: queue.to_string(),
                message_id: message_id.to_string(),
            }],
        };

        let resp = self
            .send_and_recv(req.encode(request_id))
            .await
            .map_err(AckError::Status)?;

        if let Some((code, message, _)) = Self::check_error(&resp) {
            return Err(ack_error_from_code(code, message));
        }

        let ack_resp = FibpAckResponse::decode(resp.payload).map_err(|e| {
            AckError::Status(StatusError::Protocol(format!(
                "failed to decode ack response: {e}"
            )))
        })?;

        if ack_resp.results.is_empty() {
            return Err(AckError::Status(StatusError::Protocol(
                "empty ack response".to_string(),
            )));
        }

        let item = &ack_resp.results[0];
        if item.error_code != ErrorCode::Ok {
            return Err(ack_error_from_code(
                item.error_code,
                format!("{:?}", item.error_code),
            ));
        }

        Ok(())
    }

    /// Negatively acknowledge a message that failed processing.
    ///
    /// The message is requeued for retry or routed to the dead-letter queue
    /// based on the queue's on_failure Lua hook configuration.
    pub async fn nack(&self, queue: &str, message_id: &str, error: &str) -> Result<(), NackError> {
        let (request_id, _) = self.start_request().await;

        let req = FibpNackRequest {
            items: vec![fila_fibp::NackItem {
                queue: queue.to_string(),
                message_id: message_id.to_string(),
                error: error.to_string(),
            }],
        };

        let resp = self
            .send_and_recv(req.encode(request_id))
            .await
            .map_err(NackError::Status)?;

        if let Some((code, message, _)) = Self::check_error(&resp) {
            return Err(nack_error_from_code(code, message));
        }

        let nack_resp = FibpNackResponse::decode(resp.payload).map_err(|e| {
            NackError::Status(StatusError::Protocol(format!(
                "failed to decode nack response: {e}"
            )))
        })?;

        if nack_resp.results.is_empty() {
            return Err(NackError::Status(StatusError::Protocol(
                "empty nack response".to_string(),
            )));
        }

        let item = &nack_resp.results[0];
        if item.error_code != ErrorCode::Ok {
            return Err(nack_error_from_code(
                item.error_code,
                format!("{:?}", item.error_code),
            ));
        }

        Ok(())
    }
}

/// Read a single frame from the stream.
async fn read_one_frame(
    stream: &mut tokio::io::ReadHalf<IoStream>,
) -> Result<Option<RawFrame>, std::io::Error> {
    let mut buf = BytesMut::with_capacity(4096);
    loop {
        match RawFrame::decode(&mut buf) {
            Ok(Some(frame)) => return Ok(Some(frame)),
            Ok(None) => {}
            Err(e) => {
                return Err(std::io::Error::new(std::io::ErrorKind::InvalidData, e));
            }
        }

        let n = stream.read_buf(&mut buf).await?;
        if n == 0 {
            return Ok(None);
        }
    }
}

/// Background task that reads frames from the connection and dispatches them.
///
/// Exits when the connection closes, a read error occurs, or the cancellation
/// token is triggered (i.e. all `FilaClient` clones have been dropped).
async fn background_reader(
    mut stream: tokio::io::ReadHalf<IoStream>,
    state: Arc<Mutex<ConnectionInner>>,
    cancel: CancellationToken,
    writer: Arc<Mutex<FrameWriter>>,
) {
    let mut buf = BytesMut::with_capacity(4096);

    loop {
        loop {
            match RawFrame::decode(&mut buf) {
                Ok(Some(frame)) => {
                    dispatch_frame(frame, &state).await;
                }
                Ok(None) => break,
                Err(e) => {
                    notify_all_disconnected(&state, &format!("frame decode error: {e}")).await;
                    return;
                }
            }
        }

        tokio::select! {
            result = stream.read_buf(&mut buf) => {
                match result {
                    Ok(0) => {
                        notify_all_disconnected(&state, "connection closed by server").await;
                        return;
                    }
                    Ok(_) => {}
                    Err(e) => {
                        notify_all_disconnected(&state, &format!("read error: {e}")).await;
                        return;
                    }
                }
            }
            _ = cancel.cancelled() => {
                // Send a Disconnect frame so the server cleans up immediately
                // instead of waiting for TCP EOF detection.
                let disconnect = RawFrame {
                    opcode: Opcode::Disconnect as u8,
                    flags: 0,
                    request_id: 0,
                    payload: bytes::Bytes::new(),
                };
                let _ = writer.lock().await.send_frame(&disconnect).await;
                return;
            }
        }
    }
}

/// Dispatch a received frame to the appropriate handler.
async fn dispatch_frame(frame: RawFrame, state: &Mutex<ConnectionInner>) {
    let opcode = Opcode::from_u8(frame.opcode);
    let request_id = frame.request_id;

    match opcode {
        Some(Opcode::Delivery) => {
            // Clone the sender and drop the lock before awaiting sends.
            // Holding the lock while awaiting channel sends can deadlock:
            // if the channel is full, consumers need to call ack/nack which
            // also acquires the lock.
            let tx = {
                let guard = state.lock().await;
                guard.delivery_tx.clone()
            };
            if let Some(tx) = tx {
                match DeliveryBatch::decode(frame.payload) {
                    Ok(batch) => {
                        for msg in batch.messages {
                            let consume_msg = ConsumeMessage {
                                id: msg.message_id,
                                headers: msg.headers,
                                payload: msg.payload,
                                fairness_key: msg.fairness_key,
                                attempt_count: msg.attempt_count,
                                queue: msg.queue,
                            };
                            let _ = tx.send(Ok(consume_msg)).await;
                        }
                    }
                    Err(e) => {
                        let _ = tx
                            .send(Err(StatusError::Protocol(format!(
                                "delivery decode error: {e}"
                            ))))
                            .await;
                    }
                }
            }
        }
        Some(Opcode::Pong) => {
            // Pong responses are silently consumed.
        }
        _ => {
            // Route response to the pending request by request_id.
            let mut guard = state.lock().await;

            // If this is the consume request_id and it's an Error or initial Delivery,
            // it should go through the oneshot (for the initial consume response).
            if let Some(tx) = guard.pending.remove(&request_id) {
                let _ = tx.send(ResponseFrame::Response(frame));
            }
            // If not found in pending, it might be an unsolicited frame — ignore.
        }
    }
}

/// Notify all pending requests that the connection has been lost.
async fn notify_all_disconnected(state: &Mutex<ConnectionInner>, msg: &str) {
    let mut guard = state.lock().await;
    for (_, tx) in guard.pending.drain() {
        let _ = tx.send(ResponseFrame::Disconnected(msg.to_string()));
    }
    if let Some(ref tx) = guard.delivery_tx {
        let _ = tx
            .send(Err(StatusError::Unavailable(format!(
                "connection lost: {msg}"
            ))))
            .await;
    }
}
