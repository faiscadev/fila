pub mod binary_service;
pub mod grpc_service;
pub mod multi_raft;
pub mod network;
pub mod proto_convert;
pub mod store;
#[cfg(test)]
mod tests;
pub mod types;

pub use store::MetaStoreEvent;
pub use types::{AckItemData, ClusterRequest, ClusterResponse, NackItemData, NodeId, TypeConfig};

use std::collections::BTreeMap;
use std::sync::Arc;

use bytes::BytesMut;
use fila_fibp::ProtocolMessage;
use openraft::error::ClientWriteError;
use openraft::error::RaftError;
use openraft::storage::Adaptor;
use openraft::{BasicNode, Config, Raft};
#[allow(unused_imports)]
use prost::Message as _;
use tokio::task::JoinHandle;
use tracing::info;

use crate::broker::config::{ClusterConfig, TlsParams};
use crate::storage::RaftKeyValueStore;
use binary_service::ClusterBinaryService;
use multi_raft::MultiRaftManager;
use network::{ClusterTlsConfig, FilaNetworkFactory};
use store::FilaRaftStore;

/// Shareable handle to cluster resources. Services hold this to check
/// cluster mode and route requests through Raft consensus.
///
/// This is separate from `ClusterManager` because the manager owns
/// non-cloneable resources (oneshot shutdown channel) while the handle
/// only holds Arc-wrapped, freely cloneable references.
pub struct ClusterHandle {
    pub meta_raft: Arc<Raft<TypeConfig>>,
    pub multi_raft: Arc<MultiRaftManager>,
    pub node_id: NodeId,
    /// Number of nodes per queue Raft group. When the cluster has more nodes,
    /// only a subset is selected for each queue.
    pub replication_factor: usize,
    /// TLS config for outgoing cluster connections. `None` when TLS is disabled.
    tls: Option<Arc<ClusterTlsConfig>>,
    /// Cached binary protocol connections for forwarding writes to leader nodes.
    /// Avoids opening a new TCP connection per forwarded request.
    client_cache: tokio::sync::Mutex<
        std::collections::HashMap<String, Arc<tokio::sync::Mutex<Option<network::PeerStream>>>>,
    >,
}

/// Error from a cluster write operation (client_write or forward).
#[derive(Debug)]
pub enum ClusterWriteError {
    /// The queue's Raft group does not exist anywhere in the cluster.
    QueueGroupNotFound,
    /// The queue exists in the cluster but this node's Raft group is not
    /// ready yet (still catching up or being created). Clients should retry
    /// on another node or wait.
    NodeNotReady,
    /// No leader is currently known for the target Raft group.
    NoLeader,
    /// Raft consensus error.
    Raft(String),
    /// Error forwarding request to the leader node.
    Forward(String),
}

impl std::fmt::Display for ClusterWriteError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::QueueGroupNotFound => write!(f, "queue raft group not found"),
            Self::NodeNotReady => write!(f, "node not ready for this queue"),
            Self::NoLeader => write!(f, "no leader available"),
            Self::Raft(e) => write!(f, "raft error: {e}"),
            Self::Forward(e) => write!(f, "forward error: {e}"),
        }
    }
}

impl std::error::Error for ClusterWriteError {}

/// Result of a cluster write operation.
#[derive(Debug)]
pub struct ClusterWriteResult {
    pub response: ClusterResponse,
    /// True if this node handled the write locally (is the leader).
    /// False if the write was forwarded to another node.
    pub handled_locally: bool,
}

