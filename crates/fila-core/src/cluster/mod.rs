pub mod grpc_service;
pub mod multi_raft;
pub mod network;
pub mod proto_convert;
pub mod store;
#[cfg(test)]
mod tests;
pub mod types;

pub use store::MetaStoreEvent;
pub use types::{ClusterRequest, ClusterResponse, NodeId, TypeConfig};

use std::collections::BTreeMap;
use std::sync::Arc;

use openraft::error::ClientWriteError;
use openraft::error::RaftError;
use openraft::storage::Adaptor;
use openraft::{BasicNode, Config, Raft};
use tokio::task::JoinHandle;
use tonic::transport::Server;
use tracing::info;

use tonic::transport::ClientTlsConfig;

use crate::broker::config::{ClusterConfig, TlsParams};
use crate::storage::RaftKeyValueStore;
use fila_proto::fila_cluster_server::FilaClusterServer;
use grpc_service::ClusterGrpcService;
use multi_raft::MultiRaftManager;
use network::FilaNetworkFactory;
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
    /// TLS config for outgoing cluster connections. `None` when TLS is disabled.
    tls: Option<Arc<ClientTlsConfig>>,
    /// Cached gRPC clients for forwarding writes to leader nodes.
    /// Avoids opening a new HTTP/2 connection per forwarded request.
    client_cache: tokio::sync::Mutex<
        std::collections::HashMap<
            String,
            fila_proto::fila_cluster_client::FilaClusterClient<tonic::transport::Channel>,
        >,
    >,
}

/// Error from a cluster write operation (client_write or forward).
#[derive(Debug)]
pub enum ClusterWriteError {
    /// The queue's Raft group does not exist on this node.
    QueueGroupNotFound,
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
        let raft = self
            .multi_raft
            .get_raft(queue_id)
            .await
            .ok_or(ClusterWriteError::QueueGroupNotFound)?;

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

