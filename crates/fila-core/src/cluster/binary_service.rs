//! Binary protocol service for cluster inter-node communication.
//!
//! Replaces the gRPC-based `ClusterGrpcService` with a TCP listener that speaks
//! the FIBP (Fila Binary Protocol). Raft payloads remain protobuf-serialized
//! but are transported as opaque bytes inside binary protocol frames.

use std::sync::Arc;

use bytes::BytesMut;
use fila_fibp::{
    ClusterAddNodeRequest, ClusterAddNodeResponse, ClusterClientWriteRequest,
    ClusterClientWriteResponse, ClusterGetNodeInfoResponse, ClusterRaftRequest,
    ClusterRaftResponse, ClusterRemoveNodeRequest, ClusterRemoveNodeResponse, FrameError, Opcode,
    RawFrame,
};
use openraft::error::RaftError;
use openraft::{BasicNode, Raft};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tracing::{debug, error, info};

use super::multi_raft::MultiRaftManager;
use super::proto_convert;
use super::types::{NodeId, TypeConfig};
use crate::Broker;

/// Shared state for the cluster binary protocol service.
pub struct ClusterBinaryService {
    meta_raft: Arc<Raft<TypeConfig>>,
    multi_raft: Arc<MultiRaftManager>,
    broker: Arc<std::sync::OnceLock<Arc<Broker>>>,
    node_id: NodeId,
    client_addr: String,
}

impl ClusterBinaryService {
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

    async fn resolve_raft(&self, group_id: &str) -> Option<Arc<Raft<TypeConfig>>> {
        if group_id.is_empty() {
            Some(Arc::clone(&self.meta_raft))
        } else {
            self.multi_raft.get_raft(group_id).await
        }
    }
}

/// IO stream abstraction for plain TCP or TLS.
enum Stream {
    Plain(TcpStream),
    Tls(Box<tokio_rustls::server::TlsStream<TcpStream>>),
}

impl Stream {
    async fn read_buf(&mut self, buf: &mut BytesMut) -> std::io::Result<usize> {
        match self {
            Stream::Plain(s) => s.read_buf(buf).await,
            Stream::Tls(s) => s.read_buf(buf).await,
        }
    }

    async fn write_all(&mut self, buf: &[u8]) -> std::io::Result<()> {
        match self {
            Stream::Plain(s) => s.write_all(buf).await,
            Stream::Tls(s) => s.write_all(buf).await,
        }
    }

    async fn flush(&mut self) -> std::io::Result<()> {
        match self {
            Stream::Plain(s) => s.flush().await,
            Stream::Tls(s) => s.flush().await,
        }
    }
}

/// Start the cluster binary protocol listener. Runs until `shutdown` fires.
pub async fn run(
    service: Arc<ClusterBinaryService>,
    listener: TcpListener,
    tls_acceptor: Option<tokio_rustls::TlsAcceptor>,
    mut shutdown: tokio::sync::watch::Receiver<bool>,
) {
    info!(
        addr = %listener.local_addr().unwrap(),
        tls = tls_acceptor.is_some(),
        "cluster binary protocol service listening"
    );

    loop {
        tokio::select! {
            accept_result = listener.accept() => {
                match accept_result {
                    Ok((stream, peer)) => {
                        debug!(%peer, "new cluster connection");
                        let svc = Arc::clone(&service);
                        let tls = tls_acceptor.clone();
                        tokio::spawn(async move {
                            if let Err(e) = handle_connection(svc, stream, tls).await {
                                debug!(%peer, error = %e, "cluster connection ended");
                            }
                        });
                    }
                    Err(e) => {
                        error!(error = %e, "cluster accept failed");
                    }
                }
            }
            _ = shutdown.changed() => {
                if *shutdown.borrow() {
                    info!("cluster binary protocol service shutting down");
                    return;
                }
            }
        }
    }
}

/// Handle a single cluster peer connection.
async fn handle_connection(
    service: Arc<ClusterBinaryService>,
    tcp: TcpStream,
    tls_acceptor: Option<tokio_rustls::TlsAcceptor>,
) -> Result<(), ConnectionError> {
    let mut stream = match tls_acceptor {
        Some(acceptor) => {
            let tls_stream = acceptor.accept(tcp).await.map_err(ConnectionError::Tls)?;
            Stream::Tls(Box::new(tls_stream))
        }
        None => Stream::Plain(tcp),
    };

    let mut buf = BytesMut::with_capacity(8192);

    // Cluster connections skip the FIBP handshake — no auth needed for inter-node.
    // Go directly into the frame processing loop.
    loop {
        let frame = match read_frame(&mut stream, &mut buf).await {
            Ok(f) => f,
            Err(ConnectionError::Closed) => return Ok(()),
            Err(e) => return Err(e),
        };

        let response = dispatch(&service, &frame).await;

        let mut out = BytesMut::new();
        response.encode(&mut out);
        stream.write_all(&out).await.map_err(ConnectionError::Io)?;
        stream.flush().await.map_err(ConnectionError::Io)?;
    }
}

