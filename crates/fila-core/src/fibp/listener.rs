//! FIBP TCP listener with optional TLS.
//!
//! Binds a TCP socket on the configured address and spawns a
//! `FibpConnection` task for each accepted connection. When TLS is
//! configured, the TCP stream is wrapped with `tokio_rustls::TlsAcceptor`
//! before the FIBP handshake.

use std::net::SocketAddr;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::Arc;

use bytes::{Bytes, BytesMut};
use tokio::net::TcpListener;
use tokio::sync::{watch, Notify};
use tracing::{debug, info, warn};

use super::codec::{
    OP_ACK, OP_AUTH, OP_CONSUME, OP_CREATE_QUEUE, OP_DELETE_QUEUE, OP_ENQUEUE, OP_FLOW, OP_GOAWAY,
    OP_HEARTBEAT, OP_LIST_QUEUES, OP_NACK, OP_PAUSE_QUEUE, OP_QUEUE_STATS, OP_REDRIVE,
    OP_RESUME_QUEUE,
};
use super::connection::FibpConnection;
use super::{dispatch, error::FibpError, wire};
use crate::broker::config::{FibpConfig, TlsParams};
use crate::broker::Broker;
use crate::cluster::ClusterHandle;

/// TCP listener for the FIBP transport.
///
/// Call [`start()`](FibpListener::start) to bind and begin accepting
/// connections. Call [`shutdown()`](FibpListener::shutdown) to stop accepting
/// new connections and signal existing ones to drain.
pub struct FibpListener {
    shutdown_tx: watch::Sender<bool>,
    local_addr: SocketAddr,
}

impl FibpListener {
    /// Bind on the configured address and begin accepting FIBP connections.
    ///
    /// When `tls_params` is `Some`, all connections are TLS-wrapped using
    /// the configured cert/key/CA.
    ///
    /// Returns immediately after binding; the accept loop runs in a spawned
    /// tokio task. Use the returned handle to query the local address and to
    /// trigger a graceful shutdown.
    pub async fn start(
        config: &FibpConfig,
        broker: Arc<Broker>,
        tls_params: Option<&TlsParams>,
        cluster: Option<Arc<ClusterHandle>>,
    ) -> Result<Self, FibpError> {
        let listener = TcpListener::bind(&config.listen_addr).await?;
        let local_addr = listener.local_addr()?;
        let max_frame_size = config.max_frame_size;

        let tls_acceptor = match tls_params {
            Some(tls) => Some(build_tls_acceptor(tls)?),
            None => None,
        };

        info!(
            addr = %local_addr,
            tls = tls_acceptor.is_some(),
            "starting FIBP listener"
        );

        let (shutdown_tx, shutdown_rx) = watch::channel(false);

        tokio::spawn(accept_loop(
            listener,
            max_frame_size,
            shutdown_rx,
            broker,
            tls_acceptor,
            cluster,
        ));

        Ok(Self {
            shutdown_tx,
            local_addr,
        })
    }

    /// The address the listener actually bound to (useful when port 0 is used).
    pub fn local_addr(&self) -> SocketAddr {
        self.local_addr
    }

    /// Signal the listener to stop accepting new connections.
    pub fn shutdown(&self) {
        let _ = self.shutdown_tx.send(true);
    }
}