impl ClusterHandle {
    /// Submit a write to a queue's Raft group. If this node is not the
    /// leader, the request is transparently forwarded to the leader via
    /// the cluster gRPC `ClientWrite` RPC.
    ///
    /// Returns a `ClusterWriteResult` indicating whether the write was
    /// handled locally (caller should apply to scheduler) or forwarded
    /// (the leader already applied to its scheduler).
    pub async fn write_to_queue(
        &self,
        queue_id: &str,
        request: ClusterRequest,
    ) -> Result<ClusterWriteResult, ClusterWriteError> {
        let raft = match self.multi_raft.get_raft(queue_id).await {
            Some(raft) => raft,
            None => {
                return if self.multi_raft.is_queue_expected(queue_id).await {
                    Err(ClusterWriteError::NodeNotReady)
                } else {
                    Err(ClusterWriteError::QueueGroupNotFound)
                };
            }
        };

        match raft.client_write(request.clone()).await {
            Ok(resp) => Ok(ClusterWriteResult {
                response: resp.data,
                handled_locally: true,
            }),
            Err(RaftError::APIError(ClientWriteError::ForwardToLeader(fwd))) => {
                let leader_addr = fwd
                    .leader_node
                    .map(|n| n.addr)
                    .ok_or(ClusterWriteError::NoLeader)?;
                let response = self
                    .forward_client_write(&leader_addr, queue_id, &request)
                    .await?;
                Ok(ClusterWriteResult {
                    response,
                    handled_locally: false,
                })
            }
            Err(RaftError::APIError(ClientWriteError::ChangeMembershipError(e))) => {
                Err(ClusterWriteError::Raft(format!("{e}")))
            }
            Err(RaftError::Fatal(e)) => Err(ClusterWriteError::Raft(format!("fatal: {e}"))),
        }
    }

    /// Submit a write to the meta Raft group (cluster-wide coordination).
    /// Transparently forwards to the meta leader if this node is not it.
    pub async fn write_to_meta(
        &self,
        request: ClusterRequest,
    ) -> Result<ClusterResponse, ClusterWriteError> {
        match self.meta_raft.client_write(request.clone()).await {
            Ok(resp) => Ok(resp.data),
            Err(RaftError::APIError(ClientWriteError::ForwardToLeader(fwd))) => {
                let leader_addr = fwd
                    .leader_node
                    .map(|n| n.addr)
                    .ok_or(ClusterWriteError::NoLeader)?;
                // Empty group_id means meta Raft.
                self.forward_client_write(&leader_addr, "", &request).await
            }
            Err(RaftError::APIError(ClientWriteError::ChangeMembershipError(e))) => {
                Err(ClusterWriteError::Raft(format!("{e}")))
            }
            Err(RaftError::Fatal(e)) => Err(ClusterWriteError::Raft(format!("fatal: {e}"))),
        }
    }

    /// Check if this node is the leader for a queue's Raft group.
    pub async fn is_queue_leader(&self, queue_id: &str) -> Option<bool> {
        let raft = self.multi_raft.get_raft(queue_id).await?;
        let leader = raft.current_leader().await;
        Some(leader == Some(self.node_id))
    }

    /// Get the leader's client-facing gRPC address for a queue. Returns `None`
    /// if the queue group doesn't exist, no leader is elected, or the leader's
    /// client address is unknown.
    pub async fn get_queue_leader_client_addr(&self, queue_id: &str) -> Option<String> {
        let raft = self.multi_raft.get_raft(queue_id).await?;
        let leader_id = raft.current_leader().await?;
        self.multi_raft.get_client_addr(leader_id).await
    }

    /// Get the current meta Raft membership as member IDs and addresses.
    pub fn meta_members(&self) -> (Vec<u64>, std::collections::HashMap<u64, String>) {
        let metrics = self.meta_raft.metrics().borrow().clone();
        let members: Vec<u64> = metrics.membership_config.membership().voter_ids().collect();
        let addrs: std::collections::HashMap<u64, String> = metrics
            .membership_config
            .membership()
            .nodes()
            .map(|(&id, n)| (id, n.addr.clone()))
            .collect();
        (members, addrs)
    }

