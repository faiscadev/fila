use std::sync::Arc;

use fila_core::{
    Broker, ClusterHandle, ClusterRequest, ClusterResponse, ClusterWriteError, Message,
    ReadyMessage, SchedulerCommand,
};
use fila_proto::fila_service_server::FilaService;
use fila_proto::{
    AckRequest, AckResponse, BatchEnqueueRequest, BatchEnqueueResponse, BatchEnqueueResult,
    ConsumeRequest, ConsumeResponse, EnqueueRequest, EnqueueResponse, NackRequest, NackResponse,
    StreamEnqueueRequest, StreamEnqueueResponse,
};
use tonic::{Request, Response, Status};
use tracing::{debug, instrument};
use uuid::Uuid;

use crate::auth::ValidatedKeyId;
use crate::error::IntoStatus;

use fila_core::broker::auth::CallerKey;

/// Map cluster write errors to appropriate gRPC status codes.
fn cluster_write_err_to_status(err: ClusterWriteError) -> Status {
    match err {
        ClusterWriteError::QueueGroupNotFound => Status::not_found("queue raft group not found"),
        ClusterWriteError::NodeNotReady => {
            Status::unavailable("node not ready for this queue — retry on another node or wait")
        }
        ClusterWriteError::NoLeader => Status::unavailable("no leader available"),
        ClusterWriteError::Raft(e) => Status::internal(format!("raft error: {e}")),
        ClusterWriteError::Forward(e) => Status::unavailable(format!("forward error: {e}")),
    }
}

/// Process a single enqueue request without requiring `&self`. Used by the streaming handler
/// which runs in a spawned task that cannot hold a reference to `HotPathService`.
async fn enqueue_single_standalone(
    caller: &Option<CallerKey>,
    broker: &Arc<Broker>,
    cluster: &Option<Arc<ClusterHandle>>,
    req: EnqueueRequest,
) -> Result<String, Status> {
    if req.queue.is_empty() {
        return Err(Status::invalid_argument("queue name must not be empty"));
    }

    // ACL check: produce permission on this queue.
    if let Some(ref caller) = caller {
        let permitted = broker
            .check_permission(
                caller,
                fila_core::broker::auth::Permission::Produce,
                &req.queue,
            )
            .map_err(|e| Status::internal(format!("acl check error: {e}")))?;
        if !permitted {
            return Err(Status::permission_denied(format!(
                "key does not have produce permission on queue \"{}\"",
                req.queue
            )));
        }
    }

    let now_ns = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos() as u64;

    let msg_id = Uuid::now_v7();

    let message = Message {
        id: msg_id,
        queue_id: req.queue.clone(),
        headers: req.headers,
        payload: req.payload,
        fairness_key: "default".to_string(),
        weight: 1,
        throttle_keys: vec![],
        attempt_count: 0,
        enqueued_at: now_ns,
        leased_at: None,
    };

    if let Some(ref cluster) = cluster {
        let result = cluster
            .write_to_queue(
                &req.queue,
                ClusterRequest::Enqueue {
                    message: message.clone(),
                },
            )
            .await
            .map_err(cluster_write_err_to_status)?;

        let msg_id = match result.response {
            ClusterResponse::Enqueue { msg_id } => msg_id,
            ClusterResponse::Error { message } => {
                return Err(Status::internal(message));
            }
            _ => return Err(Status::internal("unexpected cluster response")),
        };

        if result.handled_locally {
            let (reply_tx, reply_rx) = tokio::sync::oneshot::channel();
            broker
                .send_command(SchedulerCommand::Enqueue {
                    message,
                    reply: reply_tx,
                })
                .map_err(IntoStatus::into_status)?;

            let _ = reply_rx
                .await
                .map_err(|_| Status::internal("scheduler reply channel dropped"))?
                .map_err(IntoStatus::into_status)?;
        }

        Ok(msg_id.to_string())
    } else {
        let (reply_tx, reply_rx) = tokio::sync::oneshot::channel();
        broker
            .send_command(SchedulerCommand::Enqueue {
                message,
                reply: reply_tx,
            })
            .map_err(IntoStatus::into_status)?;

        let msg_id = reply_rx
            .await
            .map_err(|_| Status::internal("scheduler reply channel dropped"))?
            .map_err(IntoStatus::into_status)?;

        Ok(msg_id.to_string())
    }
}

