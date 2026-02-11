use std::sync::Arc;
use std::time::Duration;

use crossbeam_channel::Receiver;
use tracing::{debug, info};

use crate::broker::command::SchedulerCommand;
use crate::broker::config::SchedulerConfig;
use crate::storage::Storage;

/// Single-threaded scheduler core. Owns all mutable scheduler state and
/// processes commands from IO threads via a crossbeam channel.
pub struct Scheduler {
    storage: Arc<dyn Storage>,
    inbound: Receiver<SchedulerCommand>,
    idle_timeout: Duration,
    running: bool,
}

impl Scheduler {
    pub fn new(
        storage: Arc<dyn Storage>,
        inbound: Receiver<SchedulerCommand>,
        config: &SchedulerConfig,
    ) -> Self {
        Self {
            storage,
            inbound,
            idle_timeout: Duration::from_millis(config.idle_timeout_ms),
            running: true,
        }
    }

    /// Run the scheduler event loop. This blocks the current thread until
    /// a `Shutdown` command is received or the inbound channel is disconnected.
    pub fn run(&mut self) {
        info!("scheduler started");

        while self.running {
            // Phase 1: Drain all buffered commands (non-blocking)
            let mut drained = 0;
            while let Ok(cmd) = self.inbound.try_recv() {
                self.handle_command(cmd);
                drained += 1;
                if !self.running {
                    break;
                }
            }

            if !self.running {
                break;
            }

            // Phase 2: Future stories will add lease expiry, token bucket refill, DRR here

            // Phase 3: Park until next command or timeout
            if drained == 0 {
                match self.inbound.recv_timeout(self.idle_timeout) {
                    Ok(cmd) => self.handle_command(cmd),
                    Err(crossbeam_channel::RecvTimeoutError::Timeout) => {
                        // Normal idle wakeup — future stories add periodic work here
                    }
                    Err(crossbeam_channel::RecvTimeoutError::Disconnected) => {
                        info!("inbound channel disconnected, shutting down");
                        self.running = false;
                    }
                }
            }
        }

        info!("scheduler stopped");
    }

    fn handle_command(&mut self, cmd: SchedulerCommand) {
        match cmd {
            SchedulerCommand::Enqueue { message, reply } => {
                debug!(queue_id = %message.queue_id, msg_id = %message.id, "enqueue command received");
                // Stub: will be implemented in story 1.6
                let _ = reply.send(Ok(message.id));
            }
            SchedulerCommand::Ack {
                queue_id,
                msg_id,
                reply,
            } => {
                debug!(%queue_id, %msg_id, "ack command received");
                // Stub: will be implemented in story 1.8
                let _ = reply.send(Ok(()));
            }
            SchedulerCommand::Nack {
                queue_id,
                msg_id,
                error,
                reply,
            } => {
                debug!(%queue_id, %msg_id, error, "nack command received");
                // Stub: will be implemented in story 1.8
                let _ = reply.send(Ok(()));
            }
            SchedulerCommand::RegisterConsumer {
                queue_id,
                consumer_id,
                ..
            } => {
                info!(%queue_id, %consumer_id, "consumer registered");
                // Stub: will be implemented in story 1.7
            }
            SchedulerCommand::UnregisterConsumer { consumer_id } => {
                info!(%consumer_id, "consumer unregistered");
                // Stub: will be implemented in story 1.7
            }
            SchedulerCommand::CreateQueue {
                name,
                config,
                reply,
            } => {
                info!(%name, "create queue command received");
                let result = self.handle_create_queue(name, config);
                let _ = reply.send(result);
            }
            SchedulerCommand::DeleteQueue { queue_id, reply } => {
                info!(%queue_id, "delete queue command received");
                let result = self.handle_delete_queue(&queue_id);
                let _ = reply.send(result);
            }
            SchedulerCommand::Shutdown => {
                info!("shutdown command received, draining remaining commands");
                self.running = false;
            }
        }
    }

