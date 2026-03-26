//! FIBP connection handler.
//!
//! `FibpConnection` wraps a TCP stream, performs the protocol handshake, and
//! dispatches incoming frames to the scheduler via `Arc<Broker>`.
//!
//! When auth is enabled, the first frame after handshake MUST be `OP_AUTH`.
//! The auth payload is the raw API key (UTF-8). If auth fails, an error
//! frame is sent and the connection is closed. Subsequent operations are
//! checked against per-queue ACLs using the authenticated caller identity.

use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::Arc;

use bytes::{Bytes, BytesMut};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;
use tokio::sync::Notify;
use tokio_stream::StreamExt;
use tokio_util::codec::{Encoder, Framed};
use tracing::{debug, warn};

use crate::broker::auth::{CallerKey, Permission};
use crate::broker::Broker;
use crate::cluster::ClusterHandle;

use super::codec::{
    FibpCodec, Frame, FLAG_STREAM, OP_ACK, OP_AUTH, OP_AUTH_CREATE_KEY, OP_AUTH_GET_ACL,
    OP_AUTH_LIST_KEYS, OP_AUTH_REVOKE_KEY, OP_AUTH_SET_ACL, OP_CONFIG_GET, OP_CONFIG_LIST,
    OP_CONFIG_SET, OP_CONSUME, OP_CREATE_QUEUE, OP_DELETE_QUEUE, OP_ENQUEUE, OP_FLOW, OP_GOAWAY,
    OP_HEARTBEAT, OP_LIST_QUEUES, OP_NACK, OP_PAUSE_QUEUE, OP_QUEUE_STATS, OP_REDRIVE,
    OP_RESUME_QUEUE,
};
use super::dispatch;
use super::error::FibpError;
use super::wire;
use super::MAGIC;

/// A single FIBP client connection.
///
/// Performs the handshake and then enters a frame read/dispatch loop.
/// Data operations are dispatched to the scheduler via `Arc<Broker>`.
///
/// `Debug` is implemented manually because `Framed<TcpStream>` does not
/// derive `Debug`.
pub struct FibpConnection {
    framed: Framed<TcpStream, FibpCodec>,
    peer_addr: std::net::SocketAddr,
    broker: Arc<Broker>,
    /// Optional cluster handle for enriching responses with Raft metadata.
    /// `None` in single-node mode.
    cluster: Option<Arc<ClusterHandle>>,
    /// Active consume session state. Only one consume stream per connection.
    consume_state: Option<ConsumeState>,
    /// Channel for the push task to send encoded frames that the main loop
    /// writes to the TCP stream. This avoids splitting the TcpStream.
    push_frame_rx: Option<tokio::sync::mpsc::Receiver<BytesMut>>,
    /// Whether authentication has been completed. Always `true` when auth is
    /// disabled on the broker (all operations are allowed).
    authenticated: bool,
    /// The authenticated caller identity. `None` when auth is disabled.
    caller: Option<CallerKey>,
}

impl std::fmt::Debug for FibpConnection {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("FibpConnection")
            .field("peer_addr", &self.peer_addr)
            .field("consume_active", &self.consume_state.is_some())
            .field("authenticated", &self.authenticated)
            .finish_non_exhaustive()
    }
}

/// State for an active consume session on this connection.
struct ConsumeState {
    consumer_id: String,
    /// Available credits for server-push delivery. The consumer task reads
    /// this atomically before sending each push frame.
    credits: Arc<AtomicU32>,
    /// Notified when new credits are added via OP_FLOW.
    credits_notify: Arc<Notify>,
    /// Abort handle for the push task so we can cancel it on disconnect.
    push_task: tokio::task::JoinHandle<()>,
}

impl FibpConnection {
    /// Accept a new FIBP connection on `stream`.
    ///
    /// Performs the 6-byte handshake before returning. Returns `Err` if the
    /// handshake fails (version mismatch, I/O error, etc.).
    pub async fn accept(
        mut stream: TcpStream,
        max_frame_size: u32,
        broker: Arc<Broker>,
        cluster: Option<Arc<ClusterHandle>>,
    ) -> Result<Self, FibpError> {
        let peer_addr = stream.peer_addr()?;
        debug!(peer = %peer_addr, "fibp handshake starting");

        // --- Handshake: read client magic, validate, echo back. ---
        let mut client_magic = [0u8; 6];
        stream.read_exact(&mut client_magic).await?;

        if client_magic[..4] != MAGIC[..4] {
            // Not FIBP at all — write GoAway and close.
            let goaway = Frame::goaway("invalid magic");
            write_frame_raw(&mut stream, &goaway, max_frame_size)
                .await
                .ok();
            return Err(FibpError::HandshakeFailed {
                reason: "invalid magic bytes".into(),
            });
        }

        let client_major = client_magic[4];
        if client_major != MAGIC[4] {
            // Major version mismatch — send GoAway.
            let goaway = Frame::goaway(&format!(
                "unsupported protocol version {client_major}.x (server supports {}.x)",
                MAGIC[4]
            ));
            write_frame_raw(&mut stream, &goaway, max_frame_size)
                .await
                .ok();
            return Err(FibpError::HandshakeFailed {
                reason: format!(
                    "major version mismatch: client={client_major}, server={}",
                    MAGIC[4]
                ),
            });
        }

        // Handshake accepted — echo the server's magic.
        stream.write_all(MAGIC).await?;

        let codec = FibpCodec::new(max_frame_size);
        let framed = Framed::new(stream, codec);

        // When auth is disabled, the connection is pre-authenticated.
        let authenticated = !broker.auth_enabled;

        debug!(peer = %peer_addr, auth_required = broker.auth_enabled, "fibp handshake complete");
        Ok(Self {
            framed,
            peer_addr,
            broker,
            cluster,
            consume_state: None,
            push_frame_rx: None,
            authenticated,
            caller: None,
        })
    }