/// gRPC hot-path service implementation for producers and consumers.
pub struct HotPathService {
    broker: Arc<Broker>,
    cluster: Option<Arc<ClusterHandle>>,
    /// Maximum messages to batch in a single `ConsumeResponse` frame.
    delivery_batch_max: usize,
}

impl HotPathService {
    pub fn new(
        broker: Arc<Broker>,
        cluster: Option<Arc<ClusterHandle>>,
        delivery_batch_max: usize,
    ) -> Self {
        Self {
            broker,
            cluster,
            delivery_batch_max,
        }
    }
}

impl HotPathService {
    /// Process a single enqueue request. Used by both the `Enqueue` and `BatchEnqueue` RPCs.
    ///
    /// Returns the message ID string on success, or a gRPC `Status` on failure.
    async fn enqueue_single(
        &self,
        caller: &Option<CallerKey>,
        req: EnqueueRequest,
    ) -> Result<String, Status> {
        enqueue_single_standalone(caller, &self.broker, &self.cluster, req).await
    }
}

/// Convert a ReadyMessage to a protobuf Message for the ConsumeResponse.
fn ready_to_proto(ready: ReadyMessage) -> fila_proto::Message {
    fila_proto::Message {
        id: ready.msg_id.to_string(),
        headers: ready.headers,
        payload: ready.payload,
        metadata: Some(fila_proto::MessageMetadata {
            fairness_key: ready.fairness_key,
            weight: ready.weight,
            throttle_keys: ready.throttle_keys,
            attempt_count: ready.attempt_count,
            queue_id: ready.queue_id,
        }),
        timestamps: None,
    }
}
#[tonic::async_trait]
impl FilaService for HotPathService {
    #[instrument(skip(self), fields(queue_id, msg_id))]
    async fn enqueue(
        &self,
        request: Request<EnqueueRequest>,
    ) -> Result<Response<EnqueueResponse>, Status> {
        let caller = request
            .extensions()
            .get::<ValidatedKeyId>()
            .map(|k| k.0.clone());
        let req = request.into_inner();

        tracing::Span::current().record("queue_id", req.queue.as_str());

        let msg_id = self.enqueue_single(&caller, req).await?;

        tracing::Span::current().record("msg_id", msg_id.as_str());

        Ok(Response::new(EnqueueResponse { message_id: msg_id }))
    }

    #[instrument(skip(self), fields(batch_size))]
    async fn batch_enqueue(
        &self,
        request: Request<BatchEnqueueRequest>,
    ) -> Result<Response<BatchEnqueueResponse>, Status> {
        let caller = request
            .extensions()
            .get::<ValidatedKeyId>()
            .map(|k| k.0.clone());
        let req = request.into_inner();

        tracing::Span::current().record("batch_size", req.messages.len());

        let mut results = Vec::with_capacity(req.messages.len());
        for enqueue_req in req.messages {
            let result = self.enqueue_single(&caller, enqueue_req).await;
            results.push(match result {
                Ok(msg_id) => BatchEnqueueResult {
                    result: Some(fila_proto::batch_enqueue_result::Result::Success(
                        EnqueueResponse { message_id: msg_id },
                    )),
                },
                Err(status) => BatchEnqueueResult {
                    result: Some(fila_proto::batch_enqueue_result::Result::Error(
                        status.message().to_string(),
                    )),
                },
            });
        }

        Ok(Response::new(BatchEnqueueResponse { results }))
    }

    type StreamEnqueueStream =
        tokio_stream::wrappers::ReceiverStream<Result<StreamEnqueueResponse, Status>>;

