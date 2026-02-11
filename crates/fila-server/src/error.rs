use fila_core::{BrokerError, CreateQueueError, DeleteQueueError};
use tonic::Status;

pub trait IntoStatus {
    fn into_status(self) -> Status;
}

impl IntoStatus for CreateQueueError {
    fn into_status(self) -> Status {
        match self {
            CreateQueueError::QueueAlreadyExists(msg) => Status::already_exists(msg),
            CreateQueueError::Storage(e) => Status::internal(e.to_string()),
        }
    }
}

impl IntoStatus for DeleteQueueError {
    fn into_status(self) -> Status {
        match self {
            DeleteQueueError::QueueNotFound(msg) => Status::not_found(msg),
            DeleteQueueError::Storage(e) => Status::internal(e.to_string()),
        }
    }
}

impl IntoStatus for BrokerError {
    fn into_status(self) -> Status {
        match self {
            BrokerError::SchedulerSpawn(msg) => Status::internal(msg),
            BrokerError::ChannelFull => Status::resource_exhausted("scheduler overloaded"),
            BrokerError::ChannelDisconnected => Status::unavailable("scheduler unavailable"),
            BrokerError::SchedulerPanicked => Status::internal("scheduler panicked"),
        }
    }
}
