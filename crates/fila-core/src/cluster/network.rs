use std::sync::Arc;

use openraft::error::{InstallSnapshotError, RPCError, RaftError, Unreachable};
use openraft::network::RPCOption;
use openraft::raft::{
    AppendEntriesRequest, AppendEntriesResponse, InstallSnapshotRequest, InstallSnapshotResponse,
    VoteRequest, VoteResponse,
};
use openraft::{BasicNode, RaftNetwork, RaftNetworkFactory};
use tokio::sync::OnceCell;
use tonic::transport::{Channel, ClientTlsConfig};

use super::proto_convert;
use super::types::{NodeId, TypeConfig};
use fila_proto::fila_cluster_client::FilaClusterClient;

/// Build a tonic `Channel` to a peer, applying TLS when configured.
///
/// When `tls` is `None`, uses plain HTTP/2. When set, upgrades to TLS
/// and uses the `https://` scheme.
async fn connect_channel(
    addr: &str,
    tls: Option<&Arc<ClientTlsConfig>>,
) -> Result<Channel, Box<dyn std::error::Error + Send + Sync>> {
    if let Some(tls) = tls {
        // Replace http:// with https:// when TLS is enabled.
        let tls_url = if addr.starts_with("http://") {
            addr.replacen("http://", "https://", 1)
        } else if addr.starts_with("https://") {
            addr.to_string()
        } else {
            format!("https://{addr}")
        };
        let channel = Channel::from_shared(tls_url)?
            .tls_config((**tls).clone())?
            .connect()
            .await?;
        Ok(channel)
    } else {
        let plain_url = if addr.starts_with("http") {
            addr.to_string()
        } else {
            format!("http://{addr}")
        };
        let channel = Channel::from_shared(plain_url)?.connect().await?;
        Ok(channel)
    }
}

/// Factory that creates gRPC-based network connections to peer nodes.
///
/// Each factory is scoped to a Raft group: the meta group uses an empty
/// `group_id`, while queue groups carry their queue ID so the remote node
/// can route the RPC to the correct Raft instance.
pub struct FilaNetworkFactory {
    group_id: String,
    tls: Option<Arc<ClientTlsConfig>>,
}

impl FilaNetworkFactory {
    /// Create a factory for the meta Raft group (no TLS).
    pub fn meta() -> Self {
        Self {
            group_id: String::new(),
            tls: None,
        }
    }

    /// Create a factory for the meta Raft group with optional TLS.
    pub fn meta_with_tls(tls: Option<Arc<ClientTlsConfig>>) -> Self {
        Self {
            group_id: String::new(),
            tls,
        }
    }

    /// Create a factory for a queue-level Raft group.
    pub fn for_queue(queue_id: String) -> Self {
        Self {
            group_id: queue_id,
            tls: None,
        }
    }

    /// Create a factory for a queue-level Raft group with optional TLS.
    pub fn for_queue_with_tls(queue_id: String, tls: Option<Arc<ClientTlsConfig>>) -> Self {
        Self {
            group_id: queue_id,
            tls,
        }
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
            tls: self.tls.clone(),
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
    tls: Option<Arc<ClientTlsConfig>>,
    client: OnceCell<FilaClusterClient<tonic::transport::Channel>>,
}

impl FilaNetwork {
    async fn get_client(
        &self,
    ) -> Result<
        FilaClusterClient<tonic::transport::Channel>,
        RPCError<NodeId, BasicNode, RaftError<NodeId>>,
    > {
        let tls = self.tls.clone();
        let url = self.url.clone();
        let client = self
            .client
            .get_or_try_init(|| async {
                let channel = connect_channel(&url, tls.as_ref()).await.map_err(|e| {
                    let io = std::io::Error::other(e.to_string());
                    RPCError::Unreachable(Unreachable::new(&io))
                })?;
                Ok::<_, RPCError<NodeId, BasicNode, RaftError<NodeId>>>(FilaClusterClient::new(
                    channel,
                ))
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
