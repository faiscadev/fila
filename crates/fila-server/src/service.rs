use std::sync::Arc;

use fila_core::broker::command::{AckItem, NackItem};
use fila_core::cluster::{AckItemData, NackItemData};
use fila_core::{
    Broker, ClusterHandle, ClusterRequest, ClusterResponse, ClusterWriteError, Message,
    ReadyMessage, SchedulerCommand,
};
use fila_proto::fila_service_server::FilaService;
use fila_proto::{
    AckRequest, AckResponse, ConsumeRequest, ConsumeResponse, EnqueueRequest, EnqueueResponse,
    NackRequest, NackResponse,
};
use tonic::{Request, Response, Status};
use tracing::{debug, instrument};
use uuid::Uuid;

use crate::auth::ValidatedKeyId;
use crate::error::IntoStatus;

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

/// gRPC hot-path service implementation for producers and consumers.
pub struct HotPathService {
    broker: Arc<Broker>,
    cluster: Option<Arc<ClusterHandle>>,
}

impl HotPathService {
    pub fn new(broker: Arc<Broker>, cluster: Option<Arc<ClusterHandle>>) -> Self {
        Self { broker, cluster }
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
    #[instrument(skip(self, request), fields(queue_id, msg_id))]
    async fn enqueue(
        &self,
        request: Request<EnqueueRequest>,
    ) -> Result<Response<EnqueueResponse>, Status> {
        let caller = request
            .extensions()
            .get::<ValidatedKeyId>()
            .map(|k| k.0.clone());
        let req = request.into_inner();

        if req.queue.is_empty() {
            return Err(Status::invalid_argument("queue name must not be empty"));
        }

        // ACL check: produce permission on this queue.
        if let Some(ref caller) = caller {
            let permitted = self
                .broker
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
        tracing::Span::current().record("queue_id", req.queue.as_str());
        tracing::Span::current().record("msg_id", tracing::field::display(&msg_id));

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

        if let Some(ref cluster) = self.cluster {
            // Cluster mode: commit through the queue's Raft group.
            let result = cluster
                .write_to_queue(
                    &req.queue,
                    ClusterRequest::Enqueue {
                        messages: vec![message.clone()],
                        message: None,
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

            // Apply to local scheduler only if this node handled the
            // write (is the leader). Forwarded writes are applied by
            // the leader's ClientWrite handler.
            if result.handled_locally {
                let (reply_tx, reply_rx) = tokio::sync::oneshot::channel();
                self.broker
                    .send_command(SchedulerCommand::Enqueue {
                        messages: vec![message],
                        reply: reply_tx,
                    })
                    .map_err(IntoStatus::into_status)?;

                let results = reply_rx
                    .await
                    .map_err(|_| Status::internal("scheduler reply channel dropped"))?;
                // Unwrap batch-of-1 result
                results
                    .into_iter()
                    .next()
                    .unwrap_or(Err(fila_core::EnqueueError::QueueNotFound(
                        "unknown".into(),
                    )))
                    .map_err(IntoStatus::into_status)?;
            }

            Ok(Response::new(EnqueueResponse {
                message_id: msg_id.to_string(),
            }))
        } else {
            // Single-node mode: direct to scheduler (batch-of-1).
            let (reply_tx, reply_rx) = tokio::sync::oneshot::channel();
            self.broker
                .send_command(SchedulerCommand::Enqueue {
                    messages: vec![message],
                    reply: reply_tx,
                })
                .map_err(IntoStatus::into_status)?;

            let results = reply_rx
                .await
                .map_err(|_| Status::internal("scheduler reply channel dropped"))?;
            // Unwrap batch-of-1 result
            let msg_id = results
                .into_iter()
                .next()
                .unwrap_or(Err(fila_core::EnqueueError::QueueNotFound(
                    "unknown".into(),
                )))
                .map_err(IntoStatus::into_status)?;

            Ok(Response::new(EnqueueResponse {
                message_id: msg_id.to_string(),
            }))
        }
    }

    type ConsumeStream = tokio_stream::wrappers::ReceiverStream<Result<ConsumeResponse, Status>>;

    #[instrument(skip(self, request), fields(queue_id))]
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
        // and handles cleanup on disconnect
        let broker = Arc::clone(&self.broker);
        let cid = consumer_id.clone();
        tokio::spawn(async move {
            loop {
                tokio::select! {
                    msg = ready_rx.recv() => {
                        match msg {
                            Some(ready) => {
                                let response = ConsumeResponse {
                                    message: Some(ready_to_proto(ready)),
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

    #[instrument(skip(self, request), fields(queue_id, msg_id))]
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

        let ack_item = AckItem {
            queue_id: req.queue.clone(),
            msg_id,
        };

        if let Some(ref cluster) = self.cluster {
            // Cluster mode: commit through queue Raft.
            let result = cluster
                .write_to_queue(
                    &ack_item.queue_id,
                    ClusterRequest::Ack {
                        items: vec![AckItemData {
                            queue_id: ack_item.queue_id.clone(),
                            msg_id: ack_item.msg_id,
                        }],
                        queue_id: None,
                        msg_id: None,
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
                        items: vec![ack_item],
                        reply: reply_tx,
                    })
                    .map_err(IntoStatus::into_status)?;

                let results = reply_rx
                    .await
                    .map_err(|_| Status::internal("scheduler reply channel dropped"))?;
                // Unwrap batch-of-1 result
                results
                    .into_iter()
                    .next()
                    .unwrap_or(Err(fila_core::AckError::MessageNotFound("unknown".into())))
                    .map_err(IntoStatus::into_status)?;
            }
        } else {
            let (reply_tx, reply_rx) = tokio::sync::oneshot::channel();
            self.broker
                .send_command(SchedulerCommand::Ack {
                    items: vec![ack_item],
                    reply: reply_tx,
                })
                .map_err(IntoStatus::into_status)?;

            let results = reply_rx
                .await
                .map_err(|_| Status::internal("scheduler reply channel dropped"))?;
            // Unwrap batch-of-1 result
            results
                .into_iter()
                .next()
                .unwrap_or(Err(fila_core::AckError::MessageNotFound("unknown".into())))
                .map_err(IntoStatus::into_status)?;
        }

        Ok(Response::new(AckResponse {}))
    }

    #[instrument(skip(self, request), fields(queue_id, msg_id))]
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

        let nack_item = NackItem {
            queue_id: req.queue.clone(),
            msg_id,
            error: req.error.clone(),
        };

        if let Some(ref cluster) = self.cluster {
            // Cluster mode: commit through queue Raft.
            let result = cluster
                .write_to_queue(
                    &nack_item.queue_id,
                    ClusterRequest::Nack {
                        items: vec![NackItemData {
                            queue_id: nack_item.queue_id.clone(),
                            msg_id: nack_item.msg_id,
                            error: nack_item.error.clone(),
                        }],
                        queue_id: None,
                        msg_id: None,
                        error: None,
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
                        items: vec![nack_item],
                        reply: reply_tx,
                    })
                    .map_err(IntoStatus::into_status)?;

                let results = reply_rx
                    .await
                    .map_err(|_| Status::internal("scheduler reply channel dropped"))?;
                // Unwrap batch-of-1 result
                results
                    .into_iter()
                    .next()
                    .unwrap_or(Err(fila_core::NackError::MessageNotFound("unknown".into())))
                    .map_err(IntoStatus::into_status)?;
            }
        } else {
            let (reply_tx, reply_rx) = tokio::sync::oneshot::channel();
            self.broker
                .send_command(SchedulerCommand::Nack {
                    items: vec![nack_item],
                    reply: reply_tx,
                })
                .map_err(IntoStatus::into_status)?;

            let results = reply_rx
                .await
                .map_err(|_| Status::internal("scheduler reply channel dropped"))?;
            // Unwrap batch-of-1 result
            results
                .into_iter()
                .next()
                .unwrap_or(Err(fila_core::NackError::MessageNotFound("unknown".into())))
                .map_err(IntoStatus::into_status)?;
        }

        Ok(Response::new(NackResponse {}))
    }
}
