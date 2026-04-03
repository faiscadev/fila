//! Binary protocol server — accepts TCP connections and dispatches FIBP frames
//! to the broker's scheduler through the command channel.

use std::collections::HashMap;
use std::sync::Arc;

use bytes::BytesMut;
use fila_core::{Broker, BrokerError, ClusterHandle, TlsParams};
use fila_fibp::{
    ContinuationAssembler, ErrorCode, ErrorFrame, FrameError, Handshake, HandshakeOk, Opcode,
    ProtocolMessage, RawFrame, MAX_FRAME_SIZE,
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

    // --- Protocol handshake (includes auth validation) ---
    let caller_key = protocol_handshake(&server, &mut stream, &mut read_buf).await?;

    // --- Frame processing loop ---
    let mut conn = ConnectionState::new(server, stream, read_buf, caller_key);
    conn.frame_loop().await
}

/// Perform the protocol handshake: read Handshake, validate auth, send HandshakeOk.
/// Returns the validated caller key (None when auth is disabled).
async fn protocol_handshake(
    server: &BinaryServer,
    stream: &mut Stream,
    buf: &mut BytesMut,
) -> Result<Option<fila_core::broker::auth::CallerKey>, ConnectionError> {
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

    // Validate API key BEFORE sending HandshakeOk so the client gets an Error
    // frame on invalid/missing keys instead of a successful handshake.
    let caller_key = if server.broker.auth_enabled {
        match hs.api_key {
            Some(ref key) => match server.broker.validate_api_key(key) {
                Ok(Some(caller)) => Some(caller),
                Ok(None) | Err(_) => {
                    send_error(
                        stream,
                        0,
                        ErrorCode::Unauthorized,
                        "invalid API key",
                        &HashMap::new(),
                    )
                    .await?;
                    return Err(ConnectionError::Protocol("invalid API key".into()));
                }
            },
            None => {
                send_error(
                    stream,
                    0,
                    ErrorCode::Unauthorized,
                    "API key required",
                    &HashMap::new(),
                )
                .await?;
                return Err(ConnectionError::Protocol("API key required".into()));
            }
        }
    } else {
        None
    };

    let ok = HandshakeOk {
        negotiated_version: 1,
        node_id: server.node_id,
        max_frame_size: MAX_FRAME_SIZE,
    };
    send_frame(stream, &ok.encode(0)).await?;

    Ok(caller_key)
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
    /// Continuation frame reassembler for multiplexed streams.
    assembler: ContinuationAssembler,
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
            assembler: ContinuationAssembler::new(),
        }
    }

    async fn frame_loop(&mut self) -> Result<(), ConnectionError> {
        loop {
            // Try to decode a frame from the read buffer first (no IO needed).
            if let Some(raw_frame) =
                RawFrame::decode(&mut self.read_buf).map_err(ConnectionError::Frame)?
            {
                // Pass through the continuation assembler. It returns None
                // while accumulating chunks and Some(assembled) on the final
                // frame of a continuation sequence.
                match self
                    .assembler
                    .push_frame(raw_frame)
                    .map_err(ConnectionError::Frame)?
                {
                    Some(frame) => {
                        if self.dispatch_frame(frame).await? {
                            break;
                        }
                    }
                    None => {
                        // Continuation chunk buffered, loop to read more.
                    }
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

        // Cleanup: discard any partial continuation state
        self.assembler.clear();

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
            // Admin operations
            Some(Opcode::CreateQueue) => self.handle_create_queue(frame).await?,
            Some(Opcode::DeleteQueue) => self.handle_delete_queue(frame).await?,
            Some(Opcode::GetStats) => self.handle_get_stats(frame).await?,
            Some(Opcode::ListQueues) => self.handle_list_queues(frame).await?,
            Some(Opcode::SetConfig) => self.handle_set_config(frame).await?,
            Some(Opcode::GetConfig) => self.handle_get_config(frame).await?,
            Some(Opcode::ListConfig) => self.handle_list_config(frame).await?,
            Some(Opcode::Redrive) => self.handle_redrive(frame).await?,
            // Auth management operations
            Some(Opcode::CreateApiKey) => self.handle_create_api_key(frame).await?,
            Some(Opcode::RevokeApiKey) => self.handle_revoke_api_key(frame).await?,
            Some(Opcode::ListApiKeys) => self.handle_list_api_keys(frame).await?,
            Some(Opcode::SetAcl) => self.handle_set_acl(frame).await?,
            Some(Opcode::GetAcl) => self.handle_get_acl(frame).await?,
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

        match binary_handlers::handle_enqueue(
            &self.server.broker,
            self.server.cluster.as_deref(),
            req,
        )
        .await
        {
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

        // Unregister all existing consumers for this connection before
        // registering a new one. A connection supports at most one active
        // consume subscription at a time.
        for (_, old_consumer_id) in self.consumers.drain() {
            let _ =
                self.server
                    .broker
                    .send_command(fila_core::SchedulerCommand::UnregisterConsumer {
                        consumer_id: old_consumer_id,
                    });
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

        self.consumers.insert(frame.request_id, consumer_id.clone());

        debug!(consumer_id = %consumer_id, queue = %req.queue, "consume stream started");

        // Send ConsumeOk confirmation before any deliveries.
        let ok = fila_fibp::ConsumeOkResponse {
            consumer_id: consumer_id.clone(),
        };
        send_frame(&mut self.stream, &ok.encode(frame.request_id)).await?;

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

        match binary_handlers::handle_ack(&self.server.broker, self.server.cluster.as_deref(), req)
            .await
        {
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

        match binary_handlers::handle_nack(&self.server.broker, self.server.cluster.as_deref(), req)
            .await
        {
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

    // --- Admin operation handlers ---

    /// Check that the caller has global admin permission (admin:*).
    /// Returns Ok(()) when auth is disabled or the caller has the permission.
    fn check_global_admin(&self) -> Result<(), (ErrorCode, String)> {
        if let Some(ref caller) = self.caller_key {
            let permitted = self
                .server
                .broker
                .check_permission(caller, fila_core::broker::auth::Permission::Admin, "*")
                .map_err(|e| (ErrorCode::InternalError, format!("acl check error: {e}")))?;
            if !permitted {
                return Err((
                    ErrorCode::Forbidden,
                    "key does not have admin permission".to_string(),
                ));
            }
        }
        Ok(())
    }

    /// Check that the caller has any admin permission.
    /// Returns Ok(Some(caller)) when auth is enabled and the caller has admin,
    /// Ok(None) when auth is disabled.
    fn require_any_admin(
        &self,
    ) -> Result<Option<fila_core::broker::auth::CallerKey>, (ErrorCode, String)> {
        match &self.caller_key {
            None => Ok(None),
            Some(caller) => {
                let ok = self
                    .server
                    .broker
                    .has_any_admin(caller)
                    .map_err(|e| (ErrorCode::InternalError, format!("acl check error: {e}")))?;
                if ok {
                    Ok(Some(caller.clone()))
                } else {
                    Err((
                        ErrorCode::Forbidden,
                        "key does not have admin permission".to_string(),
                    ))
                }
            }
        }
    }

    async fn handle_create_queue(&mut self, frame: RawFrame) -> Result<(), ConnectionError> {
        let req = match fila_fibp::CreateQueueRequest::decode(frame.payload) {
            Ok(r) => r,
            Err(e) => {
                send_error(
                    &mut self.stream,
                    frame.request_id,
                    ErrorCode::InvalidFrame,
                    &format!("invalid create_queue frame: {e}"),
                    &HashMap::new(),
                )
                .await?;
                return Ok(());
            }
        };

        // ACL: require any admin, then check queue-scoped admin
        match self.require_any_admin() {
            Err(e) => {
                send_error(
                    &mut self.stream,
                    frame.request_id,
                    e.0,
                    &e.1,
                    &HashMap::new(),
                )
                .await?;
                return Ok(());
            }
            Ok(Some(ref caller)) => {
                let ok = self
                    .server
                    .broker
                    .caller_has_queue_admin(caller, &req.name)
                    .map_err(|e| ConnectionError::Protocol(format!("acl check error: {e}")))?;
                if !ok {
                    send_error(
                        &mut self.stream,
                        frame.request_id,
                        ErrorCode::Forbidden,
                        &format!("no admin permission on queue \"{}\"", req.name),
                        &HashMap::new(),
                    )
                    .await?;
                    return Ok(());
                }
            }
            Ok(None) => {} // auth disabled
        }

        match binary_handlers::handle_create_queue(
            &self.server.broker,
            self.server.cluster.as_deref(),
            req,
        )
        .await
        {
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

    async fn handle_delete_queue(&mut self, frame: RawFrame) -> Result<(), ConnectionError> {
        let req = match fila_fibp::DeleteQueueRequest::decode(frame.payload) {
            Ok(r) => r,
            Err(e) => {
                send_error(
                    &mut self.stream,
                    frame.request_id,
                    ErrorCode::InvalidFrame,
                    &format!("invalid delete_queue frame: {e}"),
                    &HashMap::new(),
                )
                .await?;
                return Ok(());
            }
        };

        // ACL: require any admin, then check queue-scoped admin
        match self.require_any_admin() {
            Err(e) => {
                send_error(
                    &mut self.stream,
                    frame.request_id,
                    e.0,
                    &e.1,
                    &HashMap::new(),
                )
                .await?;
                return Ok(());
            }
            Ok(Some(ref caller)) => {
                let ok = self
                    .server
                    .broker
                    .caller_has_queue_admin(caller, &req.queue)
                    .map_err(|e| ConnectionError::Protocol(format!("acl check error: {e}")))?;
                if !ok {
                    send_error(
                        &mut self.stream,
                        frame.request_id,
                        ErrorCode::Forbidden,
                        &format!("no admin permission on queue \"{}\"", req.queue),
                        &HashMap::new(),
                    )
                    .await?;
                    return Ok(());
                }
            }
            Ok(None) => {}
        }

        match binary_handlers::handle_delete_queue(
            &self.server.broker,
            self.server.cluster.as_deref(),
            req,
        )
        .await
        {
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

    async fn handle_get_stats(&mut self, frame: RawFrame) -> Result<(), ConnectionError> {
        let req = match fila_fibp::GetStatsRequest::decode(frame.payload) {
            Ok(r) => r,
            Err(e) => {
                send_error(
                    &mut self.stream,
                    frame.request_id,
                    ErrorCode::InvalidFrame,
                    &format!("invalid get_stats frame: {e}"),
                    &HashMap::new(),
                )
                .await?;
                return Ok(());
            }
        };

        if let Err(e) = self.check_global_admin() {
            send_error(
                &mut self.stream,
                frame.request_id,
                e.0,
                &e.1,
                &HashMap::new(),
            )
            .await?;
            return Ok(());
        }

        match binary_handlers::handle_get_stats(
            &self.server.broker,
            self.server.cluster.as_deref(),
            req,
        )
        .await
        {
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

    async fn handle_list_queues(&mut self, frame: RawFrame) -> Result<(), ConnectionError> {
        // ListQueues has no payload to decode
        let _ = fila_fibp::ListQueuesRequest::decode(frame.payload);

        if let Err(e) = self.check_global_admin() {
            send_error(
                &mut self.stream,
                frame.request_id,
                e.0,
                &e.1,
                &HashMap::new(),
            )
            .await?;
            return Ok(());
        }

        match binary_handlers::handle_list_queues(
            &self.server.broker,
            self.server.cluster.as_deref(),
        )
        .await
        {
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

    async fn handle_set_config(&mut self, frame: RawFrame) -> Result<(), ConnectionError> {
        let req = match fila_fibp::SetConfigRequest::decode(frame.payload) {
            Ok(r) => r,
            Err(e) => {
                send_error(
                    &mut self.stream,
                    frame.request_id,
                    ErrorCode::InvalidFrame,
                    &format!("invalid set_config frame: {e}"),
                    &HashMap::new(),
                )
                .await?;
                return Ok(());
            }
        };

        if let Err(e) = self.check_global_admin() {
            send_error(
                &mut self.stream,
                frame.request_id,
                e.0,
                &e.1,
                &HashMap::new(),
            )
            .await?;
            return Ok(());
        }

        match binary_handlers::handle_set_config(&self.server.broker, req).await {
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

    async fn handle_get_config(&mut self, frame: RawFrame) -> Result<(), ConnectionError> {
        let req = match fila_fibp::GetConfigRequest::decode(frame.payload) {
            Ok(r) => r,
            Err(e) => {
                send_error(
                    &mut self.stream,
                    frame.request_id,
                    ErrorCode::InvalidFrame,
                    &format!("invalid get_config frame: {e}"),
                    &HashMap::new(),
                )
                .await?;
                return Ok(());
            }
        };

        if let Err(e) = self.check_global_admin() {
            send_error(
                &mut self.stream,
                frame.request_id,
                e.0,
                &e.1,
                &HashMap::new(),
            )
            .await?;
            return Ok(());
        }

        match binary_handlers::handle_get_config(&self.server.broker, req).await {
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

    async fn handle_list_config(&mut self, frame: RawFrame) -> Result<(), ConnectionError> {
        let req = match fila_fibp::ListConfigRequest::decode(frame.payload) {
            Ok(r) => r,
            Err(e) => {
                send_error(
                    &mut self.stream,
                    frame.request_id,
                    ErrorCode::InvalidFrame,
                    &format!("invalid list_config frame: {e}"),
                    &HashMap::new(),
                )
                .await?;
                return Ok(());
            }
        };

        if let Err(e) = self.check_global_admin() {
            send_error(
                &mut self.stream,
                frame.request_id,
                e.0,
                &e.1,
                &HashMap::new(),
            )
            .await?;
            return Ok(());
        }

        match binary_handlers::handle_list_config(&self.server.broker, req).await {
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

    async fn handle_redrive(&mut self, frame: RawFrame) -> Result<(), ConnectionError> {
        let req = match fila_fibp::RedriveRequest::decode(frame.payload) {
            Ok(r) => r,
            Err(e) => {
                send_error(
                    &mut self.stream,
                    frame.request_id,
                    ErrorCode::InvalidFrame,
                    &format!("invalid redrive frame: {e}"),
                    &HashMap::new(),
                )
                .await?;
                return Ok(());
            }
        };

        if let Err(e) = self.check_global_admin() {
            send_error(
                &mut self.stream,
                frame.request_id,
                e.0,
                &e.1,
                &HashMap::new(),
            )
            .await?;
            return Ok(());
        }

        match binary_handlers::handle_redrive(&self.server.broker, req).await {
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

    // --- Auth management handlers ---

    async fn handle_create_api_key(&mut self, frame: RawFrame) -> Result<(), ConnectionError> {
        let req = match fila_fibp::CreateApiKeyRequest::decode(frame.payload) {
            Ok(r) => r,
            Err(e) => {
                send_error(
                    &mut self.stream,
                    frame.request_id,
                    ErrorCode::InvalidFrame,
                    &format!("invalid create_api_key frame: {e}"),
                    &HashMap::new(),
                )
                .await?;
                return Ok(());
            }
        };

        // ACL: require any admin
        match self.require_any_admin() {
            Err(e) => {
                send_error(
                    &mut self.stream,
                    frame.request_id,
                    e.0,
                    &e.1,
                    &HashMap::new(),
                )
                .await?;
                return Ok(());
            }
            Ok(Some(ref caller)) => {
                // Only superadmins can create superadmin keys
                if req.is_superadmin {
                    let ok =
                        self.server.broker.is_superadmin(caller).map_err(|e| {
                            ConnectionError::Protocol(format!("acl check error: {e}"))
                        })?;
                    if !ok {
                        send_error(
                            &mut self.stream,
                            frame.request_id,
                            ErrorCode::Forbidden,
                            "only superadmin keys can create other superadmin keys",
                            &HashMap::new(),
                        )
                        .await?;
                        return Ok(());
                    }
                }
            }
            Ok(None) => {} // auth disabled
        }

        match binary_handlers::handle_create_api_key(&self.server.broker, req) {
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

    async fn handle_revoke_api_key(&mut self, frame: RawFrame) -> Result<(), ConnectionError> {
        let req = match fila_fibp::RevokeApiKeyRequest::decode(frame.payload) {
            Ok(r) => r,
            Err(e) => {
                send_error(
                    &mut self.stream,
                    frame.request_id,
                    ErrorCode::InvalidFrame,
                    &format!("invalid revoke_api_key frame: {e}"),
                    &HashMap::new(),
                )
                .await?;
                return Ok(());
            }
        };

        if let Err(e) = self.check_global_admin() {
            send_error(
                &mut self.stream,
                frame.request_id,
                e.0,
                &e.1,
                &HashMap::new(),
            )
            .await?;
            return Ok(());
        }

        match binary_handlers::handle_revoke_api_key(&self.server.broker, req) {
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

    async fn handle_list_api_keys(&mut self, frame: RawFrame) -> Result<(), ConnectionError> {
        let _ = fila_fibp::ListApiKeysRequest::decode(frame.payload);

        if let Err(e) = self.check_global_admin() {
            send_error(
                &mut self.stream,
                frame.request_id,
                e.0,
                &e.1,
                &HashMap::new(),
            )
            .await?;
            return Ok(());
        }

        match binary_handlers::handle_list_api_keys(&self.server.broker) {
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

    async fn handle_set_acl(&mut self, frame: RawFrame) -> Result<(), ConnectionError> {
        let req = match fila_fibp::SetAclRequest::decode(frame.payload) {
            Ok(r) => r,
            Err(e) => {
                send_error(
                    &mut self.stream,
                    frame.request_id,
                    ErrorCode::InvalidFrame,
                    &format!("invalid set_acl frame: {e}"),
                    &HashMap::new(),
                )
                .await?;
                return Ok(());
            }
        };

        // ACL: require any admin, then check delegation scope
        match self.require_any_admin() {
            Err(e) => {
                send_error(
                    &mut self.stream,
                    frame.request_id,
                    e.0,
                    &e.1,
                    &HashMap::new(),
                )
                .await?;
                return Ok(());
            }
            Ok(Some(ref caller)) => {
                let perms: Vec<(String, String)> = req
                    .permissions
                    .iter()
                    .map(|p| (p.kind.clone(), p.pattern.clone()))
                    .collect();
                let ok = self
                    .server
                    .broker
                    .caller_can_grant_all(caller, &perms)
                    .map_err(|e| ConnectionError::Protocol(format!("acl check error: {e}")))?;
                if !ok {
                    send_error(
                        &mut self.stream,
                        frame.request_id,
                        ErrorCode::Forbidden,
                        "one or more permissions exceed the caller's admin scope",
                        &HashMap::new(),
                    )
                    .await?;
                    return Ok(());
                }
            }
            Ok(None) => {}
        }

        match binary_handlers::handle_set_acl(&self.server.broker, req) {
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

    async fn handle_get_acl(&mut self, frame: RawFrame) -> Result<(), ConnectionError> {
        let req = match fila_fibp::GetAclRequest::decode(frame.payload) {
            Ok(r) => r,
            Err(e) => {
                send_error(
                    &mut self.stream,
                    frame.request_id,
                    ErrorCode::InvalidFrame,
                    &format!("invalid get_acl frame: {e}"),
                    &HashMap::new(),
                )
                .await?;
                return Ok(());
            }
        };

        if let Err(e) = self.check_global_admin() {
            send_error(
                &mut self.stream,
                frame.request_id,
                e.0,
                &e.1,
                &HashMap::new(),
            )
            .await?;
            return Ok(());
        }

        match binary_handlers::handle_get_acl(&self.server.broker, req) {
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
