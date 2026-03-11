use std::sync::Arc;

use openraft::error::RaftError;
use openraft::raft::{AppendEntriesRequest, InstallSnapshotRequest, VoteRequest};
use openraft::{BasicNode, Raft};
use tonic::{Request, Response, Status};

use super::multi_raft::MultiRaftManager;
use super::types::{NodeId, TypeConfig};
use crate::Broker;
use fila_proto::fila_cluster_server::FilaCluster;
use fila_proto::{
    AddNodeRequest, AddNodeResponse, RaftRequest, RaftResponse, RemoveNodeRequest,
    RemoveNodeResponse,
};

/// gRPC service handler that forwards Raft RPCs to the correct local Raft
/// instance — either the meta group or a queue-level group based on `group_id`.
pub struct ClusterGrpcService {
    meta_raft: Arc<Raft<TypeConfig>>,
    multi_raft: Arc<MultiRaftManager>,
    /// Broker reference for applying forwarded writes to the local scheduler.
    /// Set after Broker creation via OnceLock — None during initial startup.
    broker: Arc<std::sync::OnceLock<Arc<Broker>>>,
}

impl ClusterGrpcService {
    pub fn new(
        meta_raft: Arc<Raft<TypeConfig>>,
        multi_raft: Arc<MultiRaftManager>,
        broker: Arc<std::sync::OnceLock<Arc<Broker>>>,
    ) -> Self {
        Self {
            meta_raft,
            multi_raft,
            broker,
        }
    }

    /// Apply a forwarded write to the local scheduler after Raft commit.
    /// This ensures the leader's scheduler has the data for serving consumers.
    async fn apply_to_scheduler(broker: &Broker, req: &super::types::ClusterRequest) {
        match req {
            super::types::ClusterRequest::Enqueue { message } => {
                let (reply_tx, reply_rx) = tokio::sync::oneshot::channel();
                if let Err(e) = broker.send_command(crate::SchedulerCommand::Enqueue {
                    message: message.clone(),
                    reply: reply_tx,
                }) {
                    tracing::error!(error = %e, "failed to apply forwarded enqueue to scheduler");
                    return;
                }
                match reply_rx.await {
                    Err(e) => {
                        tracing::error!(error = %e, "scheduler dropped reply for forwarded enqueue");
                    }
                    Ok(Err(e)) => {
                        tracing::error!(error = %e, "scheduler rejected forwarded enqueue");
                    }
                    Ok(Ok(_)) => {}
                }
            }
            super::types::ClusterRequest::Ack { queue_id, msg_id } => {
                let (reply_tx, reply_rx) = tokio::sync::oneshot::channel();
                if let Err(e) = broker.send_command(crate::SchedulerCommand::Ack {
                    queue_id: queue_id.clone(),
                    msg_id: *msg_id,
                    reply: reply_tx,
                }) {
                    tracing::error!(error = %e, "failed to apply forwarded ack to scheduler");
                    return;
                }
                match reply_rx.await {
                    Err(e) => {
                        tracing::error!(error = %e, "scheduler dropped reply for forwarded ack");
                    }
                    Ok(Err(e)) => {
                        tracing::error!(error = %e, "scheduler rejected forwarded ack");
                    }
                    Ok(Ok(_)) => {}
                }
            }
            super::types::ClusterRequest::Nack {
                queue_id,
                msg_id,
                error,
            } => {
                let (reply_tx, reply_rx) = tokio::sync::oneshot::channel();
                if let Err(e) = broker.send_command(crate::SchedulerCommand::Nack {
                    queue_id: queue_id.clone(),
                    msg_id: *msg_id,
                    error: error.clone(),
                    reply: reply_tx,
                }) {
                    tracing::error!(error = %e, "failed to apply forwarded nack to scheduler");
                    return;
                }
                match reply_rx.await {
                    Err(e) => {
                        tracing::error!(error = %e, "scheduler dropped reply for forwarded nack");
                    }
                    Ok(Err(e)) => {
                        tracing::error!(error = %e, "scheduler rejected forwarded nack");
                    }
                    Ok(Ok(_)) => {}
                }
            }
            // Other request types (CreateQueue, DeleteQueue, etc.) are handled
            // through the meta Raft event system, not here.
            _ => {}
        }
    }

