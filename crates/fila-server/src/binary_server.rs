//! Binary protocol server — accepts TCP connections and dispatches FIBP frames
//! to the broker's scheduler through the same command channel as gRPC.

use std::collections::HashMap;
use std::sync::Arc;

use bytes::BytesMut;
use fila_core::{Broker, BrokerError, ClusterHandle, TlsParams};
use fila_fibp::{
    ErrorCode, ErrorFrame, FrameError, Handshake, HandshakeOk, Opcode, RawFrame, MAX_FRAME_SIZE,
};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tracing::{debug, error, info};

use crate::binary_handlers;

/// State shared across all connections.
pub struct BinaryServer {
    broker: Arc<Broker>,
    cluster: Option<Arc<ClusterHandle>>,
    node_id: u64,
}

impl BinaryServer {
    pub fn new(broker: Arc<Broker>, cluster: Option<Arc<ClusterHandle>>, node_id: u64) -> Self {
        Self {
            broker,
            cluster,
            node_id,
        }
    }
}

/// Start the binary protocol listener. Returns a future that runs until
/// the shutdown signal fires.
pub async fn run(
    server: Arc<BinaryServer>,
    listener: TcpListener,
    tls_acceptor: Option<tokio_rustls::TlsAcceptor>,
    mut shutdown: tokio::sync::watch::Receiver<bool>,
) {
    info!(
        addr = %listener.local_addr().unwrap(),
        tls = tls_acceptor.is_some(),
        "binary protocol server listening"
    );

    let mut connections = tokio::task::JoinSet::new();

    loop {
        tokio::select! {
            accept_result = listener.accept() => {
                match accept_result {
                    Ok((stream, peer)) => {
                        debug!(%peer, "new TCP connection");
                        let server = Arc::clone(&server);
                        let tls = tls_acceptor.clone();
                        connections.spawn(async move {
                            if let Err(e) = handle_connection(server, stream, tls).await {
                                debug!(%peer, error = %e, "connection ended");
                            }
                        });
                    }
                    Err(e) => {
                        error!(error = %e, "accept failed");
                    }
                }
            }
            // Reap completed connection tasks to prevent unbounded JoinHandle growth.
            Some(_) = connections.join_next(), if !connections.is_empty() => {}
            _ = shutdown.changed() => {
                info!(
                    active_connections = connections.len(),
                    "binary protocol server shutting down"
                );
                connections.abort_all();
                while connections.join_next().await.is_some() {}
                return;
            }
        }
    }
}

/// Async stream abstraction over plain TCP and TLS connections.
///
/// Implements `AsyncRead + AsyncWrite` so all standard tokio IO extension
/// methods (`read_buf`, `write_all`, `flush`) work directly.
enum Stream {
    Plain(TcpStream),
    Tls(Box<tokio_rustls::server::TlsStream<TcpStream>>),
}

impl tokio::io::AsyncRead for Stream {
    fn poll_read(
        self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
        buf: &mut tokio::io::ReadBuf<'_>,
    ) -> std::task::Poll<std::io::Result<()>> {
        match self.get_mut() {
            Stream::Plain(s) => std::pin::Pin::new(s).poll_read(cx, buf),
            Stream::Tls(s) => std::pin::Pin::new(s).poll_read(cx, buf),
        }
    }
}

impl tokio::io::AsyncWrite for Stream {
    fn poll_write(
        self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
        buf: &[u8],
    ) -> std::task::Poll<std::io::Result<usize>> {
        match self.get_mut() {
            Stream::Plain(s) => std::pin::Pin::new(s).poll_write(cx, buf),
            Stream::Tls(s) => std::pin::Pin::new(s).poll_write(cx, buf),
        }
    }