    /// Run the connection loop until the peer disconnects or an error occurs.
    ///
    /// When a consume session is active, the loop also drains server-push
    /// frames from the push task and writes them to the TCP stream.
    pub async fn run(mut self) {
        loop {
            // When a consume session is active, select between incoming frames
            // and push frames from the background task.
            if let Some(ref mut push_rx) = self.push_frame_rx {
                tokio::select! {
                    biased;
                    // Prioritize incoming frames (ack/nack/flow from client).
                    frame_result = self.framed.next() => {
                        match frame_result {
                            Some(Ok(f)) => {
                                if let Err(e) = self.dispatch(f).await {
                                    warn!(peer = %self.peer_addr, error = %e, "fibp dispatch error");
                                    break;
                                }
                            }
                            Some(Err(e)) => {
                                warn!(peer = %self.peer_addr, error = %e, "fibp read error");
                                break;
                            }
                            None => {
                                debug!(peer = %self.peer_addr, "fibp connection closed by peer");
                                break;
                            }
                        }
                    }
                    // Write push frames from the background consume task.
                    push_buf = push_rx.recv() => {
                        match push_buf {
                            Some(buf) => {
                                let stream = self.framed.get_mut();
                                if let Err(e) = stream.write_all(&buf).await {
                                    debug!(peer = %self.peer_addr, error = %e, "push write error");
                                    break;
                                }
                                if let Err(e) = stream.flush().await {
                                    debug!(peer = %self.peer_addr, error = %e, "push flush error");
                                    break;
                                }
                            }
                            None => {
                                // Push task ended (scheduler closed channel).
                                debug!(peer = %self.peer_addr, "consume push channel closed");
                                self.push_frame_rx = None;
                            }
                        }
                    }
                }
            } else {
                // No consume session — just process incoming frames.
                let frame = match self.framed.next().await {
                    Some(Ok(f)) => f,
                    Some(Err(e)) => {
                        warn!(peer = %self.peer_addr, error = %e, "fibp read error");
                        break;
                    }
                    None => {
                        debug!(peer = %self.peer_addr, "fibp connection closed by peer");
                        break;
                    }
                };

                if let Err(e) = self.dispatch(frame).await {
                    warn!(peer = %self.peer_addr, error = %e, "fibp dispatch error");
                    break;
                }
            }
        }

        // Clean up consume session if active.
        self.cleanup_consume();
    }

    /// Dispatch a single incoming frame.
    async fn dispatch(&mut self, frame: Frame) -> Result<(), FibpError> {
        // OP_AUTH is always allowed (it's how the client authenticates).
        // Heartbeat and GoAway are control frames, always allowed.
        // All other ops require authentication when auth is enabled.
        match frame.op {
            OP_AUTH => return self.handle_auth(frame).await,
            OP_HEARTBEAT => {
                let pong = Frame::new(0, OP_HEARTBEAT, frame.correlation_id, Bytes::new());
                return self.write_frame(pong).await;
            }
            OP_GOAWAY => {
                debug!(peer = %self.peer_addr, "received goaway from peer");
                return Err(FibpError::Io(std::io::Error::new(
                    std::io::ErrorKind::ConnectionReset,
                    "peer sent goaway",
                )));
            }
            _ => {}
        }

        // Enforce authentication: if not authenticated, reject with error.
        if !self.authenticated {
            let err_frame = Frame::error(
                frame.correlation_id,
                "authentication required: send OP_AUTH frame first",
            );
            self.write_frame(err_frame).await?;
            return Err(FibpError::AuthFailed {
                reason: "unauthenticated operation attempt".into(),
            });
        }

        match frame.op {
            OP_ENQUEUE => self.handle_enqueue(frame).await,
            OP_CONSUME => self.handle_consume(frame).await,
            OP_ACK => self.handle_ack(frame).await,
            OP_NACK => self.handle_nack(frame).await,
            OP_FLOW => self.handle_flow(frame),
            // Admin operations — protobuf-encoded payloads.
            OP_CREATE_QUEUE => self.handle_admin(frame, Permission::Admin).await,
            OP_DELETE_QUEUE => self.handle_admin(frame, Permission::Admin).await,
            OP_QUEUE_STATS => self.handle_admin(frame, Permission::Admin).await,
            OP_LIST_QUEUES => self.handle_admin(frame, Permission::Admin).await,
            OP_PAUSE_QUEUE | OP_RESUME_QUEUE => {
                let err_frame = Frame::error(
                    frame.correlation_id,
                    &format!("operation 0x{:02X} not implemented", frame.op),
                );
                self.write_frame(err_frame).await
            }
            OP_REDRIVE => self.handle_admin(frame, Permission::Admin).await,
            // Config operations — protobuf-encoded payloads.
            OP_CONFIG_SET | OP_CONFIG_GET | OP_CONFIG_LIST => {
                self.handle_admin(frame, Permission::Admin).await
            }
            // Auth CRUD operations — protobuf-encoded payloads.
            OP_AUTH_CREATE_KEY | OP_AUTH_REVOKE_KEY | OP_AUTH_LIST_KEYS | OP_AUTH_SET_ACL
            | OP_AUTH_GET_ACL => self.handle_admin(frame, Permission::Admin).await,
            op => {
                let err_frame = Frame::error(
                    frame.correlation_id,
                    &format!("unknown operation 0x{op:02X}"),
                );
                self.write_frame(err_frame).await
            }
        }
    }

