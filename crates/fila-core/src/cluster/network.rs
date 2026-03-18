use openraft::error::{InstallSnapshotError, RPCError, RaftError, Unreachable};
use openraft::network::RPCOption;
use openraft::raft::{
    AppendEntriesRequest, AppendEntriesResponse, InstallSnapshotRequest, InstallSnapshotResponse,
    VoteRequest, VoteResponse,
};
use openraft::{BasicNode, RaftNetwork, RaftNetworkFactory};
use tokio::sync::OnceCell;

use super::types::{NodeId, TypeConfig};
use fila_proto::fila_cluster_client::FilaClusterClient;
use fila_proto::RaftRequest;

/// Factory that creates gRPC-based network connections to peer nodes.
pub struct FilaNetworkFactory;

impl RaftNetworkFactory<TypeConfig> for FilaNetworkFactory {
    type Network = FilaNetwork;

    async fn new_client(&mut self, _target: NodeId, node: &BasicNode) -> Self::Network {
        let url = if node.addr.starts_with("http") {
            node.addr.clone()
        } else {
            format!("http://{}", node.addr)
        };
        FilaNetwork {
            url,
            client: OnceCell::new(),
        }
    }
}

/// A gRPC-based network connection to a single peer node.
///
/// The underlying tonic channel is lazily established on first use
/// and reused for all subsequent RPCs to this peer.
pub struct FilaNetwork {
    url: String,
    client: OnceCell<FilaClusterClient<tonic::transport::Channel>>,
}

impl FilaNetwork {
    async fn get_client(
        &self,
    ) -> Result<
        FilaClusterClient<tonic::transport::Channel>,
        RPCError<NodeId, BasicNode, RaftError<NodeId>>,
    > {
        let client = self
            .client
            .get_or_try_init(|| async {
                FilaClusterClient::connect(self.url.clone())
                    .await
                    .map_err(|e| RPCError::Unreachable(Unreachable::new(&e)))
            })
            .await?;
        // Clone is cheap — tonic Channel is backed by a shared connection pool.
        Ok(client.clone())
    }
}

impl RaftNetwork<TypeConfig> for FilaNetwork {
    async fn append_entries(
        &mut self,
        rpc: AppendEntriesRequest<TypeConfig>,
        _option: RPCOption,
    ) -> Result<AppendEntriesResponse<NodeId>, RPCError<NodeId, BasicNode, RaftError<NodeId>>> {
        let data =
            serde_json::to_vec(&rpc).map_err(|e| RPCError::Unreachable(Unreachable::new(&e)))?;

        let mut client = self.get_client().await?;
        let resp = client
            .append_entries(RaftRequest { data })
            .await
            .map_err(|e| RPCError::Unreachable(Unreachable::new(&e)))?;

        let resp_data = resp.into_inner();
        if !resp_data.error.is_empty() {
            return Err(RPCError::Unreachable(Unreachable::new(
                &std::io::Error::other(resp_data.error),
            )));
        }

        serde_json::from_slice(&resp_data.data)
            .map_err(|e| RPCError::Unreachable(Unreachable::new(&e)))
    }

    async fn install_snapshot(
        &mut self,
        rpc: InstallSnapshotRequest<TypeConfig>,
        _option: RPCOption,
    ) -> Result<
        InstallSnapshotResponse<NodeId>,
        RPCError<NodeId, BasicNode, RaftError<NodeId, InstallSnapshotError>>,
    > {
        let data =
            serde_json::to_vec(&rpc).map_err(|e| RPCError::Unreachable(Unreachable::new(&e)))?;

        let mut client = self.get_client().await.map_err(
            |e: RPCError<NodeId, BasicNode, RaftError<NodeId>>| {
                RPCError::Unreachable(Unreachable::new(&std::io::Error::other(format!("{e}"))))
            },
        )?;
        let resp = client
            .install_snapshot(RaftRequest { data })
            .await
            .map_err(|e| RPCError::Unreachable(Unreachable::new(&e)))?;

        let resp_data = resp.into_inner();
        if !resp_data.error.is_empty() {
            return Err(RPCError::Unreachable(Unreachable::new(
                &std::io::Error::other(resp_data.error),
            )));
        }

        serde_json::from_slice(&resp_data.data)
            .map_err(|e| RPCError::Unreachable(Unreachable::new(&e)))
    }

    async fn vote(
        &mut self,
        rpc: VoteRequest<NodeId>,
        _option: RPCOption,
    ) -> Result<VoteResponse<NodeId>, RPCError<NodeId, BasicNode, RaftError<NodeId>>> {
        let data =
            serde_json::to_vec(&rpc).map_err(|e| RPCError::Unreachable(Unreachable::new(&e)))?;

        let mut client = self.get_client().await?;
        let resp = client
            .vote(RaftRequest { data })
            .await
            .map_err(|e| RPCError::Unreachable(Unreachable::new(&e)))?;

        let resp_data = resp.into_inner();
        if !resp_data.error.is_empty() {
            return Err(RPCError::Unreachable(Unreachable::new(
                &std::io::Error::other(resp_data.error),
            )));
        }

        serde_json::from_slice(&resp_data.data)
            .map_err(|e| RPCError::Unreachable(Unreachable::new(&e)))
    }
}