    /// Resolve which Raft instance should handle this RPC.
    async fn resolve_raft(&self, group_id: &str) -> Result<Arc<Raft<TypeConfig>>, Status> {
        if group_id.is_empty() {
            Ok(Arc::clone(&self.meta_raft))
        } else {
            self.multi_raft
                .get_raft(group_id)
                .await
                .ok_or_else(|| Status::not_found(format!("unknown raft group: {group_id}")))
        }
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
        let inner = request.into_inner();
        let raft = self.resolve_raft(&inner.group_id).await?;

        let req: AppendEntriesRequest<TypeConfig> = serde_json::from_slice(&inner.data)
            .map_err(|e| Status::invalid_argument(format!("deserialize: {e}")))?;

        match raft.append_entries(req).await {
            Ok(resp) => raft_response_ok(&resp),
            Err(e) => raft_response_err(e),
        }
    }

    async fn vote(&self, request: Request<RaftRequest>) -> Result<Response<RaftResponse>, Status> {
        let inner = request.into_inner();
        let raft = self.resolve_raft(&inner.group_id).await?;

        let req: VoteRequest<NodeId> = serde_json::from_slice(&inner.data)
            .map_err(|e| Status::invalid_argument(format!("deserialize: {e}")))?;

        match raft.vote(req).await {
            Ok(resp) => raft_response_ok(&resp),
            Err(e) => raft_response_err(e),
        }
    }

    async fn install_snapshot(
        &self,
        request: Request<RaftRequest>,
    ) -> Result<Response<RaftResponse>, Status> {
        let inner = request.into_inner();
        let raft = self.resolve_raft(&inner.group_id).await?;

        let req: InstallSnapshotRequest<TypeConfig> = serde_json::from_slice(&inner.data)
            .map_err(|e| Status::invalid_argument(format!("deserialize: {e}")))?;

        match raft.install_snapshot(req).await {
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

        // Add/remove node only affects the meta Raft group.
        if let Err(e) = self.meta_raft.add_learner(node_id, node, true).await {
            return Ok(Response::new(handle_membership_error(e)));
        }

        let mut members = std::collections::BTreeSet::new();
        members.insert(node_id);
        match self
            .meta_raft
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

    async fn client_write(
        &self,
        request: Request<RaftRequest>,
    ) -> Result<Response<RaftResponse>, Status> {
        let inner = request.into_inner();
        let raft = self.resolve_raft(&inner.group_id).await?;

        let req: super::types::ClusterRequest = serde_json::from_slice(&inner.data)
            .map_err(|e| Status::invalid_argument(format!("deserialize: {e}")))?;

        match raft.client_write(req.clone()).await {
            Ok(resp) => {
                // Apply to local scheduler so forwarded writes have
                // real side effects on the leader node.
                if let Some(broker) = self.broker.get() {
                    Self::apply_to_scheduler(broker, &req).await;
                }

                let data = serde_json::to_vec(&resp.data)
                    .map_err(|e| Status::internal(format!("serialize response: {e}")))?;
                Ok(Response::new(RaftResponse {
                    data,
                    error: String::new(),
                }))
            }
            Err(openraft::error::RaftError::APIError(
                openraft::error::ClientWriteError::ForwardToLeader(fwd),
            )) => {
                let leader_addr = fwd
                    .leader_node
                    .as_ref()
                    .map(|n| n.addr.clone())
                    .unwrap_or_default();
                Ok(Response::new(RaftResponse {
                    data: Vec::new(),
                    error: format!("ForwardToLeader:{leader_addr}"),
                }))
            }
            Err(e) => raft_response_err(e),
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
            .meta_raft
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
