use std::sync::Arc;

use openraft::error::RaftError;
use openraft::raft::{AppendEntriesRequest, InstallSnapshotRequest, VoteRequest};
use openraft::{BasicNode, Raft};
use tonic::{Request, Response, Status};

use super::types::{NodeId, TypeConfig};
use fila_proto::fila_cluster_server::FilaCluster;
use fila_proto::{
    AddNodeRequest, AddNodeResponse, RaftRequest, RaftResponse, RemoveNodeRequest,
    RemoveNodeResponse,
};

/// gRPC service handler that forwards Raft RPCs to the local openraft instance.
pub struct ClusterGrpcService {
    raft: Arc<Raft<TypeConfig>>,
}

impl ClusterGrpcService {
    pub fn new(raft: Arc<Raft<TypeConfig>>) -> Self {
        Self { raft }
    }
}

/// Serialize a Raft response (or error) into a RaftResponse proto message.
fn raft_response_ok<T: serde::Serialize>(value: &T) -> Result<Response<RaftResponse>, Status> {
    let data = serde_json::to_vec(value)
        .map_err(|e| Status::internal(format!("serialize response: {e}")))?;
    Ok(Response::new(RaftResponse {
        data,
        error: String::new(),
    }))
}

fn raft_response_err(error: impl std::fmt::Display) -> Result<Response<RaftResponse>, Status> {
    Ok(Response::new(RaftResponse {
        data: Vec::new(),
        error: error.to_string(),
    }))
}

#[tonic::async_trait]
impl FilaCluster for ClusterGrpcService {
    async fn append_entries(
        &self,
        request: Request<RaftRequest>,
    ) -> Result<Response<RaftResponse>, Status> {
        let req: AppendEntriesRequest<TypeConfig> =
            serde_json::from_slice(&request.into_inner().data)
                .map_err(|e| Status::invalid_argument(format!("deserialize: {e}")))?;

        match self.raft.append_entries(req).await {
            Ok(resp) => raft_response_ok(&resp),
            Err(e) => raft_response_err(e),
        }
    }

    async fn vote(&self, request: Request<RaftRequest>) -> Result<Response<RaftResponse>, Status> {
        let req: VoteRequest<NodeId> = serde_json::from_slice(&request.into_inner().data)
            .map_err(|e| Status::invalid_argument(format!("deserialize: {e}")))?;

        match self.raft.vote(req).await {
            Ok(resp) => raft_response_ok(&resp),
            Err(e) => raft_response_err(e),
        }
    }

    async fn install_snapshot(
        &self,
        request: Request<RaftRequest>,
    ) -> Result<Response<RaftResponse>, Status> {
        let req: InstallSnapshotRequest<TypeConfig> =
            serde_json::from_slice(&request.into_inner().data)
                .map_err(|e| Status::invalid_argument(format!("deserialize: {e}")))?;

        match self.raft.install_snapshot(req).await {
            Ok(resp) => raft_response_ok(&resp),
            Err(e) => raft_response_err(e),
        }
    }

    async fn add_node(
        &self,
        request: Request<AddNodeRequest>,
    ) -> Result<Response<AddNodeResponse>, Status> {
        let req = request.into_inner();
        let node_id = req.node_id;
        let node = BasicNode {
            addr: req.addr.clone(),
        };

        // First add as learner, then promote to voter.
        if let Err(e) = self.raft.add_learner(node_id, node, true).await {
            return Ok(Response::new(handle_membership_error(e)));
        }

        // Promote learner to voter.
        let mut members = std::collections::BTreeSet::new();
        members.insert(node_id);
        match self
            .raft
            .change_membership(openraft::ChangeMembers::AddVoterIds(members), false)
            .await
        {
            Ok(_) => Ok(Response::new(AddNodeResponse {
                success: true,
                error: String::new(),
                leader_addr: String::new(),
            })),
            Err(e) => Ok(Response::new(handle_membership_error(e))),
        }
    }

    async fn remove_node(
        &self,
        request: Request<RemoveNodeRequest>,
    ) -> Result<Response<RemoveNodeResponse>, Status> {
        let req = request.into_inner();
        let node_id = req.node_id;

        let mut members = std::collections::BTreeSet::new();
        members.insert(node_id);
        match self
            .raft
            .change_membership(openraft::ChangeMembers::RemoveVoters(members), false)
            .await
        {
            Ok(_) => Ok(Response::new(RemoveNodeResponse {
                success: true,
                error: String::new(),
                leader_addr: String::new(),
            })),
            Err(e) => Ok(Response::new(handle_remove_error(e))),
        }
    }
}

/// Convert a membership change error into an AddNodeResponse with leader hint.
fn handle_membership_error(
    error: RaftError<NodeId, openraft::error::ClientWriteError<NodeId, BasicNode>>,
) -> AddNodeResponse {
    match &error {
        RaftError::APIError(write_err) => match write_err {
            openraft::error::ClientWriteError::ForwardToLeader(fwd) => {
                let leader_addr = fwd
                    .leader_node
                    .as_ref()
                    .map(|n| n.addr.clone())
                    .unwrap_or_default();
                AddNodeResponse {
                    success: false,
                    error: "not leader".to_string(),
                    leader_addr,
                }
            }
            openraft::error::ClientWriteError::ChangeMembershipError(e) => AddNodeResponse {
                success: false,
                error: format!("{e}"),
                leader_addr: String::new(),
            },
        },
        RaftError::Fatal(e) => AddNodeResponse {
            success: false,
            error: format!("fatal: {e}"),
            leader_addr: String::new(),
        },
    }
}

/// Convert a membership change error into a RemoveNodeResponse with leader hint.
fn handle_remove_error(
    error: RaftError<NodeId, openraft::error::ClientWriteError<NodeId, BasicNode>>,
) -> RemoveNodeResponse {
    match &error {
        RaftError::APIError(write_err) => match write_err {
            openraft::error::ClientWriteError::ForwardToLeader(fwd) => {
                let leader_addr = fwd
                    .leader_node
                    .as_ref()
                    .map(|n| n.addr.clone())
                    .unwrap_or_default();
                RemoveNodeResponse {
                    success: false,
                    error: "not leader".to_string(),
                    leader_addr,
                }
            }
            openraft::error::ClientWriteError::ChangeMembershipError(e) => RemoveNodeResponse {
                success: false,
                error: format!("{e}"),
                leader_addr: String::new(),
            },
        },
        RaftError::Fatal(e) => RemoveNodeResponse {
            success: false,
            error: format!("fatal: {e}"),
            leader_addr: String::new(),
        },
    }
}