    /// Forward a client write to a specific node via the binary protocol.
    /// Used when this node is not the Raft leader.
    /// Reuses cached TCP connections to avoid per-request handshakes.
    async fn forward_client_write(
        &self,
        leader_addr: &str,
        group_id: &str,
        request: &ClusterRequest,
    ) -> Result<ClusterResponse, ClusterWriteError> {
        // Get or create a cached connection.
        let stream_slot = {
            let mut cache = self.client_cache.lock().await;
            cache
                .entry(leader_addr.to_string())
                .or_insert_with(|| Arc::new(tokio::sync::Mutex::new(None)))
                .clone()
        };

        let request_proto = fila_proto::ClusterRequestProto::from(request.clone());
        let data = {
            let mut buf = Vec::with_capacity(request_proto.encoded_len());
            prost::Message::encode(&request_proto, &mut buf)
                .map_err(|e| ClusterWriteError::Forward(format!("encode: {e}")))?;
            buf
        };

        let req_frame = fila_fibp::ClusterClientWriteRequest {
            group_id: group_id.to_string(),
            data,
        }
        .encode(1);

        let resp_frame = {
            let mut guard = stream_slot.lock().await;
            // Connect if no cached connection.
            if guard.is_none() {
                let stream = network::connect_peer(leader_addr, self.tls.as_deref())
                    .await
                    .map_err(|e| ClusterWriteError::Forward(format!("connect: {e}")))?;
                *guard = Some(stream);
            }

            let stream = guard.as_mut().expect("stream just initialized");
            let result = send_recv_peer(stream, req_frame).await;

            match result {
                Ok(frame) => frame,
                Err(e) => {
                    // Clear cached stream so next call reconnects.
                    *guard = None;
                    return Err(ClusterWriteError::Forward(format!("rpc: {e}")));
                }
            }
        };

        // Check for error frame.
        if resp_frame.opcode == fila_fibp::Opcode::Error as u8 {
            let err_msg = fila_fibp::ErrorFrame::decode(resp_frame.payload)
                .map(|e| e.message)
                .unwrap_or_else(|_| "unknown error".to_string());
            return Err(ClusterWriteError::Forward(err_msg));
        }

        let resp_data = fila_fibp::ClusterClientWriteResponse::decode(resp_frame.payload)
            .map_err(|e| ClusterWriteError::Forward(format!("decode: {e}")))?;

        let proto_resp: fila_proto::ClusterResponseProto = prost::Message::decode(&*resp_data.data)
            .map_err(|e| ClusterWriteError::Forward(format!("proto: {e}")))?;

        let response: ClusterResponse =
            proto_resp
                .try_into()
                .map_err(|e: proto_convert::ConvertError| {
                    ClusterWriteError::Forward(format!("deserialize: {e}"))
                })?;

        // Check if the response is a ForwardToLeader error.
        if let ClusterResponse::Error { ref message } = response {
            return Err(ClusterWriteError::Forward(message.clone()));
        }

        Ok(response)
    }
}

/// Send a frame and read the response over a peer stream (for forward_client_write).
async fn send_recv_peer(
    stream: &mut network::PeerStream,
    frame: fila_fibp::RawFrame,
) -> Result<fila_fibp::RawFrame, Box<dyn std::error::Error + Send + Sync>> {
    let mut out = BytesMut::new();
    frame.encode(&mut out);
    stream.write_all(&out).await?;
    stream.flush().await?;

    let mut buf = BytesMut::with_capacity(8192);
    loop {
        if let Some(resp) = fila_fibp::RawFrame::decode(&mut buf)? {
            return Ok(resp);
        }
        let n = stream.read_buf(&mut buf).await?;
        if n == 0 {
            return Err("connection closed".into());
        }
    }
}