/// Read a single frame from the stream, buffering as needed.
async fn read_frame(stream: &mut Stream, buf: &mut BytesMut) -> Result<RawFrame, ConnectionError> {
    loop {
        if let Some(frame) = RawFrame::decode(buf).map_err(ConnectionError::Frame)? {
            return Ok(frame);
        }
        let n = stream.read_buf(buf).await.map_err(ConnectionError::Io)?;
        if n == 0 {
            return Err(ConnectionError::Closed);
        }
    }
}

/// Dispatch a cluster frame to the appropriate handler.
async fn dispatch(service: &ClusterBinaryService, frame: &RawFrame) -> RawFrame {
    let opcode = Opcode::from_u8(frame.opcode);
    let rid = frame.request_id;

    match opcode {
        Some(Opcode::RaftAppendEntries) => {
            handle_append_entries(service, rid, &frame.payload).await
        }
        Some(Opcode::RaftVote) => handle_vote(service, rid, &frame.payload).await,
        Some(Opcode::RaftInstallSnapshot) => {
            handle_install_snapshot(service, rid, &frame.payload).await
        }
        Some(Opcode::ClusterAddNode) => handle_add_node(service, rid, &frame.payload).await,
        Some(Opcode::ClusterRemoveNode) => handle_remove_node(service, rid, &frame.payload).await,
        Some(Opcode::ClusterClientWrite) => handle_client_write(service, rid, &frame.payload).await,
        Some(Opcode::ClusterGetNodeInfo) => handle_get_node_info(service, rid).await,
        _ => {
            // Unknown opcode — return an error response.
            error_response(
                rid,
                &format!("unknown cluster opcode: 0x{:02x}", frame.opcode),
            )
        }
    }
}

async fn handle_append_entries(
    service: &ClusterBinaryService,
    request_id: u32,
    payload: &[u8],
) -> RawFrame {
    let req = match ClusterRaftRequest::decode(bytes::Bytes::copy_from_slice(payload)) {
        Ok(r) => r,
        Err(e) => return error_response(request_id, &format!("decode: {e}")),
    };

    let raft = match service.resolve_raft(&req.group_id).await {
        Some(r) => r,
        None => {
            return error_response(request_id, &format!("unknown raft group: {}", req.group_id))
        }
    };

    // Deserialize the protobuf-encoded AppendEntries request.
    let proto_req: fila_proto::RaftAppendEntriesRequest = match prost::Message::decode(&*req.data) {
        Ok(r) => r,
        Err(e) => return error_response(request_id, &format!("proto decode: {e}")),
    };

    let raft_req = match proto_convert::append_entries_request_from_proto(proto_req) {
        Ok(r) => r,
        Err(e) => return error_response(request_id, &format!("convert: {e}")),
    };

    match raft.append_entries(raft_req).await {
        Ok(resp) => {
            let proto_resp = proto_convert::append_entries_response_to_proto(resp);
            let data = prost_encode(&proto_resp);
            ClusterRaftResponse { data }.encode(request_id, Opcode::RaftAppendEntriesResult)
        }
        Err(e) => error_response(request_id, &format!("raft: {e}")),
    }
}

async fn handle_vote(service: &ClusterBinaryService, request_id: u32, payload: &[u8]) -> RawFrame {
    let req = match ClusterRaftRequest::decode(bytes::Bytes::copy_from_slice(payload)) {
        Ok(r) => r,
        Err(e) => return error_response(request_id, &format!("decode: {e}")),
    };

    let raft = match service.resolve_raft(&req.group_id).await {
        Some(r) => r,
        None => {
            return error_response(request_id, &format!("unknown raft group: {}", req.group_id))
        }
    };

    let proto_req: fila_proto::RaftVoteRequest = match prost::Message::decode(&*req.data) {
        Ok(r) => r,
        Err(e) => return error_response(request_id, &format!("proto decode: {e}")),
    };

    let raft_req = match proto_convert::vote_request_from_proto(proto_req) {
        Ok(r) => r,
        Err(e) => return error_response(request_id, &format!("convert: {e}")),
    };

    match raft.vote(raft_req).await {
        Ok(resp) => {
            let proto_resp = proto_convert::vote_response_to_proto(resp);
            let data = prost_encode(&proto_resp);
            ClusterRaftResponse { data }.encode(request_id, Opcode::RaftVoteResult)
        }
        Err(e) => error_response(request_id, &format!("raft: {e}")),
    }
}

