use std::sync::Arc;

use bytes::BytesMut;
use fila_fibp::{ClusterRaftRequest, ClusterRaftResponse, Opcode, RawFrame};
use openraft::error::{InstallSnapshotError, RPCError, RaftError, Unreachable};
use openraft::network::RPCOption;
use openraft::raft::{
    AppendEntriesRequest, AppendEntriesResponse, InstallSnapshotRequest, InstallSnapshotResponse,
    VoteRequest, VoteResponse,
};
use openraft::{BasicNode, RaftNetwork, RaftNetworkFactory};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;
use tokio::sync::Mutex;

use super::proto_convert;
use super::types::{NodeId, TypeConfig};

/// TLS configuration for outgoing cluster connections.
#[derive(Clone)]
pub struct ClusterTlsConfig {
    pub connector: tokio_rustls::TlsConnector,
    pub server_name: rustls::pki_types::ServerName<'static>,
}

/// IO stream for cluster peer connections.
pub(super) enum PeerStream {
    Plain(TcpStream),
    Tls(Box<tokio_rustls::client::TlsStream<TcpStream>>),
}

impl PeerStream {
    pub(super) async fn read_buf(&mut self, buf: &mut BytesMut) -> std::io::Result<usize> {
        match self {
            PeerStream::Plain(s) => s.read_buf(buf).await,
            PeerStream::Tls(s) => s.read_buf(buf).await,
        }
    }

    pub(super) async fn write_all(&mut self, buf: &[u8]) -> std::io::Result<()> {
        match self {
            PeerStream::Plain(s) => s.write_all(buf).await,
            PeerStream::Tls(s) => s.write_all(buf).await,
        }
    }

    pub(super) async fn flush(&mut self) -> std::io::Result<()> {
        match self {
            PeerStream::Plain(s) => s.flush().await,
            PeerStream::Tls(s) => s.flush().await,
        }
    }
}

/// Connect to a peer over TCP, optionally upgrading to TLS.
pub(super) async fn connect_peer(
    addr: &str,
    tls: Option<&ClusterTlsConfig>,
) -> Result<PeerStream, Box<dyn std::error::Error + Send + Sync>> {
    // Strip any http:// or https:// prefix — we connect via raw TCP.
    let host = addr
        .strip_prefix("https://")
        .or_else(|| addr.strip_prefix("http://"))
        .unwrap_or(addr);

    let tcp = TcpStream::connect(host).await?;

    match tls {
        Some(tls_cfg) => {
            let tls_stream = tls_cfg
                .connector
                .connect(tls_cfg.server_name.clone(), tcp)
                .await?;
            Ok(PeerStream::Tls(Box::new(tls_stream)))
        }
        None => Ok(PeerStream::Plain(tcp)),
    }
}

/// Send a frame and read the response frame over a peer stream.
async fn send_recv(
    stream: &mut PeerStream,
    frame: RawFrame,
) -> Result<RawFrame, Box<dyn std::error::Error + Send + Sync>> {
    let mut out = BytesMut::new();
    frame.encode(&mut out);
    stream.write_all(&out).await?;
    stream.flush().await?;

    let mut buf = BytesMut::with_capacity(8192);
    loop {
        if let Some(resp) = RawFrame::decode(&mut buf)? {
            return Ok(resp);
        }
        let n = stream.read_buf(&mut buf).await?;
        if n == 0 {
            return Err("connection closed".into());
        }
    }
}

/// Factory that creates binary protocol network connections to peer nodes.
///
/// Each factory is scoped to a Raft group: the meta group uses an empty
/// `group_id`, while queue groups carry their queue ID so the remote node
/// can route the RPC to the correct Raft instance.
pub struct FilaNetworkFactory {
    group_id: String,
    tls: Option<Arc<ClusterTlsConfig>>,
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
    pub fn meta_with_tls(tls: Option<Arc<ClusterTlsConfig>>) -> Self {
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
    pub fn for_queue_with_tls(queue_id: String, tls: Option<Arc<ClusterTlsConfig>>) -> Self {
        Self {
            group_id: queue_id,
            tls,
        }
    }
}

impl RaftNetworkFactory<TypeConfig> for FilaNetworkFactory {
    type Network = FilaNetwork;

    async fn new_client(&mut self, _target: NodeId, node: &BasicNode) -> Self::Network {
        FilaNetwork {
            addr: node.addr.clone(),
            group_id: self.group_id.clone(),
            tls: self.tls.clone(),
            stream: Arc::new(Mutex::new(None)),
            request_counter: std::sync::atomic::AtomicU32::new(1),
        }
    }
}

/// A binary protocol network connection to a single peer node.
///
/// The underlying TCP stream is lazily established on first use
/// and reused for all subsequent RPCs to this peer. If the connection
/// breaks, it is re-established on the next call.
pub struct FilaNetwork {
    addr: String,
    group_id: String,
    tls: Option<Arc<ClusterTlsConfig>>,
    stream: Arc<Mutex<Option<PeerStream>>>,
    request_counter: std::sync::atomic::AtomicU32,
}

impl FilaNetwork {
    async fn get_stream(
        &self,
    ) -> Result<
        tokio::sync::MutexGuard<'_, Option<PeerStream>>,
        Box<dyn std::error::Error + Send + Sync>,
    > {
        let mut guard = self.stream.lock().await;
        if guard.is_none() {
            let stream = connect_peer(&self.addr, self.tls.as_deref()).await?;
            *guard = Some(stream);
        }
        Ok(guard)
    }

    fn next_request_id(&self) -> u32 {
        self.request_counter
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed)
    }