    // ----- Auth handler ------------------------------------------------------

    async fn handle_auth(&mut self, frame: Frame) -> Result<(), FibpError> {
        if self.authenticated {
            let resp = Frame::new(0, OP_AUTH, frame.correlation_id, Bytes::new());
            return self.write_frame(resp).await;
        }

        // Payload is the raw API key as UTF-8 bytes.
        let token =
            String::from_utf8(frame.payload.to_vec()).map_err(|_| FibpError::InvalidPayload {
                reason: "auth payload must be valid utf-8".into(),
            })?;

        match self.broker.validate_api_key(&token)? {
            Some(caller) => {
                debug!(peer = %self.peer_addr, "fibp auth success");
                self.caller = Some(caller);
                self.authenticated = true;
                let resp = Frame::new(0, OP_AUTH, frame.correlation_id, Bytes::new());
                self.write_frame(resp).await
            }
            None => {
                let err_frame = Frame::error(frame.correlation_id, "invalid or missing api key");
                self.write_frame(err_frame).await?;
                Err(FibpError::AuthFailed {
                    reason: "invalid api key".into(),
                })
            }
        }
    }

    // ----- ACL helpers -------------------------------------------------------

    /// Check permission for a queue-scoped data operation.
    fn check_permission(&self, perm: Permission, queue: &str) -> Result<(), FibpError> {
        if let Some(ref caller) = self.caller {
            let permitted = self
                .broker
                .check_permission(caller, perm, queue)
                .map_err(FibpError::Storage)?;
            if !permitted {
                return Err(FibpError::PermissionDenied {
                    reason: format!(
                        "key does not have {} permission on queue \"{queue}\"",
                        match perm {
                            Permission::Produce => "produce",
                            Permission::Consume => "consume",
                            Permission::Admin => "admin",
                        }
                    ),
                });
            }
        }
        Ok(())
    }

    /// Check that the caller has admin permission (global `admin:*` or
    /// superadmin). Used for admin operations.
    fn check_admin_permission(&self) -> Result<(), FibpError> {
        if let Some(ref caller) = self.caller {
            let permitted = self
                .broker
                .check_permission(caller, Permission::Admin, "*")
                .map_err(FibpError::Storage)?;
            if !permitted {
                return Err(FibpError::PermissionDenied {
                    reason: "key does not have admin permission".into(),
                });
            }
        }
        Ok(())
    }

    // ----- Data operation handlers -------------------------------------------

    async fn handle_enqueue(&mut self, frame: Frame) -> Result<(), FibpError> {
        let req = wire::decode_enqueue_request(frame.payload)?;
        // ACL: produce permission on the target queue.
        if let Err(e) = self.check_permission(Permission::Produce, &req.queue) {
            let err_frame = Frame::error(frame.correlation_id, &e.to_string());
            return self.write_frame(err_frame).await;
        }
        let results = dispatch::dispatch_enqueue(&self.broker, self.cluster.as_ref(), req).await?;
        let payload = wire::encode_enqueue_response(&results);
        let resp = Frame::new(0, OP_ENQUEUE, frame.correlation_id, payload);
        self.write_frame(resp).await
    }