async fn handle_install_snapshot(
    service: &ClusterBinaryService,
    request_id: u32,
    payload: &[u8],
) -> RawFrame {
    let req = match ClusterRaftRequest::decode(bytes::Bytes::copy_from_slice(payload)) {
        Ok(r) => r,
        Err(e) => return error_response(request_id, &format!("decode: {e}")),
    };

    let raft = match service.resolve_raft(&req.group_id).await {
        Some(r) => r,
        None => {
            return error_response(request_id, &format!("unknown raft group: {}", req.group_id))
        }
    };

    let proto_req: fila_proto::RaftInstallSnapshotRequest = match prost::Message::decode(&*req.data)
    {
        Ok(r) => r,
        Err(e) => return error_response(request_id, &format!("proto decode: {e}")),
    };

    let raft_req = match proto_convert::install_snapshot_request_from_proto(proto_req) {
        Ok(r) => r,
        Err(e) => return error_response(request_id, &format!("convert: {e}")),
    };

    match raft.install_snapshot(raft_req).await {
        Ok(resp) => {
            let proto_resp = proto_convert::install_snapshot_response_to_proto(resp);
            let data = prost_encode(&proto_resp);
            ClusterRaftResponse { data }.encode(request_id, Opcode::RaftInstallSnapshotResult)
        }
        Err(e) => error_response(request_id, &format!("raft: {e}")),
    }
}

async fn handle_add_node(
    service: &ClusterBinaryService,
    request_id: u32,
    payload: &[u8],
) -> RawFrame {
    let req = match ClusterAddNodeRequest::decode(bytes::Bytes::copy_from_slice(payload)) {
        Ok(r) => r,
        Err(e) => return error_response(request_id, &format!("decode: {e}")),
    };

    let node_id = req.node_id;
    let node = BasicNode {
        addr: req.addr.clone(),
    };

    if !req.client_addr.is_empty() {
        service
            .multi_raft
            .register_client_addr(node_id, &req.client_addr)
            .await;
    }

    if let Err(e) = service.meta_raft.add_learner(node_id, node, true).await {
        let resp = handle_membership_error(e);
        return resp.encode(request_id);
    }

    let mut members = std::collections::BTreeSet::new();
    members.insert(node_id);
    match service
        .meta_raft
        .change_membership(openraft::ChangeMembers::AddVoterIds(members), false)
        .await
    {
        Ok(_) => ClusterAddNodeResponse {
            success: true,
            error: String::new(),
            leader_addr: String::new(),
        }
        .encode(request_id),
        Err(e) => handle_membership_error(e).encode(request_id),
    }
}

async fn handle_remove_node(
    service: &ClusterBinaryService,
    request_id: u32,
    payload: &[u8],
) -> RawFrame {
    let req = match ClusterRemoveNodeRequest::decode(bytes::Bytes::copy_from_slice(payload)) {
        Ok(r) => r,
        Err(e) => return error_response(request_id, &format!("decode: {e}")),
    };

    let mut members = std::collections::BTreeSet::new();
    members.insert(req.node_id);
    match service
        .meta_raft
        .change_membership(openraft::ChangeMembers::RemoveVoters(members), false)
        .await
    {
        Ok(_) => ClusterRemoveNodeResponse {
            success: true,
            error: String::new(),
            leader_addr: String::new(),
        }
        .encode(request_id),
        Err(e) => handle_remove_error(e).encode(request_id),
    }
}