/// Process meta Raft state machine events (queue group lifecycle).
///
/// This runs as a background task. When the meta Raft commits a
/// `CreateQueueGroup` or `DeleteQueueGroup` entry, the state machine
/// sends an event through the channel. This handler creates/removes
/// the queue in the local scheduler and starts/stops the queue's Raft group.
pub async fn process_meta_events(
    mut rx: tokio::sync::mpsc::UnboundedReceiver<MetaStoreEvent>,
    broker: Arc<crate::Broker>,
    multi_raft: Arc<MultiRaftManager>,
) {
    while let Some(event) = rx.recv().await {
        match event {
            MetaStoreEvent::QueueGroupCreated {
                queue_id,
                members,
                config,
                preferred_leader,
            } => {
                // Mark the queue as expected before creating the group so that
                // requests arriving during creation get NodeNotReady (not NotFound).
                multi_raft.mark_queue_expected(&queue_id).await;

                // Create the queue in the local scheduler.
                let (reply_tx, reply_rx) = tokio::sync::oneshot::channel();
                if let Err(e) = broker.send_command(crate::SchedulerCommand::CreateQueue {
                    name: queue_id.clone(),
                    config: *config,
                    reply: reply_tx,
                }) {
                    tracing::error!(queue_id, error = %e, "failed to send create queue command");
                    // Keep expected_queues marked: the queue exists cluster-wide
                    // even if local setup fails. Clients should get NodeNotReady
                    // (retry on another node), not QueueGroupNotFound.
                    continue;
                }
                match reply_rx.await {
                    Ok(Ok(_)) => {}
                    Ok(Err(e)) => {
                        // Queue might already exist (restart scenario) — log and continue.
                        tracing::warn!(queue_id, error = %e, "create queue via meta event");
                    }
                    Err(_) => {
                        tracing::error!(queue_id, "scheduler dropped reply for create queue");
                        continue;
                    }
                }

                // Start the queue's Raft group.
                if let Err(e) = multi_raft
                    .create_group(&queue_id, &members, preferred_leader)
                    .await
                {
                    tracing::error!(queue_id, error = %e, "failed to create queue raft group");
                }
            }
            MetaStoreEvent::QueueGroupDeleted { queue_id } => {
                // Remove the queue's Raft group.
                multi_raft.remove_group(&queue_id).await;
                multi_raft.unmark_queue_expected(&queue_id).await;

                // Delete the queue from the local scheduler.
                let (reply_tx, reply_rx) = tokio::sync::oneshot::channel();
                if let Err(e) = broker.send_command(crate::SchedulerCommand::DeleteQueue {
                    queue_id: queue_id.clone(),
                    reply: reply_tx,
                }) {
                    tracing::error!(queue_id, error = %e, "failed to send delete queue command");
                    continue;
                }
                match reply_rx.await {
                    Ok(Ok(_)) | Ok(Err(_)) => {}
                    Err(_) => {
                        tracing::error!(queue_id, "scheduler dropped reply for delete queue");
                    }
                }
            }
        }
    }
}