    fn poll_flush(
        self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<std::io::Result<()>> {
        match self.get_mut() {
            Stream::Plain(s) => std::pin::Pin::new(s).poll_flush(cx),
            Stream::Tls(s) => std::pin::Pin::new(s).poll_flush(cx),
        }
    }

    fn poll_shutdown(
        self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<std::io::Result<()>> {
        match self.get_mut() {
            Stream::Plain(s) => std::pin::Pin::new(s).poll_shutdown(cx),
            Stream::Tls(s) => std::pin::Pin::new(s).poll_shutdown(cx),
        }
    }
}

/// Handle a single TCP connection — TLS handshake, protocol handshake,
/// then frame loop.
async fn handle_connection(
    server: Arc<BinaryServer>,
    tcp: TcpStream,
    tls_acceptor: Option<tokio_rustls::TlsAcceptor>,
) -> Result<(), ConnectionError> {
    // Optional TLS — type-erase to DynStream
    let mut stream = match tls_acceptor {
        Some(acceptor) => {
            let tls_stream = acceptor.accept(tcp).await.map_err(ConnectionError::Tls)?;
            Stream::Tls(Box::new(tls_stream))
        }
        None => Stream::Plain(tcp),
    };

    let mut read_buf = BytesMut::with_capacity(8192);

    // --- Protocol handshake ---
    let api_key = protocol_handshake(&server, &mut stream, &mut read_buf).await?;

    // Validate API key if auth is enabled
    let caller_key: Option<fila_core::broker::auth::CallerKey> = if server.broker.auth_enabled {
        match api_key {
            Some(ref key) => match server.broker.validate_api_key(key) {
                Ok(Some(caller)) => Some(caller),
                Ok(None) | Err(_) => {
                    send_error(
                        &mut stream,
                        0,
                        ErrorCode::Unauthorized,
                        "invalid API key",
                        &HashMap::new(),
                    )
                    .await?;
                    return Ok(());
                }
            },
            None => {
                send_error(
                    &mut stream,
                    0,
                    ErrorCode::Unauthorized,
                    "API key required",
                    &HashMap::new(),
                )
                .await?;
                return Ok(());
            }
        }
    } else {
        None
    };

    // --- Frame processing loop ---
    let mut conn = ConnectionState::new(server, stream, read_buf, caller_key);
    conn.frame_loop().await
}

/// Perform the protocol handshake: read Handshake, send HandshakeOk.
async fn protocol_handshake(
    server: &BinaryServer,
    stream: &mut Stream,
    buf: &mut BytesMut,
) -> Result<Option<String>, ConnectionError> {
    // Read until we have a complete frame
    let frame = read_frame(stream, buf).await?;

    let opcode = Opcode::from_u8(frame.opcode);
    if opcode != Some(Opcode::Handshake) {
        send_error(
            stream,
            frame.request_id,
            ErrorCode::InvalidFrame,
            "expected Handshake frame",
            &HashMap::new(),
        )
        .await?;
        return Err(ConnectionError::Protocol("expected Handshake".into()));
    }

    let hs = Handshake::decode(frame.payload)
        .map_err(|e| ConnectionError::Protocol(format!("invalid handshake: {e}")))?;

    if hs.protocol_version != 1 {
        let mut meta = HashMap::new();
        meta.insert("max_version".to_string(), "1".to_string());
        send_error(
            stream,
            0,
            ErrorCode::UnsupportedVersion,
            "only protocol version 1 is supported",
            &meta,
        )
        .await?;
        return Err(ConnectionError::Protocol("unsupported version".into()));
    }

    let ok = HandshakeOk {
        negotiated_version: 1,
        node_id: server.node_id,
        max_frame_size: MAX_FRAME_SIZE,
    };
    send_frame(stream, &ok.encode(0)).await?;

    Ok(hs.api_key)
}

/// Per-connection state including active consume subscriptions.
struct ConnectionState {
    server: Arc<BinaryServer>,
    stream: Stream,
    read_buf: BytesMut,
    caller_key: Option<fila_core::broker::auth::CallerKey>,
    /// Active consume subscriptions: request_id -> consumer_id
    consumers: HashMap<u32, String>,
    /// Channel for delivery frames from consume tasks → frame loop writer.
    delivery_tx: tokio::sync::mpsc::Sender<RawFrame>,
    delivery_rx: tokio::sync::mpsc::Receiver<RawFrame>,
}

impl ConnectionState {
    fn new(
        server: Arc<BinaryServer>,
        stream: Stream,
        read_buf: BytesMut,
        caller_key: Option<fila_core::broker::auth::CallerKey>,
    ) -> Self {
        let (delivery_tx, delivery_rx) = tokio::sync::mpsc::channel(256);
        Self {
            server,
            stream,
            read_buf,
            caller_key,
            consumers: HashMap::new(),
            delivery_tx,
            delivery_rx,
        }
    }

    async fn frame_loop(&mut self) -> Result<(), ConnectionError> {
        loop {
            // Try to decode a frame from the read buffer first (no IO needed).
            if let Some(frame) =
                RawFrame::decode(&mut self.read_buf).map_err(ConnectionError::Frame)?
            {
                if self.dispatch_frame(frame).await? {
                    break;
                }
                continue;
            }

            // Multiplex: read from TCP or receive a delivery frame to send.
            tokio::select! {
                read_result = self.stream.read_buf(&mut self.read_buf) => {
                    let n = read_result.map_err(ConnectionError::Io)?;
                    if n == 0 {
                        if self.read_buf.is_empty() {
                            debug!("client disconnected");
                            break;
                        }
                        return Err(ConnectionError::Io(std::io::Error::new(
                            std::io::ErrorKind::UnexpectedEof,
                            "connection closed mid-frame",
                        )));
                    }
                }
                delivery = self.delivery_rx.recv() => {
                    match delivery {
                        Some(frame) => {
                            send_frame(&mut self.stream, &frame).await?;
                        }
                        None => unreachable!("delivery_tx is held by ConnectionState"),
                    }
                }
            }
        }

        // Cleanup: unregister all active consumers
        for (_, consumer_id) in self.consumers.drain() {
            let _ = self
                .server
                .broker
                .send_command(fila_core::SchedulerCommand::UnregisterConsumer { consumer_id });
        }

        Ok(())
    }

    /// Dispatch a decoded frame. Returns true if the connection should close.
    async fn dispatch_frame(&mut self, frame: RawFrame) -> Result<bool, ConnectionError> {
        let opcode = Opcode::from_u8(frame.opcode);
        match opcode {
            Some(Opcode::Ping) => {
                let pong = RawFrame {
                    opcode: Opcode::Pong as u8,
                    flags: 0,
                    request_id: frame.request_id,
                    payload: bytes::Bytes::new(),
                };
                send_frame(&mut self.stream, &pong).await?;
            }
            Some(Opcode::Disconnect) => {
                debug!("client sent Disconnect");
                return Ok(true);
            }
            Some(Opcode::Enqueue) => self.handle_enqueue(frame).await?,
            Some(Opcode::Consume) => self.handle_consume(frame).await?,
            Some(Opcode::CancelConsume) => self.handle_cancel_consume(frame).await?,
            Some(Opcode::Ack) => self.handle_ack(frame).await?,
            Some(Opcode::Nack) => self.handle_nack(frame).await?,
            _ => {
                send_error(
                    &mut self.stream,
                    frame.request_id,
                    ErrorCode::InvalidFrame,
                    &format!("unknown or unsupported opcode: 0x{:02x}", frame.opcode),
                    &HashMap::new(),
                )
                .await?;
            }
        }
        Ok(false)
    }

    async fn handle_enqueue(&mut self, frame: RawFrame) -> Result<(), ConnectionError> {
        let req = match fila_fibp::EnqueueRequest::decode(frame.payload) {
            Ok(r) => r,
            Err(e) => {
                send_error(
                    &mut self.stream,
                    frame.request_id,
                    ErrorCode::InvalidFrame,
                    &format!("invalid enqueue frame: {e}"),
                    &HashMap::new(),
                )
                .await?;
                return Ok(());
            }
        };

        if let Some(ref caller) = self.caller_key {
            for msg in &req.messages {
                let permitted = self
                    .server
                    .broker
                    .check_permission(
                        caller,
                        fila_core::broker::auth::Permission::Produce,
                        &msg.queue,
                    )
                    .map_err(|e| ConnectionError::Protocol(format!("acl check error: {e}")))?;
                if !permitted {
                    send_error(
                        &mut self.stream,
                        frame.request_id,
                        ErrorCode::Forbidden,
                        &format!("no produce permission on queue \"{}\"", msg.queue),
                        &HashMap::new(),
                    )
                    .await?;
                    return Ok(());
                }
            }
        }

        match binary_handlers::handle_enqueue(&self.server.broker, req).await {
            Ok(resp) => send_frame(&mut self.stream, &resp.encode(frame.request_id)).await,
            Err(e) => {
                send_error(
                    &mut self.stream,
                    frame.request_id,
                    e.0,
                    &e.1,
                    &HashMap::new(),
                )
                .await
            }
        }
    }

    async fn handle_consume(&mut self, frame: RawFrame) -> Result<(), ConnectionError> {
        let req = match fila_fibp::ConsumeRequest::decode(frame.payload) {
            Ok(r) => r,
            Err(e) => {
                send_error(
                    &mut self.stream,
                    frame.request_id,
                    ErrorCode::InvalidFrame,
                    &format!("invalid consume frame: {e}"),
                    &HashMap::new(),
                )
                .await?;
                return Ok(());
            }
        };

        // ACL check
        if let Some(ref caller) = self.caller_key {
            let permitted = self
                .server
                .broker
                .check_permission(
                    caller,
                    fila_core::broker::auth::Permission::Consume,
                    &req.queue,
                )
                .map_err(|e| ConnectionError::Protocol(format!("acl check error: {e}")))?;
            if !permitted {
                send_error(
                    &mut self.stream,
                    frame.request_id,
                    ErrorCode::Forbidden,
                    &format!("no consume permission on queue \"{}\"", req.queue),
                    &HashMap::new(),
                )
                .await?;
                return Ok(());
            }
        }

        // Cluster leader check
        if let Some(ref cluster) = self.server.cluster {
            match cluster.is_queue_leader(&req.queue).await {
                Some(true) => {}
                Some(false) => {
                    let mut meta = HashMap::new();
                    if let Some(addr) = cluster.get_queue_leader_client_addr(&req.queue).await {
                        if !addr.starts_with("0.0.0.0") {
                            meta.insert("leader_addr".to_string(), addr);
                        }
                    }
                    send_error(
                        &mut self.stream,
                        frame.request_id,
                        ErrorCode::NotLeader,
                        "not the leader for this queue",
                        &meta,
                    )
                    .await?;
                    return Ok(());
                }
                None => {
                    send_error(
                        &mut self.stream,
                        frame.request_id,
                        ErrorCode::QueueNotFound,
                        "queue raft group not found",
                        &HashMap::new(),
                    )
                    .await?;
                    return Ok(());
                }
            }
        }

        let consumer_id = uuid::Uuid::now_v7().to_string();
        let (ready_tx, mut ready_rx) = tokio::sync::mpsc::channel::<fila_core::ReadyMessage>(64);

        self.server
            .broker
            .send_command(fila_core::SchedulerCommand::RegisterConsumer {
                queue_id: req.queue.clone(),
                consumer_id: consumer_id.clone(),
                tx: ready_tx,
            })
            .map_err(ConnectionError::Broker)?;

        // If there's already a consumer registered for this request_id,
        // unregister the old one to prevent a consumer leak.
        if let Some(old_consumer_id) = self.consumers.insert(frame.request_id, consumer_id.clone())
        {
            let _ =
                self.server
                    .broker
                    .send_command(fila_core::SchedulerCommand::UnregisterConsumer {
                        consumer_id: old_consumer_id,
                    });
        }

        debug!(consumer_id = %consumer_id, queue = %req.queue, "consume stream started");

        // Spawn a task that converts ReadyMessage → Delivery frames and sends
        // them through the delivery channel to be written by the frame loop.
        let request_id = frame.request_id;
        let delivery_tx = self.delivery_tx.clone();
        tokio::spawn(async move {
            while let Some(ready) = ready_rx.recv().await {
                let delivery = fila_fibp::DeliveryBatch {
                    messages: vec![fila_fibp::DeliveryMessage {
                        message_id: ready.msg_id.to_string(),
                        queue: ready.queue_id,
                        headers: ready.headers,
                        payload: ready.payload,
                        fairness_key: ready.fairness_key,
                        weight: ready.weight,
                        throttle_keys: ready.throttle_keys,
                        attempt_count: ready.attempt_count,
                        enqueued_at: 0,
                        leased_at: 0,
                    }],
                };
                let frame = delivery.encode(request_id);
                if delivery_tx.send(frame).await.is_err() {
                    break; // Connection closed
                }
            }
        });

        Ok(())
    }

    async fn handle_cancel_consume(&mut self, frame: RawFrame) -> Result<(), ConnectionError> {
        if let Some(consumer_id) = self.consumers.remove(&frame.request_id) {
            let _ = self
                .server
                .broker
                .send_command(fila_core::SchedulerCommand::UnregisterConsumer { consumer_id });
        }
        Ok(())
    }

    async fn handle_ack(&mut self, frame: RawFrame) -> Result<(), ConnectionError> {
        let req = match fila_fibp::AckRequest::decode(frame.payload) {
            Ok(r) => r,
            Err(e) => {
                send_error(
                    &mut self.stream,
                    frame.request_id,
                    ErrorCode::InvalidFrame,
                    &format!("invalid ack frame: {e}"),
                    &HashMap::new(),
                )
                .await?;
                return Ok(());
            }
        };

        if let Some(ref caller) = self.caller_key {
            for item in &req.items {
                let permitted = self
                    .server
                    .broker
                    .check_permission(
                        caller,
                        fila_core::broker::auth::Permission::Consume,
                        &item.queue,
                    )
                    .map_err(|e| ConnectionError::Protocol(format!("acl check error: {e}")))?;
                if !permitted {
                    send_error(
                        &mut self.stream,
                        frame.request_id,
                        ErrorCode::Forbidden,
                        &format!("no consume permission on queue \"{}\"", item.queue),
                        &HashMap::new(),
                    )
                    .await?;
                    return Ok(());
                }
            }
        }

        match binary_handlers::handle_ack(&self.server.broker, req).await {
            Ok(resp) => send_frame(&mut self.stream, &resp.encode(frame.request_id)).await,
            Err(e) => {
                send_error(
                    &mut self.stream,
                    frame.request_id,
                    e.0,
                    &e.1,
                    &HashMap::new(),
                )
                .await
            }
        }
    }

    async fn handle_nack(&mut self, frame: RawFrame) -> Result<(), ConnectionError> {
        let req = match fila_fibp::NackRequest::decode(frame.payload) {
            Ok(r) => r,
            Err(e) => {
                send_error(
                    &mut self.stream,
                    frame.request_id,
                    ErrorCode::InvalidFrame,
                    &format!("invalid nack frame: {e}"),
                    &HashMap::new(),
                )
                .await?;
                return Ok(());
            }
        };

        if let Some(ref caller) = self.caller_key {
            for item in &req.items {
                let permitted = self
                    .server
                    .broker
                    .check_permission(
                        caller,
                        fila_core::broker::auth::Permission::Consume,
                        &item.queue,
                    )
                    .map_err(|e| ConnectionError::Protocol(format!("acl check error: {e}")))?;
                if !permitted {
                    send_error(
                        &mut self.stream,
                        frame.request_id,
                        ErrorCode::Forbidden,
                        &format!("no consume permission on queue \"{}\"", item.queue),
                        &HashMap::new(),
                    )
                    .await?;
                    return Ok(());
                }
            }
        }

        match binary_handlers::handle_nack(&self.server.broker, req).await {
            Ok(resp) => send_frame(&mut self.stream, &resp.encode(frame.request_id)).await,
            Err(e) => {
                send_error(
                    &mut self.stream,
                    frame.request_id,
                    e.0,
                    &e.1,
                    &HashMap::new(),
                )
                .await
            }
        }
    }
}

// --- IO helpers ---

/// Read a complete frame from the stream. Returns error on EOF.
async fn read_frame(stream: &mut Stream, buf: &mut BytesMut) -> Result<RawFrame, ConnectionError> {
    loop {
        if let Some(frame) = RawFrame::decode(buf).map_err(ConnectionError::Frame)? {
            return Ok(frame);
        }
        let n = stream.read_buf(buf).await.map_err(ConnectionError::Io)?;
        if n == 0 {
            return Err(ConnectionError::Io(std::io::Error::new(
                std::io::ErrorKind::UnexpectedEof,
                "connection closed",
            )));
        }
    }
}

/// Send a single frame.
async fn send_frame(stream: &mut Stream, frame: &RawFrame) -> Result<(), ConnectionError> {
    let mut buf = BytesMut::new();
    frame.encode(&mut buf);
    stream.write_all(&buf).await.map_err(ConnectionError::Io)?;
    stream.flush().await.map_err(ConnectionError::Io)?;
    Ok(())
}

/// Send an error frame.
async fn send_error(
    stream: &mut Stream,
    request_id: u32,
    code: ErrorCode,
    message: &str,
    metadata: &HashMap<String, String>,
) -> Result<(), ConnectionError> {
    let err = ErrorFrame {
        error_code: code,
        message: message.to_string(),
        metadata: metadata.clone(),
    };
    send_frame(stream, &err.encode(request_id)).await
}

/// Build a `tokio_rustls::TlsAcceptor` from Fila TLS config.
pub async fn build_tls_acceptor(
    tls: &TlsParams,
) -> Result<tokio_rustls::TlsAcceptor, Box<dyn std::error::Error>> {
    let cert_pem = tokio::fs::read(&tls.cert_file).await?;
    let key_pem = tokio::fs::read(&tls.key_file).await?;

    let certs: Vec<_> =
        rustls_pemfile::certs(&mut std::io::Cursor::new(&cert_pem)).collect::<Result<_, _>>()?;
    let key = rustls_pemfile::private_key(&mut std::io::Cursor::new(&key_pem))?
        .ok_or("no private key found in PEM")?;

    let mut config = if let Some(ref ca_file) = tls.ca_file {
        let ca_pem = tokio::fs::read(ca_file).await?;
        let ca_certs: Vec<_> =
            rustls_pemfile::certs(&mut std::io::Cursor::new(&ca_pem)).collect::<Result<_, _>>()?;
        let mut root_store = rustls::RootCertStore::empty();
        for cert in ca_certs {
            root_store.add(cert)?;
        }
        let verifier =
            rustls::server::WebPkiClientVerifier::builder(Arc::new(root_store)).build()?;
        rustls::ServerConfig::builder()
            .with_client_cert_verifier(verifier)
            .with_single_cert(certs, key)?
    } else {
        rustls::ServerConfig::builder()
            .with_no_client_auth()
            .with_single_cert(certs, key)?
    };

    config.alpn_protocols = vec![b"fila/1".to_vec()];

    Ok(tokio_rustls::TlsAcceptor::from(Arc::new(config)))
}

// --- Error types ---

#[derive(Debug)]
pub enum ConnectionError {
    Io(std::io::Error),
    Tls(std::io::Error),
    Frame(FrameError),
    Protocol(String),
    Broker(BrokerError),
}

impl std::fmt::Display for ConnectionError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ConnectionError::Io(e) => write!(f, "io: {e}"),
            ConnectionError::Tls(e) => write!(f, "tls: {e}"),
            ConnectionError::Frame(e) => write!(f, "frame: {e}"),
            ConnectionError::Protocol(msg) => write!(f, "protocol: {msg}"),
            ConnectionError::Broker(e) => write!(f, "broker: {e}"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn connection_error_display_io() {
        let err = ConnectionError::Io(std::io::Error::new(
            std::io::ErrorKind::ConnectionReset,
            "reset",
        ));
        assert_eq!(err.to_string(), "io: reset");
    }

    #[test]
    fn connection_error_display_tls() {
        let err = ConnectionError::Tls(std::io::Error::new(
            std::io::ErrorKind::Other,
            "cert invalid",
        ));
        assert_eq!(err.to_string(), "tls: cert invalid");
    }

    #[test]
    fn connection_error_display_frame() {
        let err = ConnectionError::Frame(FrameError::UnknownOpcode(0xFF));
        assert_eq!(err.to_string(), "frame: unknown opcode: 0xff");
    }

    #[test]
    fn connection_error_display_protocol() {
        let err = ConnectionError::Protocol("bad handshake".into());
        assert_eq!(err.to_string(), "protocol: bad handshake");
    }

    #[test]
    fn stream_is_send() {
        fn assert_send<T: Send>() {}
        assert_send::<Stream>();
    }

    #[test]
    fn stream_is_unpin() {
        fn assert_unpin<T: Unpin>() {}
        assert_unpin::<Stream>();
    }
}