    async fn stream_enqueue(
        &self,
        request: Request<tonic::Streaming<StreamEnqueueRequest>>,
    ) -> Result<Response<Self::StreamEnqueueStream>, Status> {
        let caller = request
            .extensions()
            .get::<ValidatedKeyId>()
            .map(|k| k.0.clone());
        let mut inbound = request.into_inner();

        let (resp_tx, resp_rx) =
            tokio::sync::mpsc::channel::<Result<StreamEnqueueResponse, Status>>(64);

        let broker = Arc::clone(&self.broker);
        let cluster = self.cluster.clone();

        tokio::spawn(async move {
            use tokio_stream::StreamExt;

            loop {
                // Wait for the first message (blocks until one arrives or stream ends).
                let first = match inbound.next().await {
                    Some(Ok(msg)) => msg,
                    Some(Err(_)) => break, // Transport error
                    None => break,         // Client closed stream
                };

                // Drain immediately available messages for write coalescing.
                // Use poll_next via tokio::select! with a zero-duration sleep as
                // the competing branch so we only take what's already buffered.
                let mut batch = vec![first];
                loop {
                    tokio::select! {
                        biased;
                        msg = inbound.next() => {
                            match msg {
                                Some(Ok(m)) => {
                                    batch.push(m);
                                    if batch.len() >= 256 {
                                        break; // Cap batch size
                                    }
                                }
                                _ => {
                                    // Stream ended or error — process what we have and exit
                                    // after sending responses.
                                    break;
                                }
                            }
                        }
                        _ = tokio::task::yield_now() => {
                            // No more immediately available messages.
                            break;
                        }
                    }
                }

                // Process each message through the scheduler pipeline and collect responses.
                for req in batch {
                    let seq = req.sequence_number;
                    let enqueue_req = EnqueueRequest {
                        queue: req.queue,
                        headers: req.headers,
                        payload: req.payload,
                    };

                    let result = enqueue_single_standalone(
                        &caller, &broker, &cluster, enqueue_req,
                    )
                    .await;

                    let response = match result {
                        Ok(msg_id) => StreamEnqueueResponse {
                            sequence_number: seq,
                            result: Some(
                                fila_proto::stream_enqueue_response::Result::MessageId(msg_id),
                            ),
                        },
                        Err(status) => StreamEnqueueResponse {
                            sequence_number: seq,
                            result: Some(
                                fila_proto::stream_enqueue_response::Result::Error(
                                    status.message().to_string(),
                                ),
                            ),
                        },
                    };

                    if resp_tx.send(Ok(response)).await.is_err() {
                        return; // Client disconnected from response stream
                    }
                }
            }
        });

        Ok(Response::new(tokio_stream::wrappers::ReceiverStream::new(
            resp_rx,
        )))
    }

    type ConsumeStream = tokio_stream::wrappers::ReceiverStream<Result<ConsumeResponse, Status>>;