/// Monitors queue-level Raft groups for leadership changes.
///
/// When this node becomes leader for a queue, it sends a `RecoverQueue`
/// command to the broker's scheduler so in-memory state (DRR keys, pending
/// index) is rebuilt from RocksDB. When this node loses leadership, it
/// sends a `DropQueueConsumers` command so consumer streams are closed and
/// clients reconnect to the new leader.
///
/// Runs as a background task, polling every `poll_interval`.
pub async fn watch_leader_changes(
    node_id: NodeId,
    multi_raft: Arc<MultiRaftManager>,
    broker: Arc<crate::Broker>,
    poll_interval: std::time::Duration,
    mut shutdown: tokio::sync::watch::Receiver<bool>,
) {
    use std::collections::HashMap;

    // Tracks which queues this node currently leads.
    let mut leading: HashMap<String, bool> = HashMap::new();

    loop {
        tokio::select! {
            _ = tokio::time::sleep(poll_interval) => {}
            _ = shutdown.changed() => {
                if *shutdown.borrow() {
                    info!("leader change watcher shutting down");
                    return;
                }
            }
        }

        let groups = multi_raft.snapshot_groups().await;

        // Track which queues we've seen this cycle (to detect removed groups).
        let mut seen = std::collections::HashSet::new();

        for (queue_id, raft) in &groups {
            seen.insert(queue_id.clone());
            let current_leader = raft.current_leader().await;
            let is_leader = current_leader == Some(node_id);
            let was_leader = leading.get(queue_id).copied().unwrap_or(false);

            if is_leader && !was_leader {
                // This node just became leader (or is leader on first sight).
                info!(queue_id, "became leader — triggering queue recovery");
                match broker.send_command(crate::SchedulerCommand::RecoverQueue {
                    queue_id: queue_id.clone(),
                    reply: None,
                }) {
                    Ok(_) => {
                        leading.insert(queue_id.clone(), true);
                    }
                    Err(e) => {
                        // Don't update leading — next poll will retry.
                        tracing::error!(queue_id, error = %e, "failed to send RecoverQueue");
                    }
                }
            } else if !is_leader && was_leader {
                // This node just lost leadership for this queue.
                info!(queue_id, "lost leadership — dropping consumer streams");
                match broker.send_command(crate::SchedulerCommand::DropQueueConsumers {
                    queue_id: queue_id.clone(),
                }) {
                    Ok(_) => {
                        leading.insert(queue_id.clone(), false);
                    }
                    Err(e) => {
                        // Don't update leading — next poll will retry.
                        tracing::error!(queue_id, error = %e, "failed to send DropQueueConsumers");
                    }
                }
            } else if !leading.contains_key(queue_id) {
                // First time seeing this queue group — record current state
                // and trigger recovery if already leader so the scheduler
                // catches any messages replicated between startup and now.
                if is_leader {
                    info!(queue_id, "first-sight leader — triggering queue recovery");
                    match broker.send_command(crate::SchedulerCommand::RecoverQueue {
                        queue_id: queue_id.clone(),
                        reply: None,
                    }) {
                        Ok(_) => {
                            leading.insert(queue_id.clone(), true);
                        }
                        Err(e) => {
                            tracing::error!(queue_id, error = %e, "failed to send RecoverQueue on first sight");
                        }
                    }
                } else {
                    leading.insert(queue_id.clone(), false);
                }
            }
        }

        // Clean up entries for removed queue groups.
        leading.retain(|qid, _| seen.contains(qid));
    }
}

/// Build a `ClusterTlsConfig` (for outgoing connections) from `TlsParams`.
/// Client cert/key are always set (used as the mTLS identity). CA cert is optional:
/// when present it is used for peer verification; when absent the system trust store is used.
async fn build_client_tls(
    tls: Option<&TlsParams>,
) -> Result<Option<ClusterTlsConfig>, Box<dyn std::error::Error>> {
    let tls = match tls {
        Some(t) => t,
        None => return Ok(None),
    };
    let cert_pem = tokio::fs::read(&tls.cert_file).await?;
    let key_pem = tokio::fs::read(&tls.key_file).await?;

    let certs = rustls_pemfile::certs(&mut &*cert_pem).collect::<Result<Vec<_>, _>>()?;
    let key = rustls_pemfile::private_key(&mut &*key_pem)?.ok_or("no private key found in PEM")?;

    let mut root_store = rustls::RootCertStore::empty();
    if let Some(ref ca_file) = tls.ca_file {
        let ca_pem = tokio::fs::read(ca_file).await?;
        let ca_certs = rustls_pemfile::certs(&mut &*ca_pem).collect::<Result<Vec<_>, _>>()?;
        for cert in ca_certs {
            root_store.add(cert)?;
        }
    }

    let config = rustls::ClientConfig::builder()
        .with_root_certificates(root_store)
        .with_client_auth_cert(certs, key)?;

    let connector = tokio_rustls::TlsConnector::from(Arc::new(config));

    Ok(Some(ClusterTlsConfig {
        connector,
        server_name: rustls::pki_types::ServerName::try_from("localhost")?.to_owned(),
    }))
}

