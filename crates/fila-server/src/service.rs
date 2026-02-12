use std::sync::Arc;

use fila_core::{Broker, Message, ReadyMessage, SchedulerCommand};
use fila_proto::fila_service_server::FilaService;
use fila_proto::{
    AckRequest, AckResponse, EnqueueRequest, EnqueueResponse, LeaseRequest, LeaseResponse,
    NackRequest, NackResponse,
};
use tonic::{Request, Response, Status};
use tracing::{debug, instrument};
use uuid::Uuid;

use crate::error::IntoStatus;

/// gRPC hot-path service implementation for producers and consumers.
pub struct HotPathService {
    broker: Arc<Broker>,
}

impl HotPathService {
    pub fn new(broker: Arc<Broker>) -> Self {
        Self { broker }
    }
}

/// Convert a ReadyMessage to a protobuf Message for the LeaseResponse.
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
    #[instrument(skip(self))]
    async fn enqueue(
        &self,
        request: Request<EnqueueRequest>,
    ) -> Result<Response<EnqueueResponse>, Status> {
        let req = request.into_inner();

        if req.queue.is_empty() {
            return Err(Status::invalid_argument("queue name must not be empty"));
        }

        let now_ns = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos() as u64;

        let message = Message {
            id: Uuid::now_v7(),
            queue_id: req.queue,
            headers: req.headers,
            payload: req.payload,
            fairness_key: "default".to_string(),
            weight: 1,
            throttle_keys: vec![],
            attempt_count: 0,
            enqueued_at: now_ns,
            leased_at: None,
        };

        let (reply_tx, reply_rx) = tokio::sync::oneshot::channel();
        self.broker
            .send_command(SchedulerCommand::Enqueue {
                message,
                reply: reply_tx,
            })
            .map_err(IntoStatus::into_status)?;

        let msg_id = reply_rx
            .await
            .map_err(|_| Status::internal("scheduler reply channel dropped"))?
            .map_err(IntoStatus::into_status)?;

        Ok(Response::new(EnqueueResponse {
            message_id: msg_id.to_string(),
        }))
    }

    type LeaseStream = tokio_stream::wrappers::ReceiverStream<Result<LeaseResponse, Status>>;

    #[instrument(skip(self))]
    async fn lease(
        &self,
        request: Request<LeaseRequest>,
    ) -> Result<Response<Self::LeaseStream>, Status> {
        let req = request.into_inner();

        if req.queue.is_empty() {
            return Err(Status::invalid_argument("queue name must not be empty"));
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

        debug!(%consumer_id, queue = %req.queue, "lease stream opened");

        // Spawn a converter task that bridges ReadyMessage -> LeaseResponse
        // and handles cleanup on disconnect
        let broker = Arc::clone(&self.broker);
        let cid = consumer_id.clone();
        tokio::spawn(async move {
            loop {
                tokio::select! {
                    msg = ready_rx.recv() => {
                        match msg {
                            Some(ready) => {
                                let response = LeaseResponse {
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
            debug!(consumer_id = %cid, "lease stream closed, unregistering consumer");
            let _ = broker.send_command(SchedulerCommand::UnregisterConsumer { consumer_id: cid });
        });

        Ok(Response::new(tokio_stream::wrappers::ReceiverStream::new(
            stream_rx,
        )))
    }

    #[instrument(skip(self))]
    async fn ack(&self, request: Request<AckRequest>) -> Result<Response<AckResponse>, Status> {
        let req = request.into_inner();

        if req.queue.is_empty() {
            return Err(Status::invalid_argument("queue name must not be empty"));
        }
        if req.message_id.is_empty() {
            return Err(Status::invalid_argument("message_id must not be empty"));
        }

        let msg_id: Uuid = req
            .message_id
            .parse()
            .map_err(|_| Status::invalid_argument("invalid message_id format"))?;

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

        Ok(Response::new(AckResponse {}))
    }

    #[instrument(skip(self))]
    async fn nack(&self, request: Request<NackRequest>) -> Result<Response<NackResponse>, Status> {
        let req = request.into_inner();

        if req.queue.is_empty() {
            return Err(Status::invalid_argument("queue name must not be empty"));
        }
        if req.message_id.is_empty() {
            return Err(Status::invalid_argument("message_id must not be empty"));
        }

        let msg_id: Uuid = req
            .message_id
            .parse()
            .map_err(|_| Status::invalid_argument("invalid message_id format"))?;

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

        Ok(Response::new(NackResponse {}))
    }
}