    #[instrument(skip(self), fields(queue_id))]
    async fn consume(
        &self,
        request: Request<ConsumeRequest>,
    ) -> Result<Response<Self::ConsumeStream>, Status> {
        let caller = request
            .extensions()
            .get::<ValidatedKeyId>()
            .map(|k| k.0.clone());
        let req = request.into_inner();

        if req.queue.is_empty() {
            return Err(Status::invalid_argument("queue name must not be empty"));
        }

        // ACL check: consume permission on this queue.
        if let Some(ref caller) = caller {
            let permitted = self
                .broker
                .check_permission(
                    caller,
                    fila_core::broker::auth::Permission::Consume,
                    &req.queue,
                )
                .map_err(|e| Status::internal(format!("acl check error: {e}")))?;
            if !permitted {
                return Err(Status::permission_denied(format!(
                    "key does not have consume permission on queue \"{}\"",
                    req.queue
                )));
            }
        }

        tracing::Span::current().record("queue_id", req.queue.as_str());

        // In cluster mode, consumers must connect to the queue's Raft leader
        // so the DRR scheduler on the leader can serve them directly.
        if let Some(ref cluster) = self.cluster {
            match cluster.is_queue_leader(&req.queue).await {
                Some(true) => { /* This node is the leader — proceed */ }
                Some(false) => {
                    let mut status = Status::unavailable(
                        "this node is not the leader for this queue; reconnect to the leader",
                    );
                    // Include the leader's client address in gRPC metadata so
                    // SDKs can transparently reconnect to the correct node.
                    if let Some(addr) = cluster.get_queue_leader_client_addr(&req.queue).await {
                        // Don't advertise wildcard addresses (0.0.0.0) — they're
                        // not routable from clients. Only send the hint when the
                        // leader's address is a concrete host.
                        if !addr.starts_with("0.0.0.0") {
                            if let Ok(val) = addr.parse() {
                                status.metadata_mut().insert("x-fila-leader-addr", val);
                            }
                        }
                    }
                    return Err(status);
                }
                None => {
                    return Err(Status::not_found("queue raft group not found"));
                }
            }
        }

        let consumer_id = Uuid::now_v7().to_string();

        // Channel from scheduler (ReadyMessage) to converter task
        let (ready_tx, mut ready_rx) = tokio::sync::mpsc::channel::<ReadyMessage>(64);

        // Channel from converter task to gRPC stream
        let (stream_tx, stream_rx) = tokio::sync::mpsc::channel(64);

        // Register consumer with the scheduler
        self.broker
            .send_command(SchedulerCommand::RegisterConsumer {
                queue_id: req.queue.clone(),
                consumer_id: consumer_id.clone(),
                tx: ready_tx,
            })
            .map_err(IntoStatus::into_status)?;

        debug!(%consumer_id, queue = %req.queue, "consume stream opened");

        // Spawn a converter task that bridges ReadyMessage -> ConsumeResponse
        // and handles cleanup on disconnect. When multiple messages are immediately
        // available, they are batched into a single ConsumeResponse frame to
        // amortize HTTP/2 framing and protobuf encoding overhead.
        let broker = Arc::clone(&self.broker);
        let cid = consumer_id.clone();
        let batch_max = self.delivery_batch_max;
        tokio::spawn(async move {
            loop {
                tokio::select! {
                    msg = ready_rx.recv() => {
                        match msg {
                            Some(first) => {
                                // Try to collect more immediately-available messages
                                // (non-blocking) to batch into one frame.
                                let mut batch = vec![first];
                                while batch.len() < batch_max {
                                    match ready_rx.try_recv() {
                                        Ok(ready) => batch.push(ready),
                                        Err(_) => break,
                                    }
                                }

                                let response = if batch.len() == 1 {
                                    // Single message: use the `message` field for
                                    // backward compatibility with older SDKs.
                                    ConsumeResponse {
                                        message: Some(ready_to_proto(batch.pop().unwrap())),
                                        messages: vec![],
                                    }
                                } else {
                                    // Batched: populate the `messages` repeated field.
                                    ConsumeResponse {
                                        message: None,
                                        messages: batch.into_iter().map(ready_to_proto).collect(),
                                    }
                                };

                                if stream_tx.send(Ok(response)).await.is_err() {
                                    break; // gRPC stream closed
                                }
                            }
                            None => break, // Scheduler dropped our sender
                        }
                    }
                    _ = stream_tx.closed() => {
                        break; // Client disconnected
                    }
                }
            }
            // Unregister consumer when the stream ends
            debug!(consumer_id = %cid, "consume stream closed, unregistering consumer");
            let _ = broker.send_command(SchedulerCommand::UnregisterConsumer { consumer_id: cid });
        });

        Ok(Response::new(tokio_stream::wrappers::ReceiverStream::new(
            stream_rx,
        )))
    }

    #[instrument(skip(self), fields(queue_id, msg_id))]
    async fn ack(&self, request: Request<AckRequest>) -> Result<Response<AckResponse>, Status> {
        let caller = request
            .extensions()
            .get::<ValidatedKeyId>()
            .map(|k| k.0.clone());
        let req = request.into_inner();

        if req.queue.is_empty() {
            return Err(Status::invalid_argument("queue name must not be empty"));
        }

        // ACL check: consume permission on this queue (ack is part of the consume lifecycle).
        if let Some(ref caller) = caller {
            let permitted = self
                .broker
                .check_permission(
                    caller,
                    fila_core::broker::auth::Permission::Consume,
                    &req.queue,
                )
                .map_err(|e| Status::internal(format!("acl check error: {e}")))?;
            if !permitted {
                return Err(Status::permission_denied(format!(
                    "key does not have consume permission on queue \"{}\"",
                    req.queue
                )));
            }
        }
        if req.message_id.is_empty() {
            return Err(Status::invalid_argument("message_id must not be empty"));
        }

        tracing::Span::current().record("queue_id", req.queue.as_str());
        tracing::Span::current().record("msg_id", req.message_id.as_str());

        let msg_id: Uuid = req
            .message_id
            .parse()
            .map_err(|_| Status::invalid_argument("invalid message_id format"))?;

        if let Some(ref cluster) = self.cluster {
            // Cluster mode: commit through queue Raft.
            let result = cluster
                .write_to_queue(
                    &req.queue,
                    ClusterRequest::Ack {
                        queue_id: req.queue.clone(),
                        msg_id,
                    },
                )
                .await
                .map_err(cluster_write_err_to_status)?;

            if let ClusterResponse::Error { message } = result.response {
                return Err(Status::internal(message));
            }

            if result.handled_locally {
                let (reply_tx, reply_rx) = tokio::sync::oneshot::channel();
                self.broker
                    .send_command(SchedulerCommand::Ack {
                        queue_id: req.queue,
                        msg_id,
                        reply: reply_tx,
                    })
                    .map_err(IntoStatus::into_status)?;

                reply_rx
                    .await
                    .map_err(|_| Status::internal("scheduler reply channel dropped"))?
                    .map_err(IntoStatus::into_status)?;
            }
        } else {
            let (reply_tx, reply_rx) = tokio::sync::oneshot::channel();
            self.broker
                .send_command(SchedulerCommand::Ack {
                    queue_id: req.queue,
                    msg_id,
                    reply: reply_tx,
                })
                .map_err(IntoStatus::into_status)?;

            reply_rx
                .await
                .map_err(|_| Status::internal("scheduler reply channel dropped"))?
                .map_err(IntoStatus::into_status)?;
        }

        Ok(Response::new(AckResponse {}))
    }

