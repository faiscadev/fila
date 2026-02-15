pub mod command;
pub mod config;
pub mod drr;
pub mod metrics;
mod scheduler;
pub mod stats;
pub mod throttle;

use std::sync::Arc;
use std::thread;

use tracing::info;

use crate::error::{BrokerError, BrokerResult};
use crate::storage::Storage;

pub use command::{QueueSummary, ReadyMessage, SchedulerCommand};
pub use config::BrokerConfig;

use scheduler::Scheduler;

/// The broker owns the scheduler thread and the inbound command channel.
/// IO threads (gRPC handlers) send commands through `send_command()`,
/// and the single-threaded scheduler processes them sequentially.
pub struct Broker {
    command_tx: crossbeam_channel::Sender<SchedulerCommand>,
    scheduler_thread: Option<thread::JoinHandle<()>>,
}

impl Broker {
    /// Create a new broker, spawning the scheduler on a dedicated OS thread.
    #[tracing::instrument(skip_all, fields(listen_addr = %config.server.listen_addr))]
    pub fn new(config: BrokerConfig, storage: Arc<dyn Storage>) -> BrokerResult<Self> {
        let (tx, rx) = crossbeam_channel::bounded::<SchedulerCommand>(
            config.scheduler.command_channel_capacity,
        );

        let scheduler_config = config.scheduler.clone();
        let lua_config = config.lua.clone();

        let handle = thread::Builder::new()
            .name("fila-scheduler".to_string())
            .spawn(move || {
                let mut scheduler = Scheduler::new(storage, rx, &scheduler_config, &lua_config);
                scheduler.run();
            })
            .map_err(|e| BrokerError::SchedulerSpawn(e.to_string()))?;

        info!("broker started");

        Ok(Self {
            command_tx: tx,
            scheduler_thread: Some(handle),
        })
    }

    /// Send a command to the scheduler. Returns an error if the channel is full
    /// or disconnected.
    #[tracing::instrument(skip_all)]
    pub fn send_command(&self, cmd: SchedulerCommand) -> BrokerResult<()> {
        self.command_tx.try_send(cmd).map_err(|e| match e {
            crossbeam_channel::TrySendError::Full(_) => BrokerError::ChannelFull,
            crossbeam_channel::TrySendError::Disconnected(_) => BrokerError::ChannelDisconnected,
        })
    }

    /// Initiate graceful shutdown: send the shutdown command and wait for the
    /// scheduler thread to finish.
    #[tracing::instrument(skip_all)]
    pub fn shutdown(mut self) -> BrokerResult<()> {
        info!("initiating broker shutdown");

        // Send shutdown command (ignore error if channel already closed)
        let _ = self.command_tx.send(SchedulerCommand::Shutdown);

        // Wait for the scheduler thread to finish
        if let Some(handle) = self.scheduler_thread.take() {
            handle.join().map_err(|_| BrokerError::SchedulerPanicked)?;
        }

        info!("broker shutdown complete");
        Ok(())
    }
}

impl Drop for Broker {
    fn drop(&mut self) {
        // If shutdown wasn't called explicitly, attempt to stop the scheduler
        if self.scheduler_thread.is_some() {
            let _ = self.command_tx.send(SchedulerCommand::Shutdown);
            if let Some(handle) = self.scheduler_thread.take() {
                let _ = handle.join();
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::message::Message;
    use crate::storage::RocksDbStorage;
    use std::collections::HashMap;
    use uuid::Uuid;

    fn test_broker() -> (Broker, tempfile::TempDir) {
        let dir = tempfile::tempdir().unwrap();
        let storage = Arc::new(RocksDbStorage::open(dir.path()).unwrap());
        let config = BrokerConfig {
            scheduler: config::SchedulerConfig {
                command_channel_capacity: 100,
                idle_timeout_ms: 10,
                quantum: 1000,
            },
            ..Default::default()
        };
        let broker = Broker::new(config, storage).unwrap();
        (broker, dir)
    }

    #[test]
    fn broker_starts_and_shuts_down() {
        let (broker, _dir) = test_broker();
        broker.shutdown().unwrap();
    }

    #[test]
    fn broker_processes_enqueue_command() {
        let (broker, _dir) = test_broker();

        // Create the queue first
        let (create_tx, create_rx) = tokio::sync::oneshot::channel();
        broker
            .send_command(SchedulerCommand::CreateQueue {
                name: "test-queue".to_string(),
                config: crate::queue::QueueConfig::new("test-queue".to_string()),
                reply: create_tx,
            })
            .unwrap();
        create_rx.blocking_recv().unwrap().unwrap();

        let msg = Message {
            id: Uuid::now_v7(),
            queue_id: "test-queue".to_string(),
            headers: HashMap::new(),
            payload: vec![42],
            fairness_key: "default".to_string(),
            weight: 1,
            throttle_keys: vec![],
            attempt_count: 0,
            enqueued_at: 1_000_000_000,
            leased_at: None,
        };
        let msg_id = msg.id;

        let (reply_tx, reply_rx) = tokio::sync::oneshot::channel();
        broker
            .send_command(SchedulerCommand::Enqueue {
                message: msg,
                reply: reply_tx,
            })
            .unwrap();

        // Give the scheduler thread time to process
        let result = reply_rx.blocking_recv().unwrap().unwrap();
        assert_eq!(result, msg_id);

        broker.shutdown().unwrap();
    }

    #[test]
    fn broker_drop_stops_scheduler() {
        let (broker, _dir) = test_broker();
        drop(broker);
        // If we get here without hanging, the Drop impl worked
    }
}
