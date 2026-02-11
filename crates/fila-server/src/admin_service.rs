use std::sync::Arc;

use fila_core::{Broker, FilaError, QueueConfig, SchedulerCommand};
use fila_proto::fila_admin_server::FilaAdmin;
use fila_proto::{
    CreateQueueRequest, CreateQueueResponse, DeleteQueueRequest, DeleteQueueResponse,
    GetConfigRequest, GetConfigResponse, GetStatsRequest, GetStatsResponse, RedriveRequest,
    RedriveResponse, SetConfigRequest, SetConfigResponse,
};
use tonic::{Request, Response, Status};
use tracing::instrument;

/// gRPC admin service implementation. Wraps a Broker to send commands
/// to the scheduler thread.
pub struct AdminService {
    broker: Arc<Broker>,
}

impl AdminService {
    pub fn new(broker: Arc<Broker>) -> Self {
        Self { broker }
    }
}

fn fila_error_to_status(err: FilaError) -> Status {
    match err {
        FilaError::QueueNotFound(msg) => Status::not_found(msg),
        FilaError::QueueAlreadyExists(msg) => Status::already_exists(msg),
        FilaError::InvalidConfig(msg) => Status::invalid_argument(msg),
        FilaError::MessageNotFound(msg) => Status::not_found(msg),
        FilaError::LuaError(msg) => Status::internal(msg),
        FilaError::SchedulerSpawn(msg) => Status::internal(msg),
        FilaError::ChannelFull => Status::resource_exhausted("scheduler overloaded"),
        FilaError::ChannelDisconnected => Status::unavailable("scheduler unavailable"),
        FilaError::SchedulerPanicked => Status::internal("scheduler panicked"),
        FilaError::Storage(e) => Status::internal(e.to_string()),
    }
}

#[tonic::async_trait]
impl FilaAdmin for AdminService {
    #[instrument(skip(self))]
    async fn create_queue(
        &self,
        request: Request<CreateQueueRequest>,
    ) -> Result<Response<CreateQueueResponse>, Status> {
        let req = request.into_inner();

        if req.name.is_empty() {
            return Err(Status::invalid_argument("queue name must not be empty"));
        }

        let config = QueueConfig {
            name: req.name.clone(),
            on_enqueue_script: req.config.as_ref().and_then(|c| {
                if c.on_enqueue_script.is_empty() {
                    None
                } else {
                    Some(c.on_enqueue_script.clone())
                }
            }),
            on_failure_script: req.config.as_ref().and_then(|c| {
                if c.on_failure_script.is_empty() {
                    None
                } else {
                    Some(c.on_failure_script.clone())
                }
            }),
            visibility_timeout_ms: req
                .config
                .as_ref()
                .map(|c| c.visibility_timeout_ms)
                .filter(|&v| v > 0)
                .unwrap_or(QueueConfig::DEFAULT_VISIBILITY_TIMEOUT_MS),
            dlq_queue_id: None,
        };

        let (reply_tx, reply_rx) = tokio::sync::oneshot::channel();
        self.broker
            .send_command(SchedulerCommand::CreateQueue {
                name: req.name,
                config,
                reply: reply_tx,
            })
            .map_err(|e| Status::internal(e.to_string()))?;

        let queue_id = reply_rx
            .await
            .map_err(|_| Status::internal("scheduler reply channel dropped"))?
            .map_err(fila_error_to_status)?;

        Ok(Response::new(CreateQueueResponse { queue_id }))
    }

    #[instrument(skip(self))]
    async fn delete_queue(
        &self,
        request: Request<DeleteQueueRequest>,
    ) -> Result<Response<DeleteQueueResponse>, Status> {
        let req = request.into_inner();

        if req.queue.is_empty() {
            return Err(Status::invalid_argument("queue name must not be empty"));
        }

        let (reply_tx, reply_rx) = tokio::sync::oneshot::channel();
        self.broker
            .send_command(SchedulerCommand::DeleteQueue {
                queue_id: req.queue,
                reply: reply_tx,
            })
            .map_err(|e| Status::internal(e.to_string()))?;

        reply_rx
            .await
            .map_err(|_| Status::internal("scheduler reply channel dropped"))?
            .map_err(fila_error_to_status)?;

        Ok(Response::new(DeleteQueueResponse {}))
    }

    async fn set_config(
        &self,
        _request: Request<SetConfigRequest>,
    ) -> Result<Response<SetConfigResponse>, Status> {
        Err(Status::unimplemented("SetConfig not yet implemented"))
    }

    async fn get_config(
        &self,
        _request: Request<GetConfigRequest>,
    ) -> Result<Response<GetConfigResponse>, Status> {
        Err(Status::unimplemented("GetConfig not yet implemented"))
    }

    async fn get_stats(
        &self,
        _request: Request<GetStatsRequest>,
    ) -> Result<Response<GetStatsResponse>, Status> {
        Err(Status::unimplemented("GetStats not yet implemented"))
    }

    async fn redrive(
        &self,
        _request: Request<RedriveRequest>,
    ) -> Result<Response<RedriveResponse>, Status> {
        Err(Status::unimplemented("Redrive not yet implemented"))
    }
}
