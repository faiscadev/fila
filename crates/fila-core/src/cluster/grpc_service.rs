use std::sync::Arc;

use openraft::error::RaftError;
use openraft::{BasicNode, Raft};
use tonic::{Request, Response, Status};

use super::multi_raft::MultiRaftManager;
use super::proto_convert;
use super::types::{NodeId, TypeConfig};
use crate::Broker;
use fila_proto::fila_cluster_server::FilaCluster;
use fila_proto::{
    AddNodeRequest, AddNodeResponse, GetNodeInfoRequest, GetNodeInfoResponse,
    RaftAppendEntriesRequest, RaftAppendEntriesResponse, RaftClientWriteRequest,
    RaftClientWriteResponse, RaftInstallSnapshotRequest, RaftInstallSnapshotResponse,
    RaftVoteRequest, RaftVoteResponse, RemoveNodeRequest, RemoveNodeResponse,
};

/// gRPC service handler that forwards Raft RPCs to the correct local Raft
/// instance — either the meta group or a queue-level group based on `group_id`.
pub struct ClusterGrpcService {
    meta_raft: Arc<Raft<TypeConfig>>,
    multi_raft: Arc<MultiRaftManager>,
    /// Broker reference for applying forwarded writes to the local scheduler.
    /// Set after Broker creation via OnceLock — None during initial startup.
    broker: Arc<std::sync::OnceLock<Arc<Broker>>>,
    /// This node's ID and client-facing gRPC address (for GetNodeInfo RPC).
    node_id: NodeId,
    client_addr: String,
}

impl ClusterGrpcService {
    pub fn new(
        meta_raft: Arc<Raft<TypeConfig>>,
        multi_raft: Arc<MultiRaftManager>,
        broker: Arc<std::sync::OnceLock<Arc<Broker>>>,
        node_id: NodeId,
        client_addr: String,
    ) -> Self {
        Self {
            meta_raft,
            multi_raft,
            broker,
            node_id,
            client_addr,
        }
    }

