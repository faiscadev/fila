use std::sync::Arc;

use fila_core::{
    Broker, ClusterHandle, ClusterRequest, ClusterResponse, ClusterWriteError, Message,
    ReadyMessage, SchedulerCommand,
};
use fila_proto::fila_service_server::FilaService;
use fila_proto::{
    AckMessage, AckRequest, AckResponse, AckResult, AckSuccess, ConsumeRequest, ConsumeResponse,
    EnqueueMessage, EnqueueRequest, EnqueueResponse, EnqueueResult, NackMessage, NackRequest,
    NackResponse, NackResult, NackSuccess, StreamEnqueueRequest, StreamEnqueueResponse,
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

/// Convert a Status error code to the typed EnqueueErrorCode proto enum.
fn status_to_enqueue_error_code(code: tonic::Code) -> i32 {
    match code {
        tonic::Code::NotFound => fila_proto::EnqueueErrorCode::QueueNotFound as i32,
        tonic::Code::PermissionDenied => fila_proto::EnqueueErrorCode::PermissionDenied as i32,
        _ => fila_proto::EnqueueErrorCode::Storage as i32,
    }
}

/// Build an EnqueueResult proto from a success/failure outcome.
fn enqueue_result_from(result: Result<String, Status>) -> EnqueueResult {
    match result {
        Ok(msg_id) => EnqueueResult {
            result: Some(fila_proto::enqueue_result::Result::MessageId(msg_id)),
        },
        Err(status) => EnqueueResult {
            result: Some(fila_proto::enqueue_result::Result::Error(
                fila_proto::EnqueueError {
                    code: status_to_enqueue_error_code(status.code()),
                    message: status.message().to_string(),
                },
            )),
        },
    }
}

/// Build an AckResult proto from a success/failure outcome.
fn ack_result_from(result: Result<(), Status>) -> AckResult {
    match result {
        Ok(()) => AckResult {
            result: Some(fila_proto::ack_result::Result::Success(AckSuccess {})),
        },
        Err(status) => AckResult {
            result: Some(fila_proto::ack_result::Result::Error(
                fila_proto::AckError {
                    code: match status.code() {
                        tonic::Code::NotFound => fila_proto::AckErrorCode::MessageNotFound as i32,
                        tonic::Code::PermissionDenied => {
                            fila_proto::AckErrorCode::PermissionDenied as i32
                        }
                        _ => fila_proto::AckErrorCode::Storage as i32,
                    },
                    message: status.message().to_string(),
                },
            )),
        },
    }
}

/// Build a NackResult proto from a success/failure outcome.
fn nack_result_from(result: Result<(), Status>) -> NackResult {
    match result {
        Ok(()) => NackResult {
            result: Some(fila_proto::nack_result::Result::Success(NackSuccess {})),
        },
        Err(status) => NackResult {
            result: Some(fila_proto::nack_result::Result::Error(
                fila_proto::NackError {
                    code: match status.code() {
                        tonic::Code::NotFound => fila_proto::NackErrorCode::MessageNotFound as i32,
                        tonic::Code::PermissionDenied => {
                            fila_proto::NackErrorCode::PermissionDenied as i32
                        }
                        _ => fila_proto::NackErrorCode::Storage as i32,
                    },
                    message: status.message().to_string(),
                },
            )),
        },
    }
}

/// Process a single enqueue message. Used by the unary and streaming handlers.
async fn process_enqueue_message(
    caller: &Option<CallerKey>,
    broker: &Arc<Broker>,
    cluster: &Option<Arc<ClusterHandle>>,
    msg: EnqueueMessage,
) -> Result<String, Status> {
    if msg.queue.is_empty() {
        return Err(Status::invalid_argument("queue name must not be empty"));
    }

    // ACL check: produce permission on this queue.
    if let Some(ref caller) = caller {
        let permitted = broker
            .check_permission(
                caller,
                fila_core::broker::auth::Permission::Produce,
                &msg.queue,
            )
            .map_err(|e| Status::internal(format!("acl check error: {e}")))?;
        if !permitted {
            return Err(Status::permission_denied(format!(
                "key does not have produce permission on queue \"{}\"",
                msg.queue
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
        queue_id: msg.queue.clone(),
        headers: msg.headers,
        payload: msg.payload,
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
                &msg.queue,
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

/// Process a single ack message.
async fn process_ack_message(
    caller: &Option<CallerKey>,
    broker: &Arc<Broker>,
    cluster: &Option<Arc<ClusterHandle>>,
    msg: AckMessage,
) -> Result<(), Status> {
    if msg.queue.is_empty() {
        return Err(Status::invalid_argument("queue name must not be empty"));
    }
    if msg.message_id.is_empty() {
        return Err(Status::invalid_argument("message_id must not be empty"));
    }

    // ACL check: consume permission on this queue (ack is part of the consume lifecycle).
    if let Some(ref caller) = caller {
        let permitted = broker
            .check_permission(
                caller,
                fila_core::broker::auth::Permission::Consume,
                &msg.queue,
            )
            .map_err(|e| Status::internal(format!("acl check error: {e}")))?;
        if !permitted {
            return Err(Status::permission_denied(format!(
                "key does not have consume permission on queue \"{}\"",
                msg.queue
            )));
        }
    }

    let msg_id: Uuid = msg
        .message_id
        .parse()
        .map_err(|_| Status::invalid_argument("invalid message_id format"))?;

    if let Some(ref cluster) = cluster {
        let result = cluster
            .write_to_queue(
                &msg.queue,
                ClusterRequest::Ack {
                    queue_id: msg.queue.clone(),
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
            broker
                .send_command(SchedulerCommand::Ack {
                    queue_id: msg.queue,
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
        broker
            .send_command(SchedulerCommand::Ack {
                queue_id: msg.queue,
                msg_id,
                reply: reply_tx,
            })
            .map_err(IntoStatus::into_status)?;

        reply_rx
            .await
            .map_err(|_| Status::internal("scheduler reply channel dropped"))?
            .map_err(IntoStatus::into_status)?;
    }

    Ok(())
}

/// Process a single nack message.
async fn process_nack_message(
    caller: &Option<CallerKey>,
    broker: &Arc<Broker>,
    cluster: &Option<Arc<ClusterHandle>>,
    msg: NackMessage,
) -> Result<(), Status> {
    if msg.queue.is_empty() {
        return Err(Status::invalid_argument("queue name must not be empty"));
    }
    if msg.message_id.is_empty() {
        return Err(Status::invalid_argument("message_id must not be empty"));
    }

    // ACL check: consume permission on this queue (nack is part of the consume lifecycle).
    if let Some(ref caller) = caller {
        let permitted = broker
            .check_permission(
                caller,
                fila_core::broker::auth::Permission::Consume,
                &msg.queue,
            )
            .map_err(|e| Status::internal(format!("acl check error: {e}")))?;
        if !permitted {
            return Err(Status::permission_denied(format!(
                "key does not have consume permission on queue \"{}\"",
                msg.queue
            )));
        }
    }

    let msg_id: Uuid = msg
        .message_id
        .parse()
        .map_err(|_| Status::invalid_argument("invalid message_id format"))?;

    if let Some(ref cluster) = cluster {
        let result = cluster
            .write_to_queue(
                &msg.queue,
                ClusterRequest::Nack {
                    queue_id: msg.queue.clone(),
                    msg_id,
                    error: msg.error.clone(),
                },
            )
            .await
            .map_err(cluster_write_err_to_status)?;

        if let ClusterResponse::Error { message } = result.response {
            return Err(Status::internal(message));
        }

        if result.handled_locally {
            let (reply_tx, reply_rx) = tokio::sync::oneshot::channel();
            broker
                .send_command(SchedulerCommand::Nack {
                    queue_id: msg.queue,
                    msg_id,
                    error: msg.error,
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
        broker
            .send_command(SchedulerCommand::Nack {
                queue_id: msg.queue,
                msg_id,
                error: msg.error,
                reply: reply_tx,
            })
            .map_err(IntoStatus::into_status)?;

        reply_rx
            .await
            .map_err(|_| Status::internal("scheduler reply channel dropped"))?
            .map_err(IntoStatus::into_status)?;
    }

    Ok(())
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
    #[instrument(skip(self), fields(batch_size))]
    async fn enqueue(
        &self,
        request: Request<EnqueueRequest>,
    ) -> Result<Response<EnqueueResponse>, Status> {
        let caller = request
            .extensions()
            .get::<ValidatedKeyId>()
            .map(|k| k.0.clone());
        let req = request.into_inner();

        tracing::Span::current().record("batch_size", req.messages.len());

        let mut results = Vec::with_capacity(req.messages.len());
        for msg in req.messages {
            let result = process_enqueue_message(&caller, &self.broker, &self.cluster, msg).await;
            results.push(enqueue_result_from(result));
        }

        Ok(Response::new(EnqueueResponse { results }))
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
                // Wait for the first stream write (blocks until one arrives or stream ends).
                let first = match inbound.next().await {
                    Some(Ok(msg)) => msg,
                    Some(Err(_)) => break, // Transport error
                    None => break,         // Client closed stream
                };

                // Drain immediately available stream writes for coalescing.
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
                                _ => break,
                            }
                        }
                        _ = tokio::task::yield_now() => {
                            break;
                        }
                    }
                }

                // Process each stream write. Each write may contain multiple messages.
                for req in batch {
                    let seq = req.sequence_number;

                    let mut results = Vec::with_capacity(req.messages.len());
                    for msg in req.messages {
                        let result = process_enqueue_message(&caller, &broker, &cluster, msg).await;
                        results.push(enqueue_result_from(result));
                    }

                    let response = StreamEnqueueResponse {
                        sequence_number: seq,
                        results,
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

        // In cluster mode, consumers must connect to the queue's Raft leader.
        if let Some(ref cluster) = self.cluster {
            match cluster.is_queue_leader(&req.queue).await {
                Some(true) => { /* This node is the leader — proceed */ }
                Some(false) => {
                    let mut status = Status::unavailable(
                        "this node is not the leader for this queue; reconnect to the leader",
                    );
                    if let Some(addr) = cluster.get_queue_leader_client_addr(&req.queue).await {
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

        let (ready_tx, mut ready_rx) = tokio::sync::mpsc::channel::<ReadyMessage>(64);
        let (stream_tx, stream_rx) = tokio::sync::mpsc::channel(64);

        self.broker
            .send_command(SchedulerCommand::RegisterConsumer {
                queue_id: req.queue.clone(),
                consumer_id: consumer_id.clone(),
                tx: ready_tx,
            })
            .map_err(IntoStatus::into_status)?;

        debug!(%consumer_id, queue = %req.queue, "consume stream opened");

        let broker = Arc::clone(&self.broker);
        let cid = consumer_id.clone();
        let batch_max = self.delivery_batch_max;
        tokio::spawn(async move {
            loop {
                tokio::select! {
                    msg = ready_rx.recv() => {
                        match msg {
                            Some(first) => {
                                let mut batch = vec![first];
                                while batch.len() < batch_max {
                                    match ready_rx.try_recv() {
                                        Ok(ready) => batch.push(ready),
                                        Err(_) => break,
                                    }
                                }

                                // Always use the repeated messages field.
                                let response = ConsumeResponse {
                                    messages: batch.into_iter().map(ready_to_proto).collect(),
                                };

                                if stream_tx.send(Ok(response)).await.is_err() {
                                    break;
                                }
                            }
                            None => break,
                        }
                    }
                    _ = stream_tx.closed() => {
                        break;
                    }
                }
            }
            debug!(consumer_id = %cid, "consume stream closed, unregistering consumer");
            let _ = broker.send_command(SchedulerCommand::UnregisterConsumer { consumer_id: cid });
        });

        Ok(Response::new(tokio_stream::wrappers::ReceiverStream::new(
            stream_rx,
        )))
    }

    #[instrument(skip(self), fields(batch_size))]
    async fn ack(&self, request: Request<AckRequest>) -> Result<Response<AckResponse>, Status> {
        let caller = request
            .extensions()
            .get::<ValidatedKeyId>()
            .map(|k| k.0.clone());
        let req = request.into_inner();

        tracing::Span::current().record("batch_size", req.messages.len());

        let mut results = Vec::with_capacity(req.messages.len());
        for msg in req.messages {
            let result = process_ack_message(&caller, &self.broker, &self.cluster, msg).await;
            results.push(ack_result_from(result));
        }

        Ok(Response::new(AckResponse { results }))
    }

    #[instrument(skip(self), fields(batch_size))]
    async fn nack(&self, request: Request<NackRequest>) -> Result<Response<NackResponse>, Status> {
        let caller = request
            .extensions()
            .get::<ValidatedKeyId>()
            .map(|k| k.0.clone());
        let req = request.into_inner();

        tracing::Span::current().record("batch_size", req.messages.len());

        let mut results = Vec::with_capacity(req.messages.len());
        for msg in req.messages {
            let result = process_nack_message(&caller, &self.broker, &self.cluster, msg).await;
            results.push(nack_result_from(result));
        }

        Ok(Response::new(NackResponse { results }))
    }
}