async fn handle_client_write(
    service: &ClusterBinaryService,
    request_id: u32,
    payload: &[u8],
) -> RawFrame {
    let req = match ClusterClientWriteRequest::decode(bytes::Bytes::copy_from_slice(payload)) {
        Ok(r) => r,
        Err(e) => return error_response(request_id, &format!("decode: {e}")),
    };

    let raft = match service.resolve_raft(&req.group_id).await {
        Some(r) => r,
        None => {
            return error_response(request_id, &format!("unknown raft group: {}", req.group_id))
        }
    };

    // Decode the protobuf-encoded ClusterRequestProto.
    let proto_req: fila_proto::ClusterRequestProto = match prost::Message::decode(&*req.data) {
        Ok(r) => r,
        Err(e) => return error_response(request_id, &format!("proto decode: {e}")),
    };

    let cluster_req: super::types::ClusterRequest = match proto_req.try_into() {
        Ok(r) => r,
        Err(e) => return error_response(request_id, &format!("convert: {e}")),
    };

    match raft.client_write(cluster_req.clone()).await {
        Ok(resp) => {
            // Apply to local scheduler so forwarded writes have
            // real side effects on the leader node.
            if let Some(broker) = service.broker.get() {
                super::grpc_service::apply_to_scheduler(broker, &cluster_req).await;
            }

            let response_proto = fila_proto::ClusterResponseProto::from(resp.data);
            let data = prost_encode(&response_proto);
            ClusterClientWriteResponse { data }.encode(request_id)
        }
        Err(RaftError::APIError(openraft::error::ClientWriteError::ForwardToLeader(fwd))) => {
            let leader_addr = fwd
                .leader_node
                .as_ref()
                .map(|n| n.addr.clone())
                .unwrap_or_default();
            let response_proto =
                fila_proto::ClusterResponseProto::from(super::types::ClusterResponse::Error {
                    message: format!("ForwardToLeader:{leader_addr}"),
                });
            let data = prost_encode(&response_proto);
            ClusterClientWriteResponse { data }.encode(request_id)
        }
        Err(e) => error_response(request_id, &format!("raft: {e}")),
    }
}

async fn handle_get_node_info(service: &ClusterBinaryService, request_id: u32) -> RawFrame {
    ClusterGetNodeInfoResponse {
        node_id: service.node_id,
        client_addr: service.client_addr.clone(),
    }
    .encode(request_id)
}

/// Encode a prost message to bytes.
fn prost_encode(msg: &impl prost::Message) -> Vec<u8> {
    let mut buf = Vec::with_capacity(msg.encoded_len());
    msg.encode(&mut buf)
        .expect("prost encode to vec never fails");
    buf
}

/// Build an error response frame with the given message.
fn error_response(request_id: u32, message: &str) -> RawFrame {
    use fila_fibp::{ErrorCode, ErrorFrame};
    ErrorFrame {
        error_code: ErrorCode::InternalError,
        message: message.to_string(),
        metadata: std::collections::HashMap::new(),
    }
    .encode(request_id)
}

/// Convert a membership change error into an `ClusterAddNodeResponse` with leader hint.
fn handle_membership_error(
    error: RaftError<NodeId, openraft::error::ClientWriteError<NodeId, BasicNode>>,
) -> ClusterAddNodeResponse {
    match &error {
        RaftError::APIError(write_err) => match write_err {
            openraft::error::ClientWriteError::ForwardToLeader(fwd) => {
                let leader_addr = fwd
                    .leader_node
                    .as_ref()
                    .map(|n| n.addr.clone())
                    .unwrap_or_default();
                ClusterAddNodeResponse {
                    success: false,
                    error: "not leader".to_string(),
                    leader_addr,
                }
            }
            openraft::error::ClientWriteError::ChangeMembershipError(e) => ClusterAddNodeResponse {
                success: false,
                error: format!("{e}"),
                leader_addr: String::new(),
            },
        },
        RaftError::Fatal(e) => ClusterAddNodeResponse {
            success: false,
            error: format!("fatal: {e}"),
            leader_addr: String::new(),
        },
    }
}

/// Convert a membership change error into a `ClusterRemoveNodeResponse` with leader hint.
fn handle_remove_error(
    error: RaftError<NodeId, openraft::error::ClientWriteError<NodeId, BasicNode>>,
) -> ClusterRemoveNodeResponse {
    match &error {
        RaftError::APIError(write_err) => match write_err {
            openraft::error::ClientWriteError::ForwardToLeader(fwd) => {
                let leader_addr = fwd
                    .leader_node
                    .as_ref()
                    .map(|n| n.addr.clone())
                    .unwrap_or_default();
                ClusterRemoveNodeResponse {
                    success: false,
                    error: "not leader".to_string(),
                    leader_addr,
                }
            }
            openraft::error::ClientWriteError::ChangeMembershipError(e) => {
                ClusterRemoveNodeResponse {
                    success: false,
                    error: format!("{e}"),
                    leader_addr: String::new(),
                }
            }
        },
        RaftError::Fatal(e) => ClusterRemoveNodeResponse {
            success: false,
            error: format!("fatal: {e}"),
            leader_addr: String::new(),
        },
    }
}

#[derive(Debug)]
enum ConnectionError {
    Io(std::io::Error),
    Frame(FrameError),
    Tls(std::io::Error),
    Closed,
}

impl std::fmt::Display for ConnectionError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Io(e) => write!(f, "io: {e}"),
            Self::Frame(e) => write!(f, "frame: {e}"),
            Self::Tls(e) => write!(f, "tls: {e}"),
            Self::Closed => write!(f, "connection closed"),
        }
    }
}