    async fn handle_consume(&mut self, frame: Frame) -> Result<(), FibpError> {
        if self.consume_state.is_some() {
            let err_frame = Frame::error(
                frame.correlation_id,
                "consume session already active on this connection",
            );
            return self.write_frame(err_frame).await;
        }

        let req = wire::decode_consume_request(frame.payload)?;
        if req.queue.is_empty() {
            let err_frame = Frame::error(frame.correlation_id, "queue name must not be empty");
            return self.write_frame(err_frame).await;
        }

        // ACL: consume permission on the target queue.
        if let Err(e) = self.check_permission(Permission::Consume, &req.queue) {
            let err_frame = Frame::error(frame.correlation_id, &e.to_string());
            return self.write_frame(err_frame).await;
        }

        // In cluster mode, consume must happen on the leader. If this node
        // is not the leader, send an error with the leader's client address
        // so the SDK can transparently reconnect.
        match dispatch::check_queue_leadership(self.cluster.as_ref(), &req.queue).await {
            Ok(None) => {} // single-node or we are the leader — proceed
            Ok(Some(leader_addr)) => {
                let err_frame =
                    Frame::error(frame.correlation_id, &format!("leader_hint:{leader_addr}"));
                return self.write_frame(err_frame).await;
            }
            Err(e) => {
                let err_frame = Frame::error(frame.correlation_id, &e.to_string());
                return self.write_frame(err_frame).await;
            }
        }

        let (consumer_id, ready_rx) = dispatch::register_consumer(&self.broker, &req.queue)?;

        let credits = Arc::new(AtomicU32::new(req.initial_credits));
        let credits_notify = Arc::new(Notify::new());

        // Send an ack response (empty payload = success).
        let ack = Frame::new(0, OP_CONSUME, frame.correlation_id, Bytes::new());
        self.write_frame(ack).await?;

        // Channel for the push task to send pre-encoded frame bytes back to
        // the main loop, which writes them to the TCP stream.
        let (push_tx, push_rx) = tokio::sync::mpsc::channel::<BytesMut>(64);

        let max_frame_size = self.framed.codec().max_frame_size();
        let peer = self.peer_addr;

        let push_task = tokio::spawn(consume_push_loop(
            push_tx,
            max_frame_size,
            ready_rx,
            Arc::clone(&credits),
            Arc::clone(&credits_notify),
            peer,
        ));

        debug!(
            peer = %self.peer_addr,
            consumer_id = %consumer_id,
            credits = req.initial_credits,
            "fibp consume session started"
        );

        self.push_frame_rx = Some(push_rx);
        self.consume_state = Some(ConsumeState {
            consumer_id,
            credits,
            credits_notify,
            push_task,
        });

        Ok(())
    }

    async fn handle_ack(&mut self, frame: Frame) -> Result<(), FibpError> {
        let items = wire::decode_ack_request(frame.payload)?;
        // ACL: consume permission on each queue (ack is part of consume lifecycle).
        for item in &items {
            if let Err(e) = self.check_permission(Permission::Consume, &item.queue) {
                let err_frame = Frame::error(frame.correlation_id, &e.to_string());
                return self.write_frame(err_frame).await;
            }
        }
        let results = dispatch::dispatch_ack(&self.broker, self.cluster.as_ref(), items).await?;
        let payload = wire::encode_ack_nack_response(&results);
        let resp = Frame::new(0, OP_ACK, frame.correlation_id, payload);
        self.write_frame(resp).await
    }

    async fn handle_nack(&mut self, frame: Frame) -> Result<(), FibpError> {
        let items = wire::decode_nack_request(frame.payload)?;
        // ACL: consume permission on each queue (nack is part of consume lifecycle).
        for item in &items {
            if let Err(e) = self.check_permission(Permission::Consume, &item.queue) {
                let err_frame = Frame::error(frame.correlation_id, &e.to_string());
                return self.write_frame(err_frame).await;
            }
        }
        let results = dispatch::dispatch_nack(&self.broker, self.cluster.as_ref(), items).await?;
        let payload = wire::encode_ack_nack_response(&results);
        let resp = Frame::new(0, OP_NACK, frame.correlation_id, payload);
        self.write_frame(resp).await
    }

    fn handle_flow(&mut self, frame: Frame) -> Result<(), FibpError> {
        let new_credits = wire::decode_flow(frame.payload)?;
        if let Some(ref state) = self.consume_state {
            state.credits.fetch_add(new_credits, Ordering::Relaxed);
            state.credits_notify.notify_one();
        }
        // Flow frames do not produce a response.
        Ok(())
    }

    // ----- Admin operation handler -------------------------------------------