/// Build a `tokio_rustls::TlsAcceptor` from the shared TLS config.
fn build_tls_acceptor(tls: &TlsParams) -> Result<tokio_rustls::TlsAcceptor, FibpError> {
    use std::io::BufReader;

    let cert_pem = std::fs::read(&tls.cert_file).map_err(|e| FibpError::TlsConfig {
        reason: format!("failed to read cert file {}: {e}", tls.cert_file),
    })?;
    let key_pem = std::fs::read(&tls.key_file).map_err(|e| FibpError::TlsConfig {
        reason: format!("failed to read key file {}: {e}", tls.key_file),
    })?;

    let certs: Vec<_> = rustls_pemfile::certs(&mut BufReader::new(cert_pem.as_slice()))
        .collect::<Result<Vec<_>, _>>()
        .map_err(|e| FibpError::TlsConfig {
            reason: format!("failed to parse cert PEM: {e}"),
        })?;

    let key = rustls_pemfile::private_key(&mut BufReader::new(key_pem.as_slice()))
        .map_err(|e| FibpError::TlsConfig {
            reason: format!("failed to parse key PEM: {e}"),
        })?
        .ok_or_else(|| FibpError::TlsConfig {
            reason: "no private key found in key file".into(),
        })?;

    let mut server_config = if let Some(ref ca_file) = tls.ca_file {
        // mTLS: verify client certs against the CA.
        let ca_pem = std::fs::read(ca_file).map_err(|e| FibpError::TlsConfig {
            reason: format!("failed to read CA file {ca_file}: {e}"),
        })?;
        let ca_certs: Vec<_> = rustls_pemfile::certs(&mut BufReader::new(ca_pem.as_slice()))
            .collect::<Result<Vec<_>, _>>()
            .map_err(|e| FibpError::TlsConfig {
                reason: format!("failed to parse CA PEM: {e}"),
            })?;

        let mut root_store = tokio_rustls::rustls::RootCertStore::empty();
        for cert in ca_certs {
            root_store.add(cert).map_err(|e| FibpError::TlsConfig {
                reason: format!("failed to add CA cert: {e}"),
            })?;
        }

        let verifier =
            tokio_rustls::rustls::server::WebPkiClientVerifier::builder(Arc::new(root_store))
                .build()
                .map_err(|e| FibpError::TlsConfig {
                    reason: format!("failed to build client verifier: {e}"),
                })?;

        tokio_rustls::rustls::ServerConfig::builder()
            .with_client_cert_verifier(verifier)
            .with_single_cert(certs, key)
            .map_err(|e| FibpError::TlsConfig {
                reason: format!("failed to build TLS server config: {e}"),
            })?
    } else {
        // Server-only TLS (no client cert verification).
        tokio_rustls::rustls::ServerConfig::builder()
            .with_no_client_auth()
            .with_single_cert(certs, key)
            .map_err(|e| FibpError::TlsConfig {
                reason: format!("failed to build TLS server config: {e}"),
            })?
    };

    server_config.alpn_protocols = vec![b"fibp".to_vec()];

    Ok(tokio_rustls::TlsAcceptor::from(Arc::new(server_config)))
}

/// Accept loop: runs until `shutdown_rx` signals `true`.
async fn accept_loop(
    listener: TcpListener,
    max_frame_size: u32,
    mut shutdown_rx: watch::Receiver<bool>,
    broker: Arc<Broker>,
    tls_acceptor: Option<tokio_rustls::TlsAcceptor>,
    cluster: Option<Arc<ClusterHandle>>,
) {
    loop {
        tokio::select! {
            result = listener.accept() => {
                match result {
                    Ok((stream, peer)) => {
                        debug!(peer = %peer, "accepted FIBP connection");
                        let broker = Arc::clone(&broker);
                        let tls = tls_acceptor.clone();
                        let cluster = cluster.clone();
                        tokio::spawn(async move {
                            let tcp_stream = if let Some(acceptor) = tls {
                                match acceptor.accept(stream).await {
                                    Ok(tls_stream) => {
                                        // Convert TlsStream into a TcpStream-like
                                        // by extracting the inner TCP stream after
                                        // TLS is established.
                                        //
                                        // tokio_rustls::server::TlsStream implements
                                        // AsyncRead + AsyncWrite but FibpConnection
                                        // expects TcpStream. We use a compatibility
                                        // wrapper via tokio's duplex or we can restructure.
                                        //
                                        // For now, we use the TlsStream directly by
                                        // accepting the raw TCP stream and doing TLS
                                        // at this layer. FibpConnection's generic-ness
                                        // isn't needed — we'll pipe through a compat layer.
                                        debug!(peer = %peer, "FIBP TLS handshake complete");
                                        // Create a duplex channel and bridge the TLS stream
                                        // to TcpStream-compatible I/O.
                                        handle_tls_connection(tls_stream, max_frame_size, broker, cluster, peer).await;
                                        return;
                                    }
                                    Err(e) => {
                                        warn!(peer = %peer, error = %e, "FIBP TLS handshake failed");
                                        return;
                                    }
                                }
                            } else {
                                stream
                            };
                            match FibpConnection::accept(tcp_stream, max_frame_size, broker, cluster).await {
                                Ok(conn) => conn.run().await,
                                Err(e) => {
                                    warn!(peer = %peer, error = %e, "FIBP handshake failed");
                                }
                            }
                        });
                    }
                    Err(e) => {
                        warn!(error = %e, "FIBP accept error");
                    }
                }
            }
            _ = shutdown_rx.changed() => {
                if *shutdown_rx.borrow() {
                    info!("FIBP listener shutting down");
                    break;
                }
            }
        }
    }
}

