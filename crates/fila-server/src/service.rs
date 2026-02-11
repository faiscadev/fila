use std::sync::Arc;

use fila_core::{Broker, Message, SchedulerCommand};
use fila_proto::fila_service_server::FilaService;
use fila_proto::{
    AckRequest, AckResponse, EnqueueRequest, EnqueueResponse, LeaseRequest, LeaseResponse,
    NackRequest, NackResponse,
};
use tonic::{Request, Response, Status};
use tracing::instrument;
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

    async fn lease(
        &self,
        _request: Request<LeaseRequest>,
    ) -> Result<Response<Self::LeaseStream>, Status> {
        Err(Status::unimplemented("Lease not yet implemented"))
    }

    async fn ack(&self, _request: Request<AckRequest>) -> Result<Response<AckResponse>, Status> {
        Err(Status::unimplemented("Ack not yet implemented"))
    }

    async fn nack(&self, _request: Request<NackRequest>) -> Result<Response<NackResponse>, Status> {
        Err(Status::unimplemented("Nack not yet implemented"))
    }
}