    /// Handle an admin operation frame. The payload is protobuf-encoded.
    ///
    /// Permission model:
    /// - Auth CRUD ops: permission checks are inside the dispatch functions.
    /// - Queue-scoped ops (create/delete/stats/redrive): check is deferred
    ///   to the dispatch handler, which validates queue-level admin scope.
    /// - Global ops (config set/get/list, list-queues): require admin:* or superadmin.
    async fn handle_admin(&mut self, frame: Frame, _perm: Permission) -> Result<(), FibpError> {
        // Only global (non-queue-scoped) admin ops need the global admin check
        // here. Queue-scoped ops check admin scope per-queue inside their
        // dispatch functions, and auth CRUD ops have their own permission model.
        let needs_global_admin = matches!(
            frame.op,
            OP_CONFIG_SET | OP_CONFIG_GET | OP_CONFIG_LIST | OP_LIST_QUEUES
        );

        if needs_global_admin {
            if let Err(e) = self.check_admin_permission() {
                let err_frame = Frame::error(frame.correlation_id, &e.to_string());
                return self.write_frame(err_frame).await;
            }
        }

        let result = match frame.op {
            OP_CREATE_QUEUE => {
                dispatch::dispatch_create_queue(
                    &self.broker,
                    self.cluster.as_ref(),
                    &self.caller,
                    frame.payload,
                )
                .await
            }
            OP_DELETE_QUEUE => {
                dispatch::dispatch_delete_queue(
                    &self.broker,
                    self.cluster.as_ref(),
                    &self.caller,
                    frame.payload,
                )
                .await
            }
            OP_QUEUE_STATS => {
                dispatch::dispatch_queue_stats(
                    &self.broker,
                    self.cluster.as_ref(),
                    &self.caller,
                    frame.payload,
                )
                .await
            }
            OP_LIST_QUEUES => {
                dispatch::dispatch_list_queues(&self.broker, self.cluster.as_ref(), frame.payload)
                    .await
            }
            OP_REDRIVE => {
                dispatch::dispatch_redrive(&self.broker, &self.caller, frame.payload).await
            }
            OP_CONFIG_SET => dispatch::dispatch_config_set(&self.broker, frame.payload).await,
            OP_CONFIG_GET => dispatch::dispatch_config_get(&self.broker, frame.payload).await,
            OP_CONFIG_LIST => dispatch::dispatch_config_list(&self.broker, frame.payload).await,
            OP_AUTH_CREATE_KEY => {
                dispatch::dispatch_auth_create_key(&self.broker, &self.caller, frame.payload)
            }
            OP_AUTH_REVOKE_KEY => dispatch::dispatch_auth_revoke_key(&self.broker, frame.payload),
            OP_AUTH_LIST_KEYS => dispatch::dispatch_auth_list_keys(&self.broker, frame.payload),
            OP_AUTH_SET_ACL => {
                dispatch::dispatch_auth_set_acl(&self.broker, &self.caller, frame.payload)
            }
            OP_AUTH_GET_ACL => dispatch::dispatch_auth_get_acl(&self.broker, frame.payload),
            _ => Err(FibpError::NotImplemented { op: frame.op }),
        };

        match result {
            Ok(payload) => {
                let resp = Frame::new(0, frame.op, frame.correlation_id, payload);
                self.write_frame(resp).await
            }
            Err(e) => {
                let err_frame = Frame::error(frame.correlation_id, &e.to_string());
                self.write_frame(err_frame).await
            }
        }
    }

    // ----- Helpers -----------------------------------------------------------

    fn cleanup_consume(&mut self) {
        if let Some(state) = self.consume_state.take() {
            state.push_task.abort();
            dispatch::unregister_consumer(&self.broker, &state.consumer_id);
            debug!(
                peer = %self.peer_addr,
                consumer_id = %state.consumer_id,
                "fibp consume session cleaned up"
            );
        }
    }

