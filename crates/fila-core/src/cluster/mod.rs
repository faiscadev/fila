pub mod grpc_service;
pub mod network;
pub mod store;
#[cfg(test)]
mod tests;
pub mod types;

pub use types::{ClusterRequest, ClusterResponse, NodeId, TypeConfig};

use std::collections::BTreeMap;
use std::sync::Arc;

use openraft::storage::Adaptor;
use openraft::{BasicNode, Config, Raft};
use tokio::task::JoinHandle;
use tonic::transport::Server;
use tracing::info;

use crate::broker::config::ClusterConfig;
use crate::storage::RocksDbEngine;
use fila_proto::fila_cluster_server::FilaClusterServer;
use grpc_service::ClusterGrpcService;
use network::FilaNetworkFactory;
use store::FilaRaftStore;

/// Manages the Raft lifecycle: creates the Raft instance, starts the cluster
/// gRPC service, bootstraps or joins the cluster.
pub struct ClusterManager {
    raft: Arc<Raft<TypeConfig>>,
    grpc_handle: JoinHandle<()>,
    shutdown_tx: tokio::sync::oneshot::Sender<()>,
}

impl ClusterManager {
    /// Create and start the cluster manager.
    ///
    /// This initializes the Raft instance, starts the cluster gRPC service on
    /// `config.bind_addr`, and either bootstraps a new cluster or joins an
    /// existing one.
    pub async fn start(
        config: &ClusterConfig,
        db: Arc<RocksDbEngine>,
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
        let raft_config = Arc::new(
            raft_config
                .validate()
                .map_err(|e| format!("invalid raft config: {e}"))?,
        );

        let store = FilaRaftStore::new(db);
        let (log_store, state_machine) = Adaptor::new(store);

        let network = FilaNetworkFactory;

        let raft = Raft::new(node_id, raft_config, network, log_store, state_machine).await?;
        let raft = Arc::new(raft);

        // Start cluster gRPC service.
        let service = ClusterGrpcService::new(Arc::clone(&raft));
        let bind_addr: std::net::SocketAddr = config.bind_addr.parse()?;
        let (shutdown_tx, shutdown_rx) = tokio::sync::oneshot::channel::<()>();

        info!(%bind_addr, node_id, "starting cluster gRPC service");

        let grpc_handle = tokio::spawn(async move {
            if let Err(e) = Server::builder()
                .add_service(FilaClusterServer::new(service))
                .serve_with_shutdown(bind_addr, async {
                    let _ = shutdown_rx.await;
                })
                .await
            {
                tracing::error!("cluster gRPC service error: {e}");
            }
        });

        // Bootstrap or join cluster.
        if config.bootstrap {
            info!(node_id, "bootstrapping single-node cluster");
            let mut members = BTreeMap::new();
            members.insert(
                node_id,
                BasicNode {
                    addr: config.bind_addr.clone(),
                },
            );
            raft.initialize(members).await?;
        } else if !config.peers.is_empty() {
            info!(node_id, peers = ?config.peers, "joining existing cluster");
            Self::join_cluster(node_id, &config.bind_addr, &config.peers).await?;
        }

        Ok(Self {
            raft,
            grpc_handle,
            shutdown_tx,
        })
    }

    /// Join an existing cluster by contacting seed peers.
    async fn join_cluster(
        node_id: u64,
        bind_addr: &str,
        peers: &[String],
    ) -> Result<(), Box<dyn std::error::Error>> {
        use fila_proto::fila_cluster_client::FilaClusterClient;
        use fila_proto::AddNodeRequest;

        let req = AddNodeRequest {
            node_id,
            addr: bind_addr.to_string(),
        };

        for peer in peers {
            let url = if peer.starts_with("http") {
                peer.clone()
            } else {
                format!("http://{peer}")
            };

            match FilaClusterClient::connect(url).await {
                Ok(mut client) => {
                    let resp = client
                        .add_node(tonic::Request::new(req.clone()))
                        .await?
                        .into_inner();

                    if resp.success {
                        info!(node_id, peer, "successfully joined cluster");
                        return Ok(());
                    }

                    // If not leader, try the leader address.
                    if !resp.leader_addr.is_empty() {
                        let leader_url = if resp.leader_addr.starts_with("http") {
                            resp.leader_addr.clone()
                        } else {
                            format!("http://{}", resp.leader_addr)
                        };

                        match FilaClusterClient::connect(leader_url).await {
                            Ok(mut leader_client) => {
                                let resp = leader_client
                                    .add_node(tonic::Request::new(req.clone()))
                                    .await?
                                    .into_inner();

                                if resp.success {
                                    info!(node_id, "joined cluster via leader redirect");
                                    return Ok(());
                                }

                                tracing::warn!(error = resp.error, "failed to join via leader");
                            }
                            Err(e) => {
                                tracing::warn!(
                                    leader_addr = resp.leader_addr,
                                    error = %e,
                                    "failed to connect to leader"
                                );
                            }
                        }
                    } else {
                        tracing::warn!(peer, error = resp.error, "peer rejected add_node");
                    }
                }
                Err(e) => {
                    tracing::warn!(peer, error = %e, "failed to connect to peer");
                }
            }
        }

        Err("failed to join cluster via any seed peer".into())
    }

    /// Get a reference to the Raft instance.
    pub fn raft(&self) -> &Arc<Raft<TypeConfig>> {
        &self.raft
    }

    /// Gracefully shut down the Raft instance and cluster gRPC service.
    pub async fn shutdown(self) {
        info!("shutting down cluster manager");
        if let Err(e) = self.raft.shutdown().await {
            tracing::error!("raft shutdown error: {e:?}");
        }
        let _ = self.shutdown_tx.send(());
        let _ = self.grpc_handle.await;
    }
}