    /// Send a Raft RPC and receive the response. On connection error,
    /// clears the cached stream so the next call reconnects.
    async fn rpc(
        &self,
        frame: RawFrame,
    ) -> Result<RawFrame, Box<dyn std::error::Error + Send + Sync>> {
        let result = {
            let mut guard = self.get_stream().await?;
            let stream = guard.as_mut().expect("stream just initialized");
            send_recv(stream, frame).await
        };

        if result.is_err() {
            // Clear cached stream so next call reconnects.
            let mut guard = self.stream.lock().await;
            *guard = None;
        }

        result
    }
}

impl RaftNetwork<TypeConfig> for FilaNetwork {
    async fn append_entries(
        &mut self,
        rpc: AppendEntriesRequest<TypeConfig>,
        _option: RPCOption,
    ) -> Result<AppendEntriesResponse<NodeId>, RPCError<NodeId, BasicNode, RaftError<NodeId>>> {
        let proto_req = proto_convert::append_entries_request_to_proto(rpc, self.group_id.clone());
        let data = prost_encode(&proto_req);

        let req_frame = ClusterRaftRequest {
            group_id: self.group_id.clone(),
            data,
        }
        .encode(self.next_request_id(), Opcode::RaftAppendEntries);

        let resp_frame = self.rpc(req_frame).await.map_err(|e| {
            RPCError::Unreachable(Unreachable::new(&std::io::Error::other(e.to_string())))
        })?;

        // Check if we got an error frame back.
        if resp_frame.opcode == Opcode::Error as u8 {
            let err_msg = fila_fibp::ErrorFrame::decode(resp_frame.payload)
                .map(|e| e.message)
                .unwrap_or_else(|_| "unknown error".to_string());
            return Err(RPCError::Unreachable(Unreachable::new(
                &std::io::Error::other(err_msg),
            )));
        }

        let resp_data = ClusterRaftResponse::decode(resp_frame.payload).map_err(|e| {
            RPCError::Unreachable(Unreachable::new(&std::io::Error::other(format!(
                "decode: {e}"
            ))))
        })?;

        let proto_resp: fila_proto::RaftAppendEntriesResponse =
            prost::Message::decode(&*resp_data.data).map_err(|e| {
                RPCError::Unreachable(Unreachable::new(&std::io::Error::other(format!(
                    "proto: {e}"
                ))))
            })?;

        proto_convert::append_entries_response_from_proto(proto_resp)
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
        let data = prost_encode(&proto_req);

        let req_frame = ClusterRaftRequest {
            group_id: self.group_id.clone(),
            data,
        }
        .encode(self.next_request_id(), Opcode::RaftInstallSnapshot);

        let resp_frame = self.rpc(req_frame).await.map_err(|e| {
            RPCError::Unreachable(Unreachable::new(&std::io::Error::other(e.to_string())))
        })?;

        if resp_frame.opcode == Opcode::Error as u8 {
            let err_msg = fila_fibp::ErrorFrame::decode(resp_frame.payload)
                .map(|e| e.message)
                .unwrap_or_else(|_| "unknown error".to_string());
            return Err(RPCError::Unreachable(Unreachable::new(
                &std::io::Error::other(err_msg),
            )));
        }

        let resp_data = ClusterRaftResponse::decode(resp_frame.payload).map_err(|e| {
            RPCError::Unreachable(Unreachable::new(&std::io::Error::other(format!(
                "decode: {e}"
            ))))
        })?;

        let proto_resp: fila_proto::RaftInstallSnapshotResponse =
            prost::Message::decode(&*resp_data.data).map_err(|e| {
                RPCError::Unreachable(Unreachable::new(&std::io::Error::other(format!(
                    "proto: {e}"
                ))))
            })?;

        proto_convert::install_snapshot_response_from_proto(proto_resp)
            .map_err(|e| RPCError::Unreachable(Unreachable::new(&e)))
    }

    async fn vote(
        &mut self,
        rpc: VoteRequest<NodeId>,
        _option: RPCOption,
    ) -> Result<VoteResponse<NodeId>, RPCError<NodeId, BasicNode, RaftError<NodeId>>> {
        let proto_req = proto_convert::vote_request_to_proto(rpc, self.group_id.clone());
        let data = prost_encode(&proto_req);

        let req_frame = ClusterRaftRequest {
            group_id: self.group_id.clone(),
            data,
        }
        .encode(self.next_request_id(), Opcode::RaftVote);

        let resp_frame = self.rpc(req_frame).await.map_err(|e| {
            RPCError::Unreachable(Unreachable::new(&std::io::Error::other(e.to_string())))
        })?;

        if resp_frame.opcode == Opcode::Error as u8 {
            let err_msg = fila_fibp::ErrorFrame::decode(resp_frame.payload)
                .map(|e| e.message)
                .unwrap_or_else(|_| "unknown error".to_string());
            return Err(RPCError::Unreachable(Unreachable::new(
                &std::io::Error::other(err_msg),
            )));
        }

        let resp_data = ClusterRaftResponse::decode(resp_frame.payload).map_err(|e| {
            RPCError::Unreachable(Unreachable::new(&std::io::Error::other(format!(
                "decode: {e}"
            ))))
        })?;

        let proto_resp: fila_proto::RaftVoteResponse = prost::Message::decode(&*resp_data.data)
            .map_err(|e| {
                RPCError::Unreachable(Unreachable::new(&std::io::Error::other(format!(
                    "proto: {e}"
                ))))
            })?;

        proto_convert::vote_response_from_proto(proto_resp)
            .map_err(|e| RPCError::Unreachable(Unreachable::new(&e)))
    }
}

/// Encode a prost message to bytes.
fn prost_encode(msg: &impl prost::Message) -> Vec<u8> {
    let mut buf = Vec::with_capacity(msg.encoded_len());
    msg.encode(&mut buf)
        .expect("prost encode to vec never fails");
    buf
}