    /// Encode and write a frame to the underlying TCP stream.
    async fn write_frame(&mut self, frame: Frame) -> Result<(), FibpError> {
        let mut buf = BytesMut::new();
        // Use a temporary codec for encoding only (the framed codec owns the read half).
        let mut enc = FibpCodec::new(u32::MAX);
        enc.encode(frame, &mut buf)?;

        let stream = self.framed.get_mut();
        stream.write_all(&buf).await?;
        stream.flush().await?;
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// Consume push loop
// ---------------------------------------------------------------------------

/// Background task that reads `ReadyMessage`s from the scheduler channel
/// and encodes them as FIBP push frames, sending the encoded bytes through
/// `push_tx` for the main connection loop to write to the TCP stream.
///
/// Respects credit-based flow control: only sends when `credits > 0`.
/// Blocks on `credits_notify` when credits are exhausted.
pub(super) async fn consume_push_loop(
    push_tx: tokio::sync::mpsc::Sender<BytesMut>,
    max_frame_size: u32,
    mut ready_rx: tokio::sync::mpsc::Receiver<crate::broker::command::ReadyMessage>,
    credits: Arc<AtomicU32>,
    credits_notify: Arc<Notify>,
    peer: std::net::SocketAddr,
) {
    loop {
        // Wait for a ready message.
        let first = match ready_rx.recv().await {
            Some(msg) => msg,
            None => {
                debug!(peer = %peer, "consume push loop: scheduler channel closed");
                return;
            }
        };

        // Batch drain: collect any immediately available messages.
        let mut batch = vec![first];
        while batch.len() < 64 {
            match ready_rx.try_recv() {
                Ok(msg) => batch.push(msg),
                Err(_) => break,
            }
        }

        // Wait for credits if needed.
        loop {
            let avail = credits.load(Ordering::Relaxed);
            if avail > 0 {
                // Decrement credits by the batch size (or available, whichever is smaller).
                let to_send = batch.len().min(avail as usize);
                credits.fetch_sub(to_send as u32, Ordering::Relaxed);
                if to_send < batch.len() {
                    batch.truncate(to_send);
                }
                break;
            }
            credits_notify.notified().await;
        }

        if batch.is_empty() {
            continue;
        }

        let push_messages: Vec<wire::ConsumePushMessage> = batch
            .into_iter()
            .map(|rm| wire::ConsumePushMessage {
                msg_id: rm.msg_id.to_string(),
                fairness_key: rm.fairness_key,
                attempt_count: rm.attempt_count,
                headers: rm.headers,
                payload: rm.payload,
            })
            .collect();

        let payload = wire::encode_consume_push(&push_messages);
        let frame = Frame::new(FLAG_STREAM, OP_CONSUME, 0, payload);

        // Encode the frame into bytes.
        let mut buf = BytesMut::new();
        let mut enc = FibpCodec::new(max_frame_size);
        if let Err(e) = enc.encode(frame, &mut buf) {
            warn!(peer = %peer, error = %e, "consume push encode error");
            return;
        }

        // Send the encoded bytes to the main loop for writing.
        if push_tx.send(buf).await.is_err() {
            debug!(peer = %peer, "consume push channel closed (connection dropped)");
            return;
        }
    }
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Write a frame directly to a raw `TcpStream` (used during handshake, before
/// the `Framed` wrapper is created).
async fn write_frame_raw(
    stream: &mut TcpStream,
    frame: &Frame,
    max_frame_size: u32,
) -> Result<(), FibpError> {
    let mut buf = BytesMut::new();
    let mut enc = FibpCodec::new(max_frame_size);
    enc.encode(frame.clone(), &mut buf)?;
    stream.write_all(&buf).await?;
    stream.flush().await?;
    Ok(())
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::super::codec::OP_ERROR;
    use super::*;
    use crate::fibp::MAGIC;
    use tokio::net::TcpListener;
    use tokio_util::codec::Decoder;

    /// Helper: start a broker backed by in-memory storage for tests.
    fn test_broker() -> Arc<Broker> {
        let config = crate::BrokerConfig::default();
        let storage = Arc::new(crate::InMemoryEngine::new());
        Arc::new(Broker::new(config, storage).unwrap())
    }

    /// Helper: start a broker with auth enabled.
    fn test_broker_with_auth(bootstrap_key: &str) -> Arc<Broker> {
        let config = crate::BrokerConfig {
            auth: Some(crate::AuthConfig {
                bootstrap_apikey: bootstrap_key.to_string(),
            }),
            ..Default::default()
        };
        let storage = Arc::new(crate::InMemoryEngine::new());
        Arc::new(Broker::new(config, storage).unwrap())
    }

    /// Helper: connect a raw TCP client, perform handshake, return the stream.
    async fn handshake_client(addr: std::net::SocketAddr) -> TcpStream {
        let mut stream = TcpStream::connect(addr).await.unwrap();
        stream.write_all(MAGIC).await.unwrap();
        let mut server_magic = [0u8; 6];
        stream.read_exact(&mut server_magic).await.unwrap();
        assert_eq!(&server_magic, MAGIC);
        stream
    }

    #[tokio::test]
    async fn handshake_success() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let broker = test_broker();

        let server = tokio::spawn(async move {
            let (stream, _) = listener.accept().await.unwrap();
            FibpConnection::accept(stream, 16_777_216, broker, None)
                .await
                .unwrap();
        });

        let _client = handshake_client(addr).await;
        server.await.unwrap();
    }

    #[tokio::test]
    async fn handshake_bad_magic() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let broker = test_broker();

        let server = tokio::spawn(async move {
            let (stream, _) = listener.accept().await.unwrap();
            let err = FibpConnection::accept(stream, 16_777_216, broker, None)
                .await
                .unwrap_err();
            assert!(
                matches!(err, FibpError::HandshakeFailed { .. }),
                "expected HandshakeFailed, got: {err:?}"
            );
        });

        let mut client = TcpStream::connect(addr).await.unwrap();
        client.write_all(b"HTTP/1").await.unwrap();
        let mut buf = vec![0u8; 256];
        let _ = client.read(&mut buf).await;

        server.await.unwrap();
    }

    #[tokio::test]
    async fn handshake_version_mismatch() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let broker = test_broker();

        let server = tokio::spawn(async move {
            let (stream, _) = listener.accept().await.unwrap();
            let err = FibpConnection::accept(stream, 16_777_216, broker, None)
                .await
                .unwrap_err();
            assert!(
                matches!(err, FibpError::HandshakeFailed { .. }),
                "expected HandshakeFailed, got: {err:?}"
            );
        });

        let mut client = TcpStream::connect(addr).await.unwrap();
        client.write_all(b"FIBP\x63\x00").await.unwrap();
        let mut buf = vec![0u8; 256];
        let _ = client.read(&mut buf).await;

        server.await.unwrap();
    }

    #[tokio::test]
    async fn heartbeat_echo() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let broker = test_broker();

        let server = tokio::spawn(async move {
            let (stream, _) = listener.accept().await.unwrap();
            let conn = FibpConnection::accept(stream, 16_777_216, broker, None)
                .await
                .unwrap();
            conn.run().await;
        });

        let client_stream = handshake_client(addr).await;
        let mut codec = FibpCodec::new(16_777_216);

        let ping = Frame::new(0, OP_HEARTBEAT, 123, Bytes::new());
        let mut send_buf = BytesMut::new();
        codec.encode(ping, &mut send_buf).unwrap();

        let (mut read_half, mut write_half) = client_stream.into_split();
        write_half.write_all(&send_buf).await.unwrap();

        let mut recv_buf = BytesMut::new();
        loop {
            let mut tmp = [0u8; 256];
            let n = read_half.read(&mut tmp).await.unwrap();
            recv_buf.extend_from_slice(&tmp[..n]);
            if let Some(frame) = codec.decode(&mut recv_buf).unwrap() {
                assert_eq!(frame.op, OP_HEARTBEAT);
                assert_eq!(frame.correlation_id, 123);
                break;
            }
        }

        drop(write_half);
        drop(read_half);
        server.await.unwrap();
    }