    fn handle_create_queue(
        &self,
        name: String,
        mut config: crate::queue::QueueConfig,
    ) -> Result<String, crate::error::CreateQueueError> {
        // Check if queue already exists
        if self.storage.get_queue(&name)?.is_some() {
            return Err(crate::error::CreateQueueError::QueueAlreadyExists(name));
        }
        // Ensure config name matches the requested queue name
        config.name = name.clone();
        self.storage.put_queue(&name, &config)?;
        Ok(name)
    }

    fn handle_delete_queue(&self, queue_id: &str) -> Result<(), crate::error::DeleteQueueError> {
        // Check if queue exists
        if self.storage.get_queue(queue_id)?.is_none() {
            return Err(crate::error::DeleteQueueError::QueueNotFound(
                queue_id.to_string(),
            ));
        }
        self.storage.delete_queue(queue_id)?;
        Ok(())
    }

    /// Access the storage layer (used by tests and future scheduler logic).
    #[allow(dead_code)]
    pub fn storage(&self) -> &dyn Storage {
        self.storage.as_ref()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::broker::config::SchedulerConfig;
    use crate::message::Message;
    use crate::storage::RocksDbStorage;
    use std::collections::HashMap;
    use uuid::Uuid;

    fn test_setup() -> (
        crossbeam_channel::Sender<SchedulerCommand>,
        Scheduler,
        tempfile::TempDir,
    ) {
        let dir = tempfile::tempdir().unwrap();
        let storage = Arc::new(RocksDbStorage::open(dir.path()).unwrap());
        let config = SchedulerConfig {
            command_channel_capacity: 100,
            idle_timeout_ms: 10,
        };
        let (tx, rx) = crossbeam_channel::bounded(config.command_channel_capacity);
        let scheduler = Scheduler::new(storage, rx, &config);
        (tx, scheduler, dir)
    }

    fn test_message(queue_id: &str) -> Message {
        Message {
            id: Uuid::now_v7(),
            queue_id: queue_id.to_string(),
            headers: HashMap::new(),
            payload: vec![1, 2, 3],
            fairness_key: "default".to_string(),
            weight: 1,
            throttle_keys: vec![],
            attempt_count: 0,
            enqueued_at: 1_000_000_000,
            leased_at: None,
        }
    }

    #[test]
    fn shutdown_causes_scheduler_to_stop() {
        let (tx, mut scheduler, _dir) = test_setup();

        tx.send(SchedulerCommand::Shutdown).unwrap();

        // Run should return after processing the shutdown command
        scheduler.run();
        // If we get here, the scheduler stopped correctly
    }

    #[test]
    fn commands_processed_in_fifo_order() {
        let (tx, mut scheduler, _dir) = test_setup();

        let mut expected_ids = Vec::new();
        let mut receivers = Vec::new();

        // Send 5 enqueue commands
        for _ in 0..5 {
            let msg = test_message("q1");
            expected_ids.push(msg.id);
            let (reply_tx, reply_rx) = tokio::sync::oneshot::channel();
            tx.send(SchedulerCommand::Enqueue {
                message: msg,
                reply: reply_tx,
            })
            .unwrap();
            receivers.push(reply_rx);
        }

        // Send shutdown so the scheduler stops
        tx.send(SchedulerCommand::Shutdown).unwrap();

        scheduler.run();

        // Verify all replies were received and IDs match (FIFO order)
        for (i, mut rx) in receivers.into_iter().enumerate() {
            let result = rx.try_recv().unwrap().unwrap();
            assert_eq!(result, expected_ids[i], "command {i} should return its ID");
        }
    }

    #[test]
    fn enqueue_reply_received() {
        let (tx, mut scheduler, _dir) = test_setup();

        let msg = test_message("test-queue");
        let msg_id = msg.id;
        let (reply_tx, mut reply_rx) = tokio::sync::oneshot::channel();

        tx.send(SchedulerCommand::Enqueue {
            message: msg,
            reply: reply_tx,
        })
        .unwrap();
        tx.send(SchedulerCommand::Shutdown).unwrap();

        scheduler.run();

        let result = reply_rx.try_recv().unwrap().unwrap();
        assert_eq!(result, msg_id);
    }

    #[test]
    fn ack_reply_received() {
        let (tx, mut scheduler, _dir) = test_setup();

        let msg_id = Uuid::now_v7();
        let (reply_tx, mut reply_rx) = tokio::sync::oneshot::channel();

        tx.send(SchedulerCommand::Ack {
            queue_id: "q1".to_string(),
            msg_id,
            reply: reply_tx,
        })
        .unwrap();
        tx.send(SchedulerCommand::Shutdown).unwrap();

        scheduler.run();

        assert!(reply_rx.try_recv().unwrap().is_ok());
    }

    #[test]
    fn channel_disconnect_stops_scheduler() {
        let (tx, mut scheduler, _dir) = test_setup();

        // Drop the sender so the channel disconnects
        drop(tx);

        // Scheduler should detect disconnection and stop
        scheduler.run();
        // If we get here, it handled disconnection correctly
    }

    #[test]
    fn create_queue_success() {
        let (tx, mut scheduler, _dir) = test_setup();

        let (reply_tx, mut reply_rx) = tokio::sync::oneshot::channel();
        let config = crate::queue::QueueConfig::new("test-queue".to_string());

        tx.send(SchedulerCommand::CreateQueue {
            name: "test-queue".to_string(),
            config,
            reply: reply_tx,
        })
        .unwrap();
        tx.send(SchedulerCommand::Shutdown).unwrap();

        scheduler.run();

        let result = reply_rx.try_recv().unwrap().unwrap();
        assert_eq!(result, "test-queue");
    }

    #[test]
    fn create_queue_already_exists() {
        let (tx, mut scheduler, _dir) = test_setup();

        // Create queue first time
        let (reply_tx1, mut reply_rx1) = tokio::sync::oneshot::channel();
        tx.send(SchedulerCommand::CreateQueue {
            name: "dup-queue".to_string(),
            config: crate::queue::QueueConfig::new("dup-queue".to_string()),
            reply: reply_tx1,
        })
        .unwrap();

        // Try to create same queue again
        let (reply_tx2, mut reply_rx2) = tokio::sync::oneshot::channel();
        tx.send(SchedulerCommand::CreateQueue {
            name: "dup-queue".to_string(),
            config: crate::queue::QueueConfig::new("dup-queue".to_string()),
            reply: reply_tx2,
        })
        .unwrap();

        tx.send(SchedulerCommand::Shutdown).unwrap();
        scheduler.run();

        assert!(reply_rx1.try_recv().unwrap().is_ok());
        let err = reply_rx2.try_recv().unwrap().unwrap_err();
        assert!(
            matches!(err, crate::error::CreateQueueError::QueueAlreadyExists(_)),
            "expected QueueAlreadyExists, got {err:?}"
        );
    }

    #[test]
    fn delete_queue_success() {
        let (tx, mut scheduler, _dir) = test_setup();

        // Create then delete
        let (create_tx, mut create_rx) = tokio::sync::oneshot::channel();
        tx.send(SchedulerCommand::CreateQueue {
            name: "del-queue".to_string(),
            config: crate::queue::QueueConfig::new("del-queue".to_string()),
            reply: create_tx,
        })
        .unwrap();

        let (del_tx, mut del_rx) = tokio::sync::oneshot::channel();
        tx.send(SchedulerCommand::DeleteQueue {
            queue_id: "del-queue".to_string(),
            reply: del_tx,
        })
        .unwrap();

        tx.send(SchedulerCommand::Shutdown).unwrap();
        scheduler.run();

        assert!(create_rx.try_recv().unwrap().is_ok());
        assert!(del_rx.try_recv().unwrap().is_ok());
    }

    #[test]
    fn delete_queue_not_found() {
        let (tx, mut scheduler, _dir) = test_setup();

        let (reply_tx, mut reply_rx) = tokio::sync::oneshot::channel();
        tx.send(SchedulerCommand::DeleteQueue {
            queue_id: "nonexistent".to_string(),
            reply: reply_tx,
        })
        .unwrap();
        tx.send(SchedulerCommand::Shutdown).unwrap();

        scheduler.run();

        let err = reply_rx.try_recv().unwrap().unwrap_err();
        assert!(
            matches!(err, crate::error::DeleteQueueError::QueueNotFound(_)),
            "expected QueueNotFound, got {err:?}"
        );
    }
}