/// Build a `TlsAcceptor` (for incoming connections) from `TlsParams`.
/// `ca_file` is optional: present -> mTLS (verify client certs); absent -> server-TLS only.
async fn build_server_tls(
    tls: Option<&TlsParams>,
) -> Result<Option<tokio_rustls::TlsAcceptor>, Box<dyn std::error::Error>> {
    let tls = match tls {
        Some(t) => t,
        None => return Ok(None),
    };
    let cert_pem = tokio::fs::read(&tls.cert_file).await?;
    let key_pem = tokio::fs::read(&tls.key_file).await?;

    let certs = rustls_pemfile::certs(&mut &*cert_pem).collect::<Result<Vec<_>, _>>()?;
    let key = rustls_pemfile::private_key(&mut &*key_pem)?.ok_or("no private key found in PEM")?;

    let config = if let Some(ref ca_file) = tls.ca_file {
        let ca_pem = tokio::fs::read(ca_file).await?;
        let ca_certs = rustls_pemfile::certs(&mut &*ca_pem).collect::<Result<Vec<_>, _>>()?;
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

    Ok(Some(tokio_rustls::TlsAcceptor::from(Arc::new(config))))
}

/// Manages the Raft lifecycle: creates the meta Raft instance, starts the
/// cluster binary protocol service, bootstraps or joins the cluster, and manages
/// per-queue Raft groups via `MultiRaftManager`.
pub struct ClusterManager {
    node_id: NodeId,
    raft: Arc<Raft<TypeConfig>>,
    multi_raft: Arc<MultiRaftManager>,
    service_handle: JoinHandle<()>,
    shutdown_tx: tokio::sync::watch::Sender<bool>,
    /// Shared slot for wiring the Broker to the cluster binary service
    /// after Broker creation. The binary service uses this to apply
    /// forwarded writes to the leader's local scheduler.
    broker_slot: Arc<std::sync::OnceLock<Arc<crate::Broker>>>,
    /// TLS config for outgoing cluster connections. Propagated to `ClusterHandle`.
    client_tls: Option<Arc<ClusterTlsConfig>>,
    /// Number of nodes per queue Raft group. Propagated to `ClusterHandle`.
    replication_factor: usize,
}

impl ClusterManager {
    /// Create and start the cluster manager.
    ///
    /// This initializes the meta Raft instance, starts the cluster binary
    /// protocol service on `config.bind_addr`, and either bootstraps a new
    /// cluster or joins an existing one.
    ///
    /// When `tls_config` is `Some`, TLS is enabled on the cluster TCP listener
    /// and all outgoing peer connections. If `tls_config.ca_file` is also set,
    /// client certificates are verified (mTLS); otherwise server-TLS only.
    pub async fn start(
        config: &ClusterConfig,
        db: Arc<dyn RaftKeyValueStore>,
        broker_storage: Arc<dyn crate::storage::StorageEngine>,
        meta_event_tx: Option<tokio::sync::mpsc::UnboundedSender<MetaStoreEvent>>,
        tls_config: Option<&TlsParams>,
        client_listen_addr: &str,
    ) -> Result<Self, Box<dyn std::error::Error>> {
        let node_id = config.node_id;

        let raft_config = Config {
            cluster_name: "fila".to_string(),
            heartbeat_interval: config.heartbeat_interval_ms,
            election_timeout_min: config.election_timeout_ms,
            election_timeout_max: config.election_timeout_ms * 2,
            snapshot_policy: openraft::SnapshotPolicy::LogsSinceLast(config.snapshot_threshold),
            ..Default::default()
        };
        let raft_config = Arc::new(raft_config.validate()?);

        // Build optional TLS configs for the cluster service and outgoing connections.
        let client_tls = build_client_tls(tls_config).await?;
        let server_tls = build_server_tls(tls_config).await?;
        let client_tls = client_tls.map(Arc::new);

        let store = FilaRaftStore::new(Arc::clone(&db), meta_event_tx);
        let (log_store, state_machine) = Adaptor::new(store);

        let network_factory = FilaNetworkFactory::meta_with_tls(client_tls.clone());

        let raft = Raft::new(
            node_id,
            Arc::clone(&raft_config),
            network_factory,
            log_store,
            state_machine,
        )
        .await?;
        let raft = Arc::new(raft);

        let multi_raft = Arc::new(MultiRaftManager::new(
            node_id,
            Arc::clone(&db),
            Arc::clone(&raft_config),
            broker_storage,
        ));

        // Register this node's own client address for leader hint routing.
        multi_raft
            .register_client_addr(node_id, client_listen_addr)
            .await;

        // Start cluster binary protocol service.
        let broker_slot = Arc::new(std::sync::OnceLock::new());
        let service = Arc::new(ClusterBinaryService::new(
            Arc::clone(&raft),
            Arc::clone(&multi_raft),
            Arc::clone(&broker_slot),
            node_id,
            client_listen_addr.to_string(),
        ));
        let bind_addr: std::net::SocketAddr = config.bind_addr.parse()?;
        let (shutdown_tx, shutdown_rx) = tokio::sync::watch::channel(false);

        info!(%bind_addr, node_id, "starting cluster binary protocol service");

        let listener = tokio::net::TcpListener::bind(bind_addr).await?;
        let service_handle = tokio::spawn(async move {
            binary_service::run(service, listener, server_tls, shutdown_rx).await;
        });

        // Bootstrap or join cluster based on peers list.
        if config.peers.is_empty() {
            info!(node_id, "bootstrapping single-node cluster");
            let mut members = BTreeMap::new();
            members.insert(
                node_id,
                BasicNode {
                    addr: config.bind_addr.clone(),
                },
            );
            raft.initialize(members).await?;
        } else {
            info!(node_id, peers = ?config.peers, "joining existing cluster");
            Self::join_cluster(
                node_id,
                &config.bind_addr,
                client_listen_addr,
                &config.peers,
                client_tls.clone(),
            )
            .await?;
        }

        // Discover all cluster members' client addresses via GetNodeInfo.
        // This runs after join so all nodes are known in the Raft membership.
        {
            let metrics = raft.metrics().borrow().clone();
            let nodes: Vec<(u64, String)> = metrics
                .membership_config
                .membership()
                .nodes()
                .map(|(&id, n)| (id, n.addr.clone()))
                .collect();
            for (peer_id, cluster_addr) in nodes {
                if peer_id == node_id {
                    continue; // Already registered our own address
                }
                match network::connect_peer(&cluster_addr, client_tls.as_deref()).await {
                    Ok(mut stream) => {
                        let req_frame = fila_fibp::ClusterGetNodeInfoRequest.encode(1);
                        match send_recv_peer(&mut stream, req_frame).await {
                            Ok(resp_frame) => {
                                if let Ok(info) = fila_fibp::ClusterGetNodeInfoResponse::decode(
                                    resp_frame.payload,
                                ) {
                                    if !info.client_addr.is_empty() {
                                        multi_raft
                                            .register_client_addr(info.node_id, &info.client_addr)
                                            .await;
                                    }
                                }
                            }
                            Err(e) => {
                                tracing::warn!(peer_id, error = %e, "failed to get node info");
                            }
                        }
                    }
                    Err(e) => {
                        tracing::warn!(peer_id, error = %e, "failed to connect for node info");
                    }
                }
            }
        }

        Ok(Self {
            node_id,
            raft,
            multi_raft,
            service_handle,
            shutdown_tx,
            broker_slot,
            client_tls,
            replication_factor: config.replication_factor,
        })
    }

    /// Join an existing cluster by contacting seed peers via binary protocol.
    async fn join_cluster(
        node_id: u64,
        bind_addr: &str,
        client_listen_addr: &str,
        peers: &[String],
        tls: Option<Arc<ClusterTlsConfig>>,
    ) -> Result<(), Box<dyn std::error::Error>> {
        let req = fila_fibp::ClusterAddNodeRequest {
            node_id,
            addr: bind_addr.to_string(),
            client_addr: client_listen_addr.to_string(),
        };

        for peer in peers {
            let mut stream = match network::connect_peer(peer, tls.as_deref()).await {
                Ok(s) => s,
                Err(e) => {
                    tracing::warn!(peer, error = %e, "failed to connect to peer");
                    continue;
                }
            };

            let frame = req.encode(1);
            match send_recv_peer(&mut stream, frame).await {
                Ok(resp_frame) => {
                    let resp = match fila_fibp::ClusterAddNodeResponse::decode(resp_frame.payload) {
                        Ok(r) => r,
                        Err(e) => {
                            tracing::warn!(peer, error = %e, "failed to decode add_node response");
                            continue;
                        }
                    };
                    if resp.success {
                        info!(node_id, peer, "successfully joined cluster");
                        return Ok(());
                    }

                    // If not leader, try the leader address.
                    if !resp.leader_addr.is_empty() {
                        match network::connect_peer(&resp.leader_addr, tls.as_deref()).await {
                            Ok(mut leader_stream) => {
                                let leader_frame = req.encode(2);
                                match send_recv_peer(&mut leader_stream, leader_frame).await {
                                    Ok(leader_resp_frame) => {
                                        if let Ok(leader_resp) =
                                            fila_fibp::ClusterAddNodeResponse::decode(
                                                leader_resp_frame.payload,
                                            )
                                        {
                                            if leader_resp.success {
                                                info!(
                                                    node_id,
                                                    "joined cluster via leader redirect"
                                                );
                                                return Ok(());
                                            }
                                            tracing::warn!(
                                                error = leader_resp.error,
                                                "failed to join via leader"
                                            );
                                        }
                                    }
                                    Err(e) => {
                                        tracing::warn!(error = %e, "leader add_node rpc failed");
                                    }
                                }
                            }
                            Err(e) => {
                                tracing::warn!(error = %e, "failed to connect to leader");
                            }
                        }
                    } else {
                        tracing::warn!(peer, error = resp.error, "peer rejected add_node");
                    }
                }
                Err(e) => {
                    tracing::warn!(peer, error = %e, "failed to connect to peer via rpc");
                }
            }
        }

        Err("failed to join cluster via any seed peer".into())
    }

    /// Get a reference to the meta Raft instance.
    pub fn raft(&self) -> &Arc<Raft<TypeConfig>> {
        &self.raft
    }

    /// Get a reference to the multi-Raft manager for queue-level groups.
    pub fn multi_raft(&self) -> &Arc<MultiRaftManager> {
        &self.multi_raft
    }

    /// Wire the broker to the cluster binary service so forwarded writes
    /// can be applied to the leader's local scheduler. Must be called
    /// after Broker creation and before any client traffic.
    pub fn set_broker(&self, broker: Arc<crate::Broker>) {
        let _ = self.broker_slot.set(broker);
    }

    /// Create a shareable `ClusterHandle` for use by service handlers.
    pub fn handle(&self) -> Arc<ClusterHandle> {
        Arc::new(ClusterHandle {
            meta_raft: Arc::clone(&self.raft),
            multi_raft: Arc::clone(&self.multi_raft),
            node_id: self.node_id,
            replication_factor: self.replication_factor,
            tls: self.client_tls.clone(),
            client_cache: tokio::sync::Mutex::new(std::collections::HashMap::new()),
        })
    }

    /// Gracefully shut down all Raft instances and the cluster binary service.
    pub async fn shutdown(self) {
        info!("shutting down cluster manager");
        // Shut down queue Raft groups first, then the meta group.
        self.multi_raft.shutdown_all().await;
        if let Err(e) = self.raft.shutdown().await {
            tracing::error!("raft shutdown error: {e:?}");
        }
        let _ = self.shutdown_tx.send(true);
        let _ = self.service_handle.await;
    }
}
