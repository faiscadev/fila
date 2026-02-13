use std::sync::Arc;

use fila_core::{Broker, QueueConfig, SchedulerCommand};
use fila_proto::fila_admin_server::FilaAdmin;
use fila_proto::{
    CreateQueueRequest, CreateQueueResponse, DeleteQueueRequest, DeleteQueueResponse,
    GetConfigRequest, GetConfigResponse, GetStatsRequest, GetStatsResponse, RedriveRequest,
    RedriveResponse, SetConfigRequest, SetConfigResponse,
};
use tonic::{Request, Response, Status};
use tracing::instrument;

use crate::error::IntoStatus;

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

        let proto_config = req.config.unwrap_or_default();

        let visibility_timeout_ms = match proto_config.visibility_timeout_ms {
            0 => QueueConfig::DEFAULT_VISIBILITY_TIMEOUT_MS, // proto3: 0 means unset
            v if v < 1_000 => {
                return Err(Status::invalid_argument(
                    "visibility_timeout_ms must be at least 1000 (1 second)",
                ));
            }
            v => v,
        };

        let config = QueueConfig {
            name: req.name.clone(),
            on_enqueue_script: if proto_config.on_enqueue_script.is_empty() {
                None
            } else {
                Some(proto_config.on_enqueue_script)
            },
            on_failure_script: if proto_config.on_failure_script.is_empty() {
                None
            } else {
                Some(proto_config.on_failure_script)
            },
            visibility_timeout_ms,
            dlq_queue_id: None,
            lua_timeout_ms: None,
            lua_memory_limit_bytes: None,
        };

        let (reply_tx, reply_rx) = tokio::sync::oneshot::channel();
        self.broker
            .send_command(SchedulerCommand::CreateQueue {
                name: req.name,
                config,
                reply: reply_tx,
            })
            .map_err(IntoStatus::into_status)?;

        let queue_id = reply_rx
            .await
            .map_err(|_| Status::internal("scheduler reply channel dropped"))?
            .map_err(IntoStatus::into_status)?;

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
            .map_err(IntoStatus::into_status)?;

        reply_rx
            .await
            .map_err(|_| Status::internal("scheduler reply channel dropped"))?
            .map_err(IntoStatus::into_status)?;

        Ok(Response::new(DeleteQueueResponse {}))
    }

    #[instrument(skip(self))]
    async fn set_config(
        &self,
        request: Request<SetConfigRequest>,
    ) -> Result<Response<SetConfigResponse>, Status> {
        let req = request.into_inner();

        if req.key.is_empty() {
            return Err(Status::invalid_argument("config key must not be empty"));
        }

        let (reply_tx, reply_rx) = tokio::sync::oneshot::channel();
        self.broker
            .send_command(SchedulerCommand::SetConfig {
                key: req.key,
                value: req.value,
                reply: reply_tx,
            })
            .map_err(IntoStatus::into_status)?;

        reply_rx
            .await
            .map_err(|_| Status::internal("scheduler reply channel dropped"))?
            .map_err(IntoStatus::into_status)?;

        Ok(Response::new(SetConfigResponse {}))
    }

    #[instrument(skip(self))]
    async fn get_config(
        &self,
        request: Request<GetConfigRequest>,
    ) -> Result<Response<GetConfigResponse>, Status> {
        let req = request.into_inner();

        if req.key.is_empty() {
            return Err(Status::invalid_argument("config key must not be empty"));
        }

        let (reply_tx, reply_rx) = tokio::sync::oneshot::channel();
        self.broker
            .send_command(SchedulerCommand::GetConfig {
                key: req.key,
                reply: reply_tx,
            })
            .map_err(IntoStatus::into_status)?;

        let value = reply_rx
            .await
            .map_err(|_| Status::internal("scheduler reply channel dropped"))?
            .map_err(IntoStatus::into_status)?;

        Ok(Response::new(GetConfigResponse {
            value: value.unwrap_or_default(),
        }))
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

#[cfg(test)]
mod tests {
    use super::*;
    use fila_core::{Broker, BrokerConfig, RocksDbStorage};
    use fila_proto::QueueConfig as ProtoQueueConfig;

    fn test_admin_service() -> (AdminService, tempfile::TempDir) {
        let dir = tempfile::tempdir().unwrap();
        let storage = Arc::new(RocksDbStorage::open(dir.path()).unwrap());
        let broker = Arc::new(Broker::new(BrokerConfig::default(), storage).unwrap());
        (AdminService::new(broker), dir)
    }

    #[tokio::test]
    async fn create_queue_rejects_invalid_visibility_timeout() {
        let (svc, _dir) = test_admin_service();

        let request = Request::new(CreateQueueRequest {
            name: "test-queue".to_string(),
            config: Some(ProtoQueueConfig {
                visibility_timeout_ms: 500, // too low
                ..Default::default()
            }),
        });

        let err = svc.create_queue(request).await.unwrap_err();
        assert_eq!(err.code(), tonic::Code::InvalidArgument);
        assert!(err.message().contains("visibility_timeout_ms"));
    }

    #[tokio::test]
    async fn create_queue_uses_default_when_timeout_is_zero() {
        let (svc, _dir) = test_admin_service();

        let request = Request::new(CreateQueueRequest {
            name: "test-queue".to_string(),
            config: Some(ProtoQueueConfig {
                visibility_timeout_ms: 0, // proto3 unset
                ..Default::default()
            }),
        });

        let resp = svc.create_queue(request).await.unwrap();
        assert_eq!(resp.into_inner().queue_id, "test-queue");
    }

    #[tokio::test]
    async fn create_queue_accepts_valid_visibility_timeout() {
        let (svc, _dir) = test_admin_service();

        let request = Request::new(CreateQueueRequest {
            name: "test-queue".to_string(),
            config: Some(ProtoQueueConfig {
                visibility_timeout_ms: 60_000,
                ..Default::default()
            }),
        });

        let resp = svc.create_queue(request).await.unwrap();
        assert_eq!(resp.into_inner().queue_id, "test-queue");
    }

    #[tokio::test]
    async fn set_config_with_valid_throttle_key() {
        let (svc, _dir) = test_admin_service();

        let request = Request::new(SetConfigRequest {
            key: "throttle.provider_a".to_string(),
            value: "10.0,100.0".to_string(),
        });

        svc.set_config(request).await.unwrap();

        // Verify via get_config
        let request = Request::new(GetConfigRequest {
            key: "throttle.provider_a".to_string(),
        });
        let resp = svc.get_config(request).await.unwrap();
        assert_eq!(resp.into_inner().value, "10.0,100.0");
    }

    #[tokio::test]
    async fn set_config_empty_key_returns_invalid_argument() {
        let (svc, _dir) = test_admin_service();

        let request = Request::new(SetConfigRequest {
            key: String::new(),
            value: "10.0,100.0".to_string(),
        });

        let err = svc.set_config(request).await.unwrap_err();
        assert_eq!(err.code(), tonic::Code::InvalidArgument);
    }

    #[tokio::test]
    async fn get_config_empty_key_returns_invalid_argument() {
        let (svc, _dir) = test_admin_service();

        let request = Request::new(GetConfigRequest { key: String::new() });

        let err = svc.get_config(request).await.unwrap_err();
        assert_eq!(err.code(), tonic::Code::InvalidArgument);
    }

    #[tokio::test]
    async fn get_config_missing_key_returns_empty_value() {
        let (svc, _dir) = test_admin_service();

        let request = Request::new(GetConfigRequest {
            key: "nonexistent".to_string(),
        });

        let resp = svc.get_config(request).await.unwrap();
        assert_eq!(resp.into_inner().value, "");
    }
}