/// Handle a TLS-wrapped FIBP connection.
///
/// The TLS handshake is already complete. This function performs the FIBP
/// magic handshake over the encrypted stream, then runs the frame loop.
async fn handle_tls_connection(
    mut stream: tokio_rustls::server::TlsStream<tokio::net::TcpStream>,
    max_frame_size: u32,
    broker: Arc<Broker>,
    cluster: Option<Arc<ClusterHandle>>,
    peer: std::net::SocketAddr,
) {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio_stream::StreamExt as _;
    use tokio_util::codec::{Encoder, Framed};

    use super::codec::{FibpCodec, Frame};

    // --- FIBP handshake over TLS ---
    let mut client_magic = [0u8; 6];
    if let Err(e) = stream.read_exact(&mut client_magic).await {
        warn!(peer = %peer, error = %e, "FIBP TLS read magic failed");
        return;
    }

    if client_magic[..4] != super::MAGIC[..4] {
        let goaway = Frame::goaway("invalid magic");
        let mut buf = BytesMut::new();
        let mut enc = FibpCodec::new(max_frame_size);
        if enc.encode(goaway, &mut buf).is_ok() {
            let _ = stream.write_all(&buf).await;
        }
        warn!(peer = %peer, "FIBP TLS invalid magic");
        return;
    }

    let client_major = client_magic[4];
    if client_major != super::MAGIC[4] {
        let goaway = Frame::goaway(&format!(
            "unsupported protocol version {client_major}.x (server supports {}.x)",
            super::MAGIC[4]
        ));
        let mut buf = BytesMut::new();
        let mut enc = FibpCodec::new(max_frame_size);
        if enc.encode(goaway, &mut buf).is_ok() {
            let _ = stream.write_all(&buf).await;
        }
        warn!(peer = %peer, "FIBP TLS version mismatch");
        return;
    }

    if let Err(e) = stream.write_all(super::MAGIC).await {
        warn!(peer = %peer, error = %e, "FIBP TLS write magic failed");
        return;
    }

    debug!(peer = %peer, "FIBP TLS handshake complete");

    // When auth is disabled, the connection is pre-authenticated.
    let authenticated = !broker.auth_enabled;

    // Run the frame loop over the TLS stream using Framed.
    let codec = FibpCodec::new(max_frame_size);
    let mut framed = Framed::new(stream, codec);

    // We replicate a simplified connection loop here for TLS streams
    // since FibpConnection is typed on TcpStream. This avoids making
    // the entire connection generic which would be a larger refactor.
    let mut caller: Option<crate::broker::auth::CallerKey> = None;
    let mut is_authenticated = authenticated;
    let mut consume_state: Option<TlsConsumeState> = None;
    let mut push_frame_rx: Option<tokio::sync::mpsc::Receiver<BytesMut>> = None;

    loop {
        if let Some(ref mut push_rx) = push_frame_rx {
            tokio::select! {
                biased;
                frame_result = framed.next() => {
                    match frame_result {
                        Some(Ok(f)) => {
                            match dispatch_tls_frame(
                                &mut framed, &broker, &cluster, &mut is_authenticated,
                                &mut caller, &mut consume_state, &mut push_frame_rx,
                                f, peer, max_frame_size,
                            ).await {
                                Ok(()) => {}
                                Err(_) => break,
                            }
                        }
                        Some(Err(e)) => {
                            warn!(peer = %peer, error = %e, "FIBP TLS read error");
                            break;
                        }
                        None => {
                            debug!(peer = %peer, "FIBP TLS connection closed by peer");
                            break;
                        }
                    }
                }
                push_buf = push_rx.recv() => {
                    match push_buf {
                        Some(buf) => {
                            let stream = framed.get_mut();
                            if stream.write_all(&buf).await.is_err() { break; }
                            if stream.flush().await.is_err() { break; }
                        }
                        None => {
                            push_frame_rx = None;
                        }
                    }
                }
            }
        } else {
            let frame = match framed.next().await {
                Some(Ok(f)) => f,
                Some(Err(e)) => {
                    warn!(peer = %peer, error = %e, "FIBP TLS read error");
                    break;
                }
                None => {
                    debug!(peer = %peer, "FIBP TLS connection closed by peer");
                    break;
                }
            };

            match dispatch_tls_frame(
                &mut framed,
                &broker,
                &cluster,
                &mut is_authenticated,
                &mut caller,
                &mut consume_state,
                &mut push_frame_rx,
                frame,
                peer,
                max_frame_size,
            )
            .await
            {
                Ok(()) => {}
                Err(_) => break,
            }
        }
    }

    // Clean up consume session if active.
    if let Some(state) = consume_state.take() {
        state.push_task.abort();
        dispatch::unregister_consumer(&broker, &state.consumer_id);
    }
}