    #[tokio::test]
    async fn auth_required_when_enabled() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let broker = test_broker_with_auth("test-key-123");

        let server = tokio::spawn(async move {
            let (stream, _) = listener.accept().await.unwrap();
            let conn = FibpConnection::accept(stream, 16_777_216, broker, None)
                .await
                .unwrap();
            conn.run().await;
        });

        let client_stream = handshake_client(addr).await;
        let mut codec = FibpCodec::new(16_777_216);

        // Try to send an enqueue without auth — should get error.
        let req = Frame::new(0, OP_ENQUEUE, 1, Bytes::from_static(b"test"));
        let mut send_buf = BytesMut::new();
        codec.encode(req, &mut send_buf).unwrap();

        let (mut read_half, mut write_half) = client_stream.into_split();
        write_half.write_all(&send_buf).await.unwrap();

        let mut recv_buf = BytesMut::new();
        loop {
            let mut tmp = [0u8; 1024];
            let n = read_half.read(&mut tmp).await.unwrap();
            recv_buf.extend_from_slice(&tmp[..n]);
            if let Some(frame) = codec.decode(&mut recv_buf).unwrap() {
                assert_eq!(frame.op, OP_ERROR, "expected error frame");
                let msg = String::from_utf8_lossy(&frame.payload);
                assert!(
                    msg.contains("authentication required"),
                    "expected auth error, got: {msg}"
                );
                break;
            }
        }

