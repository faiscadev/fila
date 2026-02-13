use fila_core::{
    AckError, BrokerError, ConfigError, CreateQueueError, DeleteQueueError, EnqueueError, NackError,
};
use tonic::Status;

pub trait IntoStatus {
    fn into_status(self) -> Status;
}

impl IntoStatus for EnqueueError {
    fn into_status(self) -> Status {
        match self {
            EnqueueError::QueueNotFound(msg) => Status::not_found(msg),
            EnqueueError::Storage(e) => Status::internal(e.to_string()),
        }
    }
}

impl IntoStatus for AckError {
    fn into_status(self) -> Status {
        match self {
            AckError::MessageNotFound(msg) => Status::not_found(msg),
            AckError::Storage(e) => Status::internal(e.to_string()),
        }
    }
}

impl IntoStatus for NackError {
    fn into_status(self) -> Status {
        match self {
            NackError::MessageNotFound(msg) => Status::not_found(msg),
            NackError::Storage(e) => Status::internal(e.to_string()),
        }
    }
}

impl IntoStatus for CreateQueueError {
    fn into_status(self) -> Status {
        match self {
            CreateQueueError::QueueAlreadyExists(msg) => Status::already_exists(msg),
            CreateQueueError::LuaCompilation(msg) => Status::invalid_argument(msg),
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

impl IntoStatus for ConfigError {
    fn into_status(self) -> Status {
        match self {
            ConfigError::InvalidValue(msg) => Status::invalid_argument(msg),
            ConfigError::Storage(e) => Status::internal(e.to_string()),
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