/// Consume state for TLS connections.
struct TlsConsumeState {
    consumer_id: String,
    credits: Arc<AtomicU32>,
    credits_notify: Arc<Notify>,
    push_task: tokio::task::JoinHandle<()>,
}

/// Write a frame to a generic `Framed` stream.
async fn write_frame_generic<S>(
    framed: &mut tokio_util::codec::Framed<S, super::codec::FibpCodec>,
    frame: super::codec::Frame,
) -> Result<(), FibpError>
where
    S: tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin + Send,
{
    use tokio::io::AsyncWriteExt;
    use tokio_util::codec::Encoder;

    let mut buf = BytesMut::new();
    let mut enc = super::codec::FibpCodec::new(u32::MAX);
    enc.encode(frame, &mut buf)?;
    let stream = framed.get_mut();
    stream.write_all(&buf).await?;
    stream.flush().await?;
    Ok(())
}

/// Dispatch a single frame on a TLS connection.
///
/// This duplicates the dispatch logic from `FibpConnection` for the TLS
/// code path. A future refactor could make `FibpConnection` generic over
/// the underlying stream type.
#[allow(clippy::too_many_arguments)]
async fn dispatch_tls_frame<S>(
    framed: &mut tokio_util::codec::Framed<S, super::codec::FibpCodec>,
    broker: &Arc<Broker>,
    cluster: &Option<Arc<ClusterHandle>>,
    authenticated: &mut bool,
    caller: &mut Option<crate::broker::auth::CallerKey>,
    consume_state: &mut Option<TlsConsumeState>,
    push_frame_rx: &mut Option<tokio::sync::mpsc::Receiver<BytesMut>>,
    frame: super::codec::Frame,
    peer: std::net::SocketAddr,
    max_frame_size: u32,
) -> Result<(), FibpError>
where
    S: tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin + Send,
{
    use super::codec::Frame;

    match frame.op {
        OP_AUTH => {
            if *authenticated {
                let resp = Frame::new(0, OP_AUTH, frame.correlation_id, Bytes::new());
                write_frame_generic(framed, resp).await?;
                return Ok(());
            }
            let token = String::from_utf8(frame.payload.to_vec()).map_err(|_| {
                FibpError::InvalidPayload {
                    reason: "auth payload must be valid utf-8".into(),
                }
            })?;
            match broker.validate_api_key(&token)? {
                Some(c) => {
                    *caller = Some(c);
                    *authenticated = true;
                    let resp = Frame::new(0, OP_AUTH, frame.correlation_id, Bytes::new());
                    write_frame_generic(framed, resp).await
                }
                None => {
                    let err = Frame::error(frame.correlation_id, "invalid or missing api key");
                    write_frame_generic(framed, err).await?;
                    Err(FibpError::AuthFailed {
                        reason: "invalid api key".into(),
                    })
                }
            }
        }
        OP_HEARTBEAT => {
            let pong = Frame::new(0, OP_HEARTBEAT, frame.correlation_id, Bytes::new());
            write_frame_generic(framed, pong).await
        }
        OP_GOAWAY => Err(FibpError::Io(std::io::Error::new(
            std::io::ErrorKind::ConnectionReset,
            "peer sent goaway",
        ))),
        _ if !*authenticated => {
            let err = Frame::error(
                frame.correlation_id,
                "authentication required: send OP_AUTH frame first",
            );
            write_frame_generic(framed, err).await?;
            Err(FibpError::AuthFailed {
                reason: "unauthenticated operation attempt".into(),
            })
        }
        OP_ENQUEUE => {
            let req = wire::decode_enqueue_request(frame.payload)?;
            if let Some(ref c) = caller {
                let p = broker
                    .check_permission(c, crate::broker::auth::Permission::Produce, &req.queue)
                    .map_err(FibpError::Storage)?;
                if !p {
                    let err = Frame::error(frame.correlation_id, "permission denied");
                    return write_frame_generic(framed, err).await;
                }
            }
            let results = dispatch::dispatch_enqueue(broker, cluster.as_ref(), req).await?;
            let payload = wire::encode_enqueue_response(&results);
            let resp = Frame::new(0, OP_ENQUEUE, frame.correlation_id, payload);
            write_frame_generic(framed, resp).await
        }
        OP_CONSUME => {
            if consume_state.is_some() {
                let err = Frame::error(frame.correlation_id, "consume session already active");
                return write_frame_generic(framed, err).await;
            }
            let req = wire::decode_consume_request(frame.payload)?;
            if req.queue.is_empty() {
                let err = Frame::error(frame.correlation_id, "queue name must not be empty");
                return write_frame_generic(framed, err).await;
            }
            if let Some(ref c) = caller {
                let p = broker
                    .check_permission(c, crate::broker::auth::Permission::Consume, &req.queue)
                    .map_err(FibpError::Storage)?;
                if !p {
                    let err = Frame::error(frame.correlation_id, "permission denied");
                    return write_frame_generic(framed, err).await;
                }
            }
            // In cluster mode, consume must happen on the leader. Redirect
            // if this node is not the leader.
            match dispatch::check_queue_leadership(cluster.as_ref(), &req.queue).await {
                Ok(None) => {} // single-node or we are the leader
                Ok(Some(leader_addr)) => {
                    let err =
                        Frame::error(frame.correlation_id, &format!("leader_hint:{leader_addr}"));
                    return write_frame_generic(framed, err).await;
                }
                Err(e) => {
                    let err = Frame::error(frame.correlation_id, &e.to_string());
                    return write_frame_generic(framed, err).await;
                }
            }
            let (consumer_id, ready_rx) = dispatch::register_consumer(broker, &req.queue)?;
            let credits = Arc::new(AtomicU32::new(req.initial_credits));
            let credits_notify = Arc::new(Notify::new());
            let ack = Frame::new(0, OP_CONSUME, frame.correlation_id, Bytes::new());
            write_frame_generic(framed, ack).await?;
            let (push_tx, push_rx) = tokio::sync::mpsc::channel::<BytesMut>(64);
            let push_task = tokio::spawn(super::connection::consume_push_loop(
                push_tx,
                max_frame_size,
                ready_rx,
                Arc::clone(&credits),
                Arc::clone(&credits_notify),
                peer,
            ));
            *push_frame_rx = Some(push_rx);
            *consume_state = Some(TlsConsumeState {
                consumer_id,
                credits,
                credits_notify,
                push_task,
            });
            Ok(())
        }
        OP_ACK => {
            let items = wire::decode_ack_request(frame.payload)?;
            if let Some(ref c) = caller {
                for item in &items {
                    let p = broker
                        .check_permission(c, crate::broker::auth::Permission::Consume, &item.queue)
                        .map_err(FibpError::Storage)?;
                    if !p {
                        let err = Frame::error(frame.correlation_id, "permission denied");
                        return write_frame_generic(framed, err).await;
                    }
                }
            }
            let results = dispatch::dispatch_ack(broker, cluster.as_ref(), items).await?;
            let payload = wire::encode_ack_nack_response(&results);
            let resp = Frame::new(0, OP_ACK, frame.correlation_id, payload);
            write_frame_generic(framed, resp).await
        }
        OP_NACK => {
            let items = wire::decode_nack_request(frame.payload)?;
            if let Some(ref c) = caller {
                for item in &items {
                    let p = broker
                        .check_permission(c, crate::broker::auth::Permission::Consume, &item.queue)
                        .map_err(FibpError::Storage)?;
                    if !p {
                        let err = Frame::error(frame.correlation_id, "permission denied");
                        return write_frame_generic(framed, err).await;
                    }
                }
            }
            let results = dispatch::dispatch_nack(broker, cluster.as_ref(), items).await?;
            let payload = wire::encode_ack_nack_response(&results);
            let resp = Frame::new(0, OP_NACK, frame.correlation_id, payload);
            write_frame_generic(framed, resp).await
        }
        OP_FLOW => {
            let new_credits = wire::decode_flow(frame.payload)?;
            if let Some(ref state) = consume_state {
                state.credits.fetch_add(new_credits, Ordering::Relaxed);
                state.credits_notify.notify_one();
            }
            Ok(())
        }
        op if op == OP_CREATE_QUEUE
            || op == OP_DELETE_QUEUE
            || op == OP_QUEUE_STATS
            || op == OP_LIST_QUEUES
            || op == OP_REDRIVE =>
        {
            // Admin ACL check
            if let Some(ref c) = caller {
                let p = broker
                    .check_permission(c, crate::broker::auth::Permission::Admin, "*")
                    .map_err(FibpError::Storage)?;
                if !p {
                    let err = Frame::error(frame.correlation_id, "permission denied");
                    return write_frame_generic(framed, err).await;
                }
            }
            // Permission already checked above — pass None so dispatch
            // functions skip their per-queue admin check.
            let no_caller = None;
            let result = match op {
                _ if op == OP_CREATE_QUEUE => {
                    dispatch::dispatch_create_queue(
                        broker,
                        cluster.as_ref(),
                        &no_caller,
                        frame.payload,
                    )
                    .await
                }
                _ if op == OP_DELETE_QUEUE => {
                    dispatch::dispatch_delete_queue(
                        broker,
                        cluster.as_ref(),
                        &no_caller,
                        frame.payload,
                    )
                    .await
                }
                _ if op == OP_QUEUE_STATS => {
                    dispatch::dispatch_queue_stats(
                        broker,
                        cluster.as_ref(),
                        &no_caller,
                        frame.payload,
                    )
                    .await
                }
                _ if op == OP_LIST_QUEUES => {
                    dispatch::dispatch_list_queues(broker, cluster.as_ref(), frame.payload).await
                }
                _ if op == OP_REDRIVE => {
                    dispatch::dispatch_redrive(broker, &no_caller, frame.payload).await
                }
                _ => unreachable!(),
            };
            match result {
                Ok(payload) => {
                    let resp = Frame::new(0, frame.op, frame.correlation_id, payload);
                    write_frame_generic(framed, resp).await
                }
                Err(e) => {
                    let err = Frame::error(frame.correlation_id, &e.to_string());
                    write_frame_generic(framed, err).await
                }
            }
        }
        op if op == OP_PAUSE_QUEUE || op == OP_RESUME_QUEUE => {
            let err = Frame::error(
                frame.correlation_id,
                &format!("operation 0x{op:02X} not implemented"),
            );
            write_frame_generic(framed, err).await
        }
        op => {
            let err = Frame::error(
                frame.correlation_id,
                &format!("unknown operation 0x{op:02X}"),
            );
            write_frame_generic(framed, err).await
        }
    }
}