        drop(write_half);
        drop(read_half);
        server.await.unwrap();
    }

    #[tokio::test]
    async fn auth_success_allows_operations() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let broker = test_broker_with_auth("test-key-456");

        let server = tokio::spawn(async move {
            let (stream, _) = listener.accept().await.unwrap();
            let conn = FibpConnection::accept(stream, 16_777_216, broker, None)
                .await
                .unwrap();
            conn.run().await;
        });

        let client_stream = handshake_client(addr).await;
        let mut codec = FibpCodec::new(16_777_216);

        // Send auth frame with correct key.
        let auth = Frame::new(0, OP_AUTH, 1, Bytes::from_static(b"test-key-456"));
        let mut send_buf = BytesMut::new();
        codec.encode(auth, &mut send_buf).unwrap();

        let (mut read_half, mut write_half) = client_stream.into_split();
        write_half.write_all(&send_buf).await.unwrap();

        // Read auth success response.
        let mut recv_buf = BytesMut::new();
        loop {
            let mut tmp = [0u8; 1024];
            let n = read_half.read(&mut tmp).await.unwrap();
            recv_buf.extend_from_slice(&tmp[..n]);
            if let Some(frame) = codec.decode(&mut recv_buf).unwrap() {
                assert_eq!(frame.op, OP_AUTH, "expected auth response");
                assert!(frame.payload.is_empty(), "auth success has empty payload");
                break;
            }
        }

        // Now heartbeat should work (proves the connection is still alive post-auth).
        let ping = Frame::new(0, OP_HEARTBEAT, 99, Bytes::new());
        send_buf.clear();
        codec.encode(ping, &mut send_buf).unwrap();
        write_half.write_all(&send_buf).await.unwrap();

        loop {
            let mut tmp = [0u8; 1024];
            let n = read_half.read(&mut tmp).await.unwrap();
            recv_buf.extend_from_slice(&tmp[..n]);
            if let Some(frame) = codec.decode(&mut recv_buf).unwrap() {
                assert_eq!(frame.op, OP_HEARTBEAT);
                assert_eq!(frame.correlation_id, 99);
                break;
            }
        }

        drop(write_half);
        drop(read_half);
        server.await.unwrap();
    }

    #[tokio::test]
    async fn auth_failure_closes_connection() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let broker = test_broker_with_auth("correct-key");

        let server = tokio::spawn(async move {
            let (stream, _) = listener.accept().await.unwrap();
            let conn = FibpConnection::accept(stream, 16_777_216, broker, None)
                .await
                .unwrap();
            conn.run().await;
        });

        let client_stream = handshake_client(addr).await;
        let mut codec = FibpCodec::new(16_777_216);

        // Send auth frame with wrong key.
        let auth = Frame::new(0, OP_AUTH, 1, Bytes::from_static(b"wrong-key"));
        let mut send_buf = BytesMut::new();
        codec.encode(auth, &mut send_buf).unwrap();

        let (mut read_half, mut write_half) = client_stream.into_split();
        write_half.write_all(&send_buf).await.unwrap();

        // Read error frame.
        let mut recv_buf = BytesMut::new();
        loop {
            let mut tmp = [0u8; 1024];
            let n = read_half.read(&mut tmp).await.unwrap();
            if n == 0 {
                break; // Connection closed
            }
            recv_buf.extend_from_slice(&tmp[..n]);
            if let Some(frame) = codec.decode(&mut recv_buf).unwrap() {
                assert_eq!(frame.op, OP_ERROR, "expected error frame");
                let msg = String::from_utf8_lossy(&frame.payload);
                assert!(msg.contains("invalid"), "expected auth failure, got: {msg}");
                break;
            }
        }

        drop(write_half);
        drop(read_half);
        server.await.unwrap();
    }

    #[tokio::test]
    async fn admin_create_queue_over_fibp() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let broker = test_broker();

        let server = tokio::spawn(async move {
            let (stream, _) = listener.accept().await.unwrap();
            let conn = FibpConnection::accept(stream, 16_777_216, broker, None)
                .await
                .unwrap();
            conn.run().await;
        });

        let client_stream = handshake_client(addr).await;
        let mut codec = FibpCodec::new(16_777_216);

        // Send a CreateQueue admin frame with protobuf payload.
        use prost::Message as ProstMsg;
        let create_req = fila_proto::CreateQueueRequest {
            name: "fibp-admin-queue".to_string(),
            config: None,
        };
        let payload = Bytes::from(create_req.encode_to_vec());
        let frame = Frame::new(0, OP_CREATE_QUEUE, 10, payload);
        let mut send_buf = BytesMut::new();
        codec.encode(frame, &mut send_buf).unwrap();

        let (mut read_half, mut write_half) = client_stream.into_split();
        write_half.write_all(&send_buf).await.unwrap();

        // Read the response.
        let mut recv_buf = BytesMut::new();
        loop {
            let mut tmp = [0u8; 1024];
            let n = read_half.read(&mut tmp).await.unwrap();
            recv_buf.extend_from_slice(&tmp[..n]);
            if let Some(frame) = codec.decode(&mut recv_buf).unwrap() {
                assert_eq!(frame.op, OP_CREATE_QUEUE);
                assert_eq!(frame.correlation_id, 10);
                // Decode protobuf response.
                let resp = fila_proto::CreateQueueResponse::decode(frame.payload).unwrap();
                assert_eq!(resp.queue_id, "fibp-admin-queue");
                break;
            }
        }

        drop(write_half);
        drop(read_half);
        server.await.unwrap();
    }

    #[tokio::test]
    async fn admin_list_queues_over_fibp() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let broker = test_broker();

        // Pre-create a queue via the scheduler.
        let (tx, rx) = tokio::sync::oneshot::channel();
        broker
            .send_command(crate::SchedulerCommand::CreateQueue {
                name: "list-test".to_string(),
                config: crate::QueueConfig::new("list-test".to_string()),
                reply: tx,
            })
            .unwrap();
        rx.await.unwrap().unwrap();

        let server = tokio::spawn(async move {
            let (stream, _) = listener.accept().await.unwrap();
            let conn = FibpConnection::accept(stream, 16_777_216, broker, None)
                .await
                .unwrap();
            conn.run().await;
        });

        let client_stream = handshake_client(addr).await;
        let mut codec = FibpCodec::new(16_777_216);

        // Send ListQueues.
        use prost::Message as ProstMsg;
        let list_req = fila_proto::ListQueuesRequest {};
        let payload = Bytes::from(list_req.encode_to_vec());
        let frame = Frame::new(0, OP_LIST_QUEUES, 11, payload);
        let mut send_buf = BytesMut::new();
        codec.encode(frame, &mut send_buf).unwrap();

        let (mut read_half, mut write_half) = client_stream.into_split();
        write_half.write_all(&send_buf).await.unwrap();

        let mut recv_buf = BytesMut::new();
        loop {
            let mut tmp = [0u8; 4096];
            let n = read_half.read(&mut tmp).await.unwrap();
            recv_buf.extend_from_slice(&tmp[..n]);
            if let Some(frame) = codec.decode(&mut recv_buf).unwrap() {
                assert_eq!(frame.op, OP_LIST_QUEUES);
                assert_eq!(frame.correlation_id, 11);
                let resp = fila_proto::ListQueuesResponse::decode(frame.payload).unwrap();
                let names: Vec<&str> = resp.queues.iter().map(|q| q.name.as_str()).collect();
                assert!(
                    names.contains(&"list-test"),
                    "expected list-test in {names:?}"
                );
                break;
            }
        }

        drop(write_half);
        drop(read_half);
        server.await.unwrap();
    }
}