    #[instrument(skip(self), fields(queue_id, msg_id))]
    async fn nack(&self, request: Request<NackRequest>) -> Result<Response<NackResponse>, Status> {
        let caller = request
            .extensions()
            .get::<ValidatedKeyId>()
            .map(|k| k.0.clone());
        let req = request.into_inner();

        if req.queue.is_empty() {
            return Err(Status::invalid_argument("queue name must not be empty"));
        }

        // ACL check: consume permission on this queue (nack is part of the consume lifecycle).
        if let Some(ref caller) = caller {
            let permitted = self
                .broker
                .check_permission(
                    caller,
                    fila_core::broker::auth::Permission::Consume,
                    &req.queue,
                )
                .map_err(|e| Status::internal(format!("acl check error: {e}")))?;
            if !permitted {
                return Err(Status::permission_denied(format!(
                    "key does not have consume permission on queue \"{}\"",
                    req.queue
                )));
            }
        }

        if req.message_id.is_empty() {
            return Err(Status::invalid_argument("message_id must not be empty"));
        }

        tracing::Span::current().record("queue_id", req.queue.as_str());
        tracing::Span::current().record("msg_id", req.message_id.as_str());

        let msg_id: Uuid = req
            .message_id
            .parse()
            .map_err(|_| Status::invalid_argument("invalid message_id format"))?;

        if let Some(ref cluster) = self.cluster {
            // Cluster mode: commit through queue Raft.
            let result = cluster
                .write_to_queue(
                    &req.queue,
                    ClusterRequest::Nack {
                        queue_id: req.queue.clone(),
                        msg_id,
                        error: req.error.clone(),
                    },
                )
                .await
                .map_err(cluster_write_err_to_status)?;

            if let ClusterResponse::Error { message } = result.response {
                return Err(Status::internal(message));
            }

            if result.handled_locally {
                let (reply_tx, reply_rx) = tokio::sync::oneshot::channel();
                self.broker
                    .send_command(SchedulerCommand::Nack {
                        queue_id: req.queue,
                        msg_id,
                        error: req.error,
                        reply: reply_tx,
                    })
                    .map_err(IntoStatus::into_status)?;

                reply_rx
                    .await
                    .map_err(|_| Status::internal("scheduler reply channel dropped"))?
                    .map_err(IntoStatus::into_status)?;
            }
        } else {
            let (reply_tx, reply_rx) = tokio::sync::oneshot::channel();
            self.broker
                .send_command(SchedulerCommand::Nack {
                    queue_id: req.queue,
                    msg_id,
                    error: req.error,
                    reply: reply_tx,
                })
                .map_err(IntoStatus::into_status)?;

            reply_rx
                .await
                .map_err(|_| Status::internal("scheduler reply channel dropped"))?
                .map_err(IntoStatus::into_status)?;
        }

        Ok(Response::new(NackResponse {}))
    }
}
