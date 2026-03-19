use openraft::error::{InstallSnapshotError, RPCError, RaftError, Unreachable};
use openraft::network::RPCOption;
use openraft::raft::{
    AppendEntriesRequest, AppendEntriesResponse, InstallSnapshotRequest, InstallSnapshotResponse,
    VoteRequest, VoteResponse,
};
use openraft::{BasicNode, RaftNetwork, RaftNetworkFactory};
use tokio::sync::OnceCell;

use super::proto_convert;
use super::types::{NodeId, TypeConfig};
use fila_proto::fila_cluster_client::FilaClusterClient;

/// Factory that creates gRPC-based network connections to peer nodes.
///
/// Each factory is scoped to a Raft group: the meta group uses an empty
/// `group_id`, while queue groups carry their queue ID so the remote node
/// can route the RPC to the correct Raft instance.
pub struct FilaNetworkFactory {
    group_id: String,
}

impl FilaNetworkFactory {
    /// Create a factory for the meta Raft group.
    pub fn meta() -> Self {
        Self {
            group_id: String::new(),
        }
    }

    /// Create a factory for a queue-level Raft group.
    pub fn for_queue(queue_id: String) -> Self {
        Self { group_id: queue_id }
    }
}

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
            group_id: self.group_id.clone(),
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
    group_id: String,
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
        let proto_req = proto_convert::append_entries_request_to_proto(rpc, self.group_id.clone());

        let mut client = self.get_client().await?;
        let resp = client
            .append_entries(proto_req)
            .await
            .map_err(|e| RPCError::Unreachable(Unreachable::new(&e)))?;

        proto_convert::append_entries_response_from_proto(resp.into_inner())
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
        let proto_req =
            proto_convert::install_snapshot_request_to_proto(rpc, self.group_id.clone());

        let mut client = self.get_client().await.map_err(
            |e: RPCError<NodeId, BasicNode, RaftError<NodeId>>| {
                RPCError::Unreachable(Unreachable::new(&std::io::Error::other(format!("{e}"))))
            },
        )?;
        let resp = client
            .install_snapshot(proto_req)
            .await
            .map_err(|e| RPCError::Unreachable(Unreachable::new(&e)))?;

        proto_convert::install_snapshot_response_from_proto(resp.into_inner())
            .map_err(|e| RPCError::Unreachable(Unreachable::new(&e)))
    }

    async fn vote(
        &mut self,
        rpc: VoteRequest<NodeId>,
        _option: RPCOption,
    ) -> Result<VoteResponse<NodeId>, RPCError<NodeId, BasicNode, RaftError<NodeId>>> {
        let proto_req = proto_convert::vote_request_to_proto(rpc, self.group_id.clone());

        let mut client = self.get_client().await?;
        let resp = client
            .vote(proto_req)
            .await
            .map_err(|e| RPCError::Unreachable(Unreachable::new(&e)))?;

        proto_convert::vote_response_from_proto(resp.into_inner())
            .map_err(|e| RPCError::Unreachable(Unreachable::new(&e)))
    }
}