    /// Apply a forwarded write to the local scheduler after Raft commit.
    /// This ensures the leader's scheduler has the data for serving consumers.
    async fn apply_to_scheduler(broker: &Broker, req: &super::types::ClusterRequest) {
        match req {
            super::types::ClusterRequest::Enqueue { messages } => {
                if messages.is_empty() {
                    return;
                }
                let msgs = messages.clone();
                let (reply_tx, reply_rx) = tokio::sync::oneshot::channel();
                if let Err(e) = broker.send_command(crate::SchedulerCommand::Enqueue {
                    messages: msgs,
                    reply: reply_tx,
                }) {
                    tracing::error!(error = %e, "failed to apply forwarded enqueue to scheduler");
                    return;
                }
                match reply_rx.await {
                    Err(e) => {
                        tracing::error!(error = %e, "scheduler dropped reply for forwarded enqueue");
                    }
                    Ok(results) => {
                        for result in results {
                            if let Err(e) = result {
                                tracing::error!(error = %e, "scheduler rejected forwarded enqueue");
                            }
                        }
                    }
                }
            }
            super::types::ClusterRequest::Ack { items } => {
                if items.is_empty() {
                    return;
                }
                let ack_items: Vec<crate::broker::command::AckItem> = items
                    .iter()
                    .map(|i| crate::broker::command::AckItem {
                        queue_id: i.queue_id.clone(),
                        msg_id: i.msg_id,
                    })
                    .collect();
                let (reply_tx, reply_rx) = tokio::sync::oneshot::channel();
                if let Err(e) = broker.send_command(crate::SchedulerCommand::Ack {
                    items: ack_items,
                    reply: reply_tx,
                }) {
                    tracing::error!(error = %e, "failed to apply forwarded ack to scheduler");
                    return;
                }
                match reply_rx.await {
                    Err(e) => {
                        tracing::error!(error = %e, "scheduler dropped reply for forwarded ack");
                    }
                    Ok(results) => {
                        for result in results {
                            if let Err(e) = result {
                                tracing::error!(error = %e, "scheduler rejected forwarded ack");
                            }
                        }
                    }
                }
            }
            super::types::ClusterRequest::Nack { items } => {
                if items.is_empty() {
                    return;
                }
                let nack_items: Vec<crate::broker::command::NackItem> = items
                    .iter()
                    .map(|i| crate::broker::command::NackItem {
                        queue_id: i.queue_id.clone(),
                        msg_id: i.msg_id,
                        error: i.error.clone(),
                    })
                    .collect();
                let (reply_tx, reply_rx) = tokio::sync::oneshot::channel();
                if let Err(e) = broker.send_command(crate::SchedulerCommand::Nack {
                    items: nack_items,
                    reply: reply_tx,
                }) {
                    tracing::error!(error = %e, "failed to apply forwarded nack to scheduler");
                    return;
                }
                match reply_rx.await {
                    Err(e) => {
                        tracing::error!(error = %e, "scheduler dropped reply for forwarded nack");
                    }
                    Ok(results) => {
                        for result in results {
                            if let Err(e) = result {
                                tracing::error!(error = %e, "scheduler rejected forwarded nack");
                            }
                        }
                    }
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

#[tonic::async_trait]
impl FilaCluster for ClusterGrpcService {
    async fn append_entries(
        &self,
        request: Request<RaftAppendEntriesRequest>,
    ) -> Result<Response<RaftAppendEntriesResponse>, Status> {
        let inner = request.into_inner();
        let raft = self.resolve_raft(&inner.group_id).await?;

        let req = proto_convert::append_entries_request_from_proto(inner)
            .map_err(|e| Status::invalid_argument(format!("deserialize: {e}")))?;

        match raft.append_entries(req).await {
            Ok(resp) => Ok(Response::new(
                proto_convert::append_entries_response_to_proto(resp),
            )),
            Err(e) => Err(Status::internal(format!("raft error: {e}"))),
        }
    }

    async fn vote(
        &self,
        request: Request<RaftVoteRequest>,
    ) -> Result<Response<RaftVoteResponse>, Status> {
        let inner = request.into_inner();
        let raft = self.resolve_raft(&inner.group_id).await?;

        let req = proto_convert::vote_request_from_proto(inner)
            .map_err(|e| Status::invalid_argument(format!("deserialize: {e}")))?;

        match raft.vote(req).await {
            Ok(resp) => Ok(Response::new(proto_convert::vote_response_to_proto(resp))),
            Err(e) => Err(Status::internal(format!("raft error: {e}"))),
        }
    }

    async fn install_snapshot(
        &self,
        request: Request<RaftInstallSnapshotRequest>,
    ) -> Result<Response<RaftInstallSnapshotResponse>, Status> {
        let inner = request.into_inner();
        let raft = self.resolve_raft(&inner.group_id).await?;

        let req = proto_convert::install_snapshot_request_from_proto(inner)
            .map_err(|e| Status::invalid_argument(format!("deserialize: {e}")))?;

        match raft.install_snapshot(req).await {
            Ok(resp) => Ok(Response::new(
                proto_convert::install_snapshot_response_to_proto(resp),
            )),
            Err(e) => Err(Status::internal(format!("raft error: {e}"))),
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

        // Store the joining node's client address for leader hint routing.
        if !req.client_addr.is_empty() {
            self.multi_raft
                .register_client_addr(node_id, &req.client_addr)
                .await;
        }

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

    async fn get_node_info(
        &self,
        _request: Request<GetNodeInfoRequest>,
    ) -> Result<Response<GetNodeInfoResponse>, Status> {
        Ok(Response::new(GetNodeInfoResponse {
            node_id: self.node_id,
            client_addr: self.client_addr.clone(),
        }))
    }

    async fn client_write(
        &self,
        request: Request<RaftClientWriteRequest>,
    ) -> Result<Response<RaftClientWriteResponse>, Status> {
        let inner = request.into_inner();
        let group_id = inner.group_id.clone();
        let raft = self.resolve_raft(&group_id).await?;

        let req: super::types::ClusterRequest = inner
            .request
            .ok_or_else(|| Status::invalid_argument("missing request"))?
            .try_into()
            .map_err(|e: super::proto_convert::ConvertError| {
                Status::invalid_argument(format!("deserialize: {e}"))
            })?;

        match raft.client_write(req.clone()).await {
            Ok(resp) => {
                // Apply to local scheduler so forwarded writes have
                // real side effects on the leader node.
                if let Some(broker) = self.broker.get() {
                    Self::apply_to_scheduler(broker, &req).await;
                }

                let response_proto = fila_proto::ClusterResponseProto::from(resp.data);
                Ok(Response::new(RaftClientWriteResponse {
                    response: Some(response_proto),
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
                // Signal forwarding via error response with leader address.
                let response_proto =
                    fila_proto::ClusterResponseProto::from(super::types::ClusterResponse::Error {
                        message: format!("ForwardToLeader:{leader_addr}"),
                    });
                Ok(Response::new(RaftClientWriteResponse {
                    response: Some(response_proto),
                }))
            }
            Err(e) => Err(Status::internal(format!("raft error: {e}"))),
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