    /// Forward a client write to a specific node via the cluster gRPC
    /// `ClientWrite` RPC. Used when this node is not the Raft leader.
    /// Reuses cached gRPC connections to avoid per-request handshakes.
    async fn forward_client_write(
        &self,
        leader_addr: &str,
        group_id: &str,
        request: &ClusterRequest,
    ) -> Result<ClusterResponse, ClusterWriteError> {
        use fila_proto::fila_cluster_client::FilaClusterClient;
        use tonic::transport::Channel;

        // Determine scheme-correct URL for cache key and connection.
        let url = if self.tls.is_some() {
            if leader_addr.starts_with("https") {
                leader_addr.to_string()
            } else if leader_addr.starts_with("http://") {
                leader_addr.replacen("http://", "https://", 1)
            } else {
                format!("https://{leader_addr}")
            }
        } else if leader_addr.starts_with("http") {
            leader_addr.to_string()
        } else {
            format!("http://{leader_addr}")
        };

        // Check cache without holding the lock during connect().
        let cached = {
            let cache = self.client_cache.lock().await;
            cache.get(&url).cloned()
        };
        let client = if let Some(client) = cached {
            client
        } else {
            let channel = if let Some(ref tls) = self.tls {
                Channel::from_shared(url.clone())
                    .map_err(|e| ClusterWriteError::Forward(format!("invalid uri: {e}")))?
                    .tls_config((**tls).clone())
                    .map_err(|e| ClusterWriteError::Forward(format!("tls config: {e}")))?
                    .connect()
                    .await
                    .map_err(|e| ClusterWriteError::Forward(format!("connect: {e}")))?
            } else {
                Channel::from_shared(url.clone())
                    .map_err(|e| ClusterWriteError::Forward(format!("invalid uri: {e}")))?
                    .connect()
                    .await
                    .map_err(|e| ClusterWriteError::Forward(format!("connect: {e}")))?
            };
            let new_client = FilaClusterClient::new(channel);
            let mut cache = self.client_cache.lock().await;
            cache.entry(url).or_insert(new_client).clone()
        };

        let request_proto = fila_proto::ClusterRequestProto::from(request.clone());

        let resp = client
            .clone()
            .client_write(tonic::Request::new(fila_proto::RaftClientWriteRequest {
                request: Some(request_proto),
                group_id: group_id.to_string(),
            }))
            .await
            .map_err(|e| ClusterWriteError::Forward(format!("rpc: {e}")))?
            .into_inner();

        let response: ClusterResponse = resp
            .response
            .ok_or_else(|| ClusterWriteError::Forward("missing response".to_string()))?
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
            } => {
                // Create the queue in the local scheduler.
                let (reply_tx, reply_rx) = tokio::sync::oneshot::channel();
                if let Err(e) = broker.send_command(crate::SchedulerCommand::CreateQueue {
                    name: queue_id.clone(),
                    config: *config,
                    reply: reply_tx,
                }) {
                    tracing::error!(queue_id, error = %e, "failed to send create queue command");
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
                if let Err(e) = multi_raft.create_group(&queue_id, &members).await {
                    tracing::error!(queue_id, error = %e, "failed to create queue raft group");
                }
            }
            MetaStoreEvent::QueueGroupDeleted { queue_id } => {
                // Remove the queue's Raft group.
                multi_raft.remove_group(&queue_id).await;

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

/// Build a `ClientTlsConfig` from `TlsParams`.
/// The CA cert is required for peer verification; client cert/key are used for mTLS.
async fn build_client_tls(
    tls: Option<&TlsParams>,
) -> Result<Option<ClientTlsConfig>, Box<dyn std::error::Error>> {
    let tls = match tls {
        Some(t) => t,
        None => return Ok(None),
    };
    let cert_pem = tokio::fs::read(&tls.cert_file).await?;
    let key_pem = tokio::fs::read(&tls.key_file).await?;
    let identity = tonic::transport::Identity::from_pem(cert_pem, key_pem);
    let mut client_tls = ClientTlsConfig::new().identity(identity);
    if let Some(ref ca_file) = tls.ca_file {
        let ca_pem = tokio::fs::read(ca_file).await?;
        client_tls = client_tls.ca_certificate(tonic::transport::Certificate::from_pem(ca_pem));
    }
    Ok(Some(client_tls))
}

/// Build a `ServerTlsConfig` from `TlsParams`.
/// `ca_file` is optional: present → mTLS (verify client certs); absent → server-TLS only.
async fn build_server_tls(
    tls: Option<&TlsParams>,
) -> Result<Option<tonic::transport::ServerTlsConfig>, Box<dyn std::error::Error>> {
    let tls = match tls {
        Some(t) => t,
        None => return Ok(None),
    };
    let cert_pem = tokio::fs::read(&tls.cert_file).await?;
    let key_pem = tokio::fs::read(&tls.key_file).await?;
    let identity = tonic::transport::Identity::from_pem(cert_pem, key_pem);
    let mut server_tls = tonic::transport::ServerTlsConfig::new().identity(identity);
    if let Some(ref ca_file) = tls.ca_file {
        let ca_pem = tokio::fs::read(ca_file).await?;
        server_tls = server_tls.client_ca_root(tonic::transport::Certificate::from_pem(ca_pem));
    }
    Ok(Some(server_tls))
}

/// Manages the Raft lifecycle: creates the meta Raft instance, starts the
/// cluster gRPC service, bootstraps or joins the cluster, and manages
/// per-queue Raft groups via `MultiRaftManager`.
pub struct ClusterManager {
    node_id: NodeId,
    raft: Arc<Raft<TypeConfig>>,
    multi_raft: Arc<MultiRaftManager>,
    grpc_handle: JoinHandle<()>,
    shutdown_tx: tokio::sync::oneshot::Sender<()>,
    /// Shared slot for wiring the Broker to the cluster gRPC service
    /// after Broker creation. The gRPC service uses this to apply
    /// forwarded writes to the leader's local scheduler.
    broker_slot: Arc<std::sync::OnceLock<Arc<crate::Broker>>>,
    /// TLS config for outgoing cluster connections. Propagated to `ClusterHandle`.
    client_tls: Option<Arc<ClientTlsConfig>>,
}

impl ClusterManager {
    /// Create and start the cluster manager.
    ///
    /// This initializes the meta Raft instance, starts the cluster gRPC service
    /// on `config.bind_addr`, and either bootstraps a new cluster or joins an
    /// existing one.
    ///
    /// When `tls_config` is provided and `tls_config.enabled` is true, the
    /// cluster gRPC server and all outgoing peer connections use mTLS.
    pub async fn start(
        config: &ClusterConfig,
        db: Arc<dyn RaftKeyValueStore>,
        broker_storage: Arc<dyn crate::storage::StorageEngine>,
        meta_event_tx: Option<tokio::sync::mpsc::UnboundedSender<MetaStoreEvent>>,
        tls_config: Option<&TlsParams>,
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

        // Build optional TLS configs for the cluster server and outgoing connections.
        let client_tls = build_client_tls(tls_config).await?;
        let server_tls = build_server_tls(tls_config).await?;
        let client_tls = client_tls.map(Arc::new);

        let store = FilaRaftStore::new(Arc::clone(&db), meta_event_tx);
        let (log_store, state_machine) = Adaptor::new(store);

        let network = FilaNetworkFactory::meta_with_tls(client_tls.clone());

        let raft = Raft::new(
            node_id,
            Arc::clone(&raft_config),
            network,
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

        // Start cluster gRPC service.
        let broker_slot = Arc::new(std::sync::OnceLock::new());
        let service = ClusterGrpcService::new(
            Arc::clone(&raft),
            Arc::clone(&multi_raft),
            Arc::clone(&broker_slot),
        );
        let bind_addr: std::net::SocketAddr = config.bind_addr.parse()?;
        let (shutdown_tx, shutdown_rx) = tokio::sync::oneshot::channel::<()>();

        info!(%bind_addr, node_id, "starting cluster gRPC service");

        let grpc_handle = tokio::spawn(async move {
            let mut builder = Server::builder();
            if let Some(tls) = server_tls {
                match builder.tls_config(tls) {
                    Ok(b) => builder = b,
                    Err(e) => {
                        tracing::error!("cluster gRPC TLS config error: {e}");
                        return;
                    }
                }
            }
            if let Err(e) = builder
                .add_service(FilaClusterServer::new(service))
                .serve_with_shutdown(bind_addr, async {
                    let _ = shutdown_rx.await;
                })
                .await
            {
                tracing::error!("cluster gRPC service error: {e}");
            }
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
            Self::join_cluster(node_id, &config.bind_addr, &config.peers, client_tls.clone()).await?;
        }

        Ok(Self {
            node_id,
            raft,
            multi_raft,
            grpc_handle,
            shutdown_tx,
            broker_slot,
            client_tls,
        })
    }

    /// Join an existing cluster by contacting seed peers.
    async fn join_cluster(
        node_id: u64,
        bind_addr: &str,
        peers: &[String],
        tls: Option<Arc<ClientTlsConfig>>,
    ) -> Result<(), Box<dyn std::error::Error>> {
        use fila_proto::fila_cluster_client::FilaClusterClient;
        use fila_proto::AddNodeRequest;
        use tonic::transport::Channel;

        let req = AddNodeRequest {
            node_id,
            addr: bind_addr.to_string(),
        };

        for peer in peers {
            let url = if tls.is_some() {
                if peer.starts_with("https") {
                    peer.clone()
                } else if peer.starts_with("http://") {
                    peer.replacen("http://", "https://", 1)
                } else {
                    format!("https://{peer}")
                }
            } else if peer.starts_with("http") {
                peer.clone()
            } else {
                format!("http://{peer}")
            };

            let endpoint = match Channel::from_shared(url.clone()) {
                Ok(e) => e,
                Err(e) => {
                    tracing::warn!(peer, error = %e, "invalid peer address");
                    continue;
                }
            };
            let endpoint = if let Some(ref t) = tls {
                match endpoint.tls_config((**t).clone()) {
                    Ok(e) => e,
                    Err(e) => {
                        tracing::warn!(peer, error = %e, "tls config error for peer");
                        continue;
                    }
                }
            } else {
                endpoint
            };
            let channel = match endpoint.connect().await {
                Ok(c) => c,
                Err(e) => {
                    tracing::warn!(peer, error = %e, "failed to connect to peer");
                    continue;
                }
            };
            let mut client = FilaClusterClient::new(channel);

            match client.add_node(tonic::Request::new(req.clone())).await {
                Ok(resp) => {
                    let resp = resp.into_inner();
                    if resp.success {
                        info!(node_id, peer, "successfully joined cluster");
                        return Ok(());
                    }

                    // If not leader, try the leader address.
                    if !resp.leader_addr.is_empty() {
                        let leader_url = if tls.is_some() {
                            if resp.leader_addr.starts_with("https") {
                                resp.leader_addr.clone()
                            } else if resp.leader_addr.starts_with("http://") {
                                resp.leader_addr.replacen("http://", "https://", 1)
                            } else {
                                format!("https://{}", resp.leader_addr)
                            }
                        } else if resp.leader_addr.starts_with("http") {
                            resp.leader_addr.clone()
                        } else {
                            format!("http://{}", resp.leader_addr)
                        };

                        let leader_endpoint = match Channel::from_shared(leader_url.clone()) {
                            Ok(e) => e,
                            Err(e) => {
                                tracing::warn!(error = %e, "invalid leader address");
                                continue;
                            }
                        };
                        let leader_endpoint = if let Some(ref t) = tls {
                            match leader_endpoint.tls_config((**t).clone()) {
                                Ok(e) => e,
                                Err(e) => {
                                    tracing::warn!(error = %e, "tls config error for leader");
                                    continue;
                                }
                            }
                        } else {
                            leader_endpoint
                        };
                        match leader_endpoint.connect().await {
                            Ok(leader_channel) => {
                                let mut leader_client = FilaClusterClient::new(leader_channel);
                                match leader_client
                                    .add_node(tonic::Request::new(req.clone()))
                                    .await
                                {
                                    Ok(resp) => {
                                        let resp = resp.into_inner();
                                        if resp.success {
                                            info!(node_id, "joined cluster via leader redirect");
                                            return Ok(());
                                        }
                                        tracing::warn!(error = resp.error, "failed to join via leader");
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

    /// Wire the broker to the cluster gRPC service so forwarded writes
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
            tls: self.client_tls.clone(),
            client_cache: tokio::sync::Mutex::new(std::collections::HashMap::new()),
        })
    }

    /// Gracefully shut down all Raft instances and the cluster gRPC service.
    pub async fn shutdown(self) {
        info!("shutting down cluster manager");
        // Shut down queue Raft groups first, then the meta group.
        self.multi_raft.shutdown_all().await;
        if let Err(e) = self.raft.shutdown().await {
            tracing::error!("raft shutdown error: {e:?}");
        }
        let _ = self.shutdown_tx.send(());
        let _ = self.grpc_handle.await;
    }
}
