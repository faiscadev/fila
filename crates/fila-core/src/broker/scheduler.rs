use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use crossbeam_channel::Receiver;
use tracing::{debug, info, warn};

use crate::broker::command::{ReadyMessage, SchedulerCommand};
use crate::broker::config::SchedulerConfig;
use crate::storage::{Storage, WriteBatchOp};

/// A registered consumer waiting for messages.
struct ConsumerEntry {
    queue_id: String,
    tx: tokio::sync::mpsc::Sender<ReadyMessage>,
}

/// Single-threaded scheduler core. Owns all mutable scheduler state and
/// processes commands from IO threads via a crossbeam channel.
pub struct Scheduler {
    storage: Arc<dyn Storage>,
    inbound: Receiver<SchedulerCommand>,
    idle_timeout: Duration,
    running: bool,
    consumers: HashMap<String, ConsumerEntry>,
    /// Round-robin index for delivering messages to consumers. Persists across
    /// calls to `try_deliver_pending` so messages are distributed fairly.
    consumer_rr_idx: usize,
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
            consumers: HashMap::new(),
            consumer_rr_idx: 0,
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
                let queue_id = message.queue_id.clone();
                let result = self.handle_enqueue(message);
                let _ = reply.send(result);
                // Try to deliver the newly enqueued message to a waiting consumer
                self.try_deliver_pending(&queue_id);
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
                tx,
            } => {
                info!(%queue_id, %consumer_id, "consumer registered");
                self.consumers.insert(
                    consumer_id,
                    ConsumerEntry {
                        queue_id: queue_id.clone(),
                        tx,
                    },
                );
                self.try_deliver_pending(&queue_id);
            }
            SchedulerCommand::UnregisterConsumer { consumer_id } => {
                info!(%consumer_id, "consumer unregistered");
                self.consumers.remove(&consumer_id);
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

    fn handle_enqueue(
        &self,
        message: crate::message::Message,
    ) -> Result<uuid::Uuid, crate::error::EnqueueError> {
        // Verify queue exists
        if self.storage.get_queue(&message.queue_id)?.is_none() {
            return Err(crate::error::EnqueueError::QueueNotFound(
                message.queue_id.clone(),
            ));
        }

        let msg_id = message.id;
        let key = crate::storage::keys::message_key(
            &message.queue_id,
            &message.fairness_key,
            message.enqueued_at,
            &msg_id,
        );

        self.storage.put_message(&key, &message)?;
        Ok(msg_id)
    }

    fn handle_create_queue(
        &self,
        name: String,
        mut config: crate::queue::QueueConfig,
    ) -> Result<String, crate::error::CreateQueueError> {
        // Check-then-put is safe: the scheduler is single-threaded, so no
        // concurrent command can create the same queue between the check and
        // the put. RocksDB put is an upsert, so the explicit check is the
        // only way to enforce uniqueness.
        // TODO(cluster): replace with atomic put-if-absent or distributed lock
        // when moving to a multi-node scheduler.
        if self.storage.get_queue(&name)?.is_some() {
            return Err(crate::error::CreateQueueError::QueueAlreadyExists(name));
        }
        config.name = name.clone();
        self.storage.put_queue(&name, &config)?;
        Ok(name)
    }

    fn handle_delete_queue(&self, queue_id: &str) -> Result<(), crate::error::DeleteQueueError> {
        // Check-then-delete is safe: same single-threaded guarantee as above.
        // RocksDB delete is a no-op on missing keys, so the explicit check is
        // the only way to return a meaningful NotFound error.
        // TODO(cluster): same as handle_create_queue — needs atomic operation.
        if self.storage.get_queue(queue_id)?.is_none() {
            return Err(crate::error::DeleteQueueError::QueueNotFound(
                queue_id.to_string(),
            ));
        }
        self.storage.delete_queue(queue_id)?;
        Ok(())
    }

    /// Scan the queue for pending (unleased) messages and deliver them to
    /// registered consumers. Creates lease + lease_expiry entries for each
    /// delivered message.
    fn try_deliver_pending(&mut self, queue_id: &str) {
        let queue_consumers: Vec<(&String, &ConsumerEntry)> = self
            .consumers
            .iter()
            .filter(|(_, e)| e.queue_id == queue_id)
            .collect();

        if queue_consumers.is_empty() {
            return;
        }

        let visibility_timeout_ms = self
            .storage
            .get_queue(queue_id)
            .ok()
            .flatten()
            .map(|c| c.visibility_timeout_ms)
            .unwrap_or(30_000);

        let prefix = crate::storage::keys::message_prefix(queue_id);
        let messages = match self.storage.list_messages(&prefix) {
            Ok(msgs) => msgs,
            Err(e) => {
                warn!(%queue_id, error = %e, "failed to list messages for delivery");
                return;
            }
        };

        for (_key, msg) in messages {
            // Skip messages that already have a lease
            let lease_key = crate::storage::keys::lease_key(queue_id, &msg.id);
            if self.storage.get_lease(&lease_key).ok().flatten().is_some() {
                continue;
            }

            // Try consumers in round-robin until one accepts (index persists across calls)
            let mut attempts = 0;
            while attempts < queue_consumers.len() {
                let (cid, entry) = queue_consumers[self.consumer_rr_idx % queue_consumers.len()];
                self.consumer_rr_idx = self.consumer_rr_idx.wrapping_add(1);
                attempts += 1;

                let ready = ReadyMessage {
                    msg_id: msg.id,
                    queue_id: msg.queue_id.clone(),
                    headers: msg.headers.clone(),
                    payload: msg.payload.clone(),
                    fairness_key: msg.fairness_key.clone(),
                    weight: msg.weight,
                    throttle_keys: msg.throttle_keys.clone(),
                    attempt_count: msg.attempt_count,
                };

                if entry.tx.try_send(ready).is_ok() {
                    // Create lease and expiry entries atomically
                    let now_ns = std::time::SystemTime::now()
                        .duration_since(std::time::UNIX_EPOCH)
                        .unwrap_or_default()
                        .as_nanos() as u64;
                    let expiry_ns = now_ns + visibility_timeout_ms * 1_000_000;

                    let lease_val = crate::storage::keys::lease_value(cid, expiry_ns);
                    let expiry_key =
                        crate::storage::keys::lease_expiry_key(expiry_ns, queue_id, &msg.id);

                    if let Err(e) = self.storage.write_batch(vec![
                        WriteBatchOp::PutLease {
                            key: lease_key,
                            value: lease_val,
                        },
                        WriteBatchOp::PutLeaseExpiry { key: expiry_key },
                    ]) {
                        warn!(msg_id = %msg.id, error = %e, "failed to write lease");
                    }
                    break;
                }
            }
        }
    }

    /// Access the storage layer (used by tests).
    #[cfg(test)]
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
            command_channel_capacity: 256,
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

    /// Helper: send a CreateQueue command so enqueue tests have an existing queue.
    fn send_create_queue(tx: &crossbeam_channel::Sender<SchedulerCommand>, name: &str) {
        let (reply_tx, _reply_rx) = tokio::sync::oneshot::channel();
        tx.send(SchedulerCommand::CreateQueue {
            name: name.to_string(),
            config: crate::queue::QueueConfig::new(name.to_string()),
            reply: reply_tx,
        })
        .unwrap();
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

        // Create the queue first so enqueue succeeds
        send_create_queue(&tx, "q1");

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

        send_create_queue(&tx, "test-queue");

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

    #[test]
    fn enqueue_to_nonexistent_queue_returns_error() {
        let (tx, mut scheduler, _dir) = test_setup();

        let msg = test_message("no-such-queue");
        let (reply_tx, mut reply_rx) = tokio::sync::oneshot::channel();

        tx.send(SchedulerCommand::Enqueue {
            message: msg,
            reply: reply_tx,
        })
        .unwrap();
        tx.send(SchedulerCommand::Shutdown).unwrap();

        scheduler.run();

        let err = reply_rx.try_recv().unwrap().unwrap_err();
        assert!(
            matches!(err, crate::error::EnqueueError::QueueNotFound(_)),
            "expected QueueNotFound, got {err:?}"
        );
    }

    #[test]
    fn enqueue_persists_message_to_storage() {
        let (tx, mut scheduler, _dir) = test_setup();

        send_create_queue(&tx, "persist-queue");

        let msg = test_message("persist-queue");
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

        // Verify the message was persisted by reading it back from storage
        let key =
            crate::storage::keys::message_key("persist-queue", "default", 1_000_000_000, &msg_id);
        let stored = scheduler.storage().get_message(&key).unwrap();
        assert!(stored.is_some(), "message should be persisted in storage");

        let stored_msg = stored.unwrap();
        assert_eq!(stored_msg.id, msg_id);
        assert_eq!(stored_msg.queue_id, "persist-queue");
        assert_eq!(stored_msg.payload, vec![1, 2, 3]);
    }

    #[test]
    fn enqueue_100_messages_unique_time_ordered_ids() {
        let (tx, mut scheduler, _dir) = test_setup();

        send_create_queue(&tx, "bulk-queue");

        let mut receivers = Vec::with_capacity(100);
        for i in 0u64..100 {
            let mut msg = test_message("bulk-queue");
            msg.enqueued_at = i;
            let (reply_tx, reply_rx) = tokio::sync::oneshot::channel();
            tx.send(SchedulerCommand::Enqueue {
                message: msg,
                reply: reply_tx,
            })
            .unwrap();
            receivers.push(reply_rx);
        }

        tx.send(SchedulerCommand::Shutdown).unwrap();
        scheduler.run();

        // Collect all returned IDs
        let ids: Vec<Uuid> = receivers
            .into_iter()
            .map(|mut rx| rx.try_recv().unwrap().unwrap())
            .collect();

        // All IDs must be unique
        let unique_count = {
            let mut set = std::collections::HashSet::new();
            ids.iter().for_each(|id| {
                set.insert(*id);
            });
            set.len()
        };
        assert_eq!(unique_count, 100, "all 100 message IDs must be unique");

        // UUIDv7 IDs are time-ordered, so sorted order should match insertion order
        let mut sorted_ids = ids.clone();
        sorted_ids.sort();
        assert_eq!(ids, sorted_ids, "UUIDv7 IDs should be time-ordered");

        // Verify all 100 messages are persisted
        let prefix = crate::storage::keys::message_prefix("bulk-queue");
        let stored = scheduler.storage().list_messages(&prefix).unwrap();
        assert_eq!(stored.len(), 100, "all 100 messages should be persisted");
    }

    #[test]
    fn consumer_receives_enqueued_messages() {
        let (tx, mut scheduler, _dir) = test_setup();

        send_create_queue(&tx, "lease-queue");

        // Register a consumer
        let (consumer_tx, mut consumer_rx) = tokio::sync::mpsc::channel(64);
        tx.send(SchedulerCommand::RegisterConsumer {
            queue_id: "lease-queue".to_string(),
            consumer_id: "c1".to_string(),
            tx: consumer_tx,
        })
        .unwrap();

        // Enqueue a message — should be delivered to the consumer
        let msg = test_message("lease-queue");
        let msg_id = msg.id;
        let (reply_tx, _reply_rx) = tokio::sync::oneshot::channel();
        tx.send(SchedulerCommand::Enqueue {
            message: msg,
            reply: reply_tx,
        })
        .unwrap();

        tx.send(SchedulerCommand::Shutdown).unwrap();
        scheduler.run();

        // Consumer should have received the message
        let ready = consumer_rx.try_recv().unwrap();
        assert_eq!(ready.msg_id, msg_id);
        assert_eq!(ready.queue_id, "lease-queue");
        assert_eq!(ready.payload, vec![1, 2, 3]);
    }

    #[test]
    fn consumer_receives_pending_messages_on_register() {
        let (tx, mut scheduler, _dir) = test_setup();

        send_create_queue(&tx, "pending-queue");

        // Enqueue messages first (no consumer yet)
        let mut msg_ids = Vec::new();
        for i in 0u64..5 {
            let mut msg = test_message("pending-queue");
            msg.enqueued_at = i;
            msg_ids.push(msg.id);
            let (reply_tx, _reply_rx) = tokio::sync::oneshot::channel();
            tx.send(SchedulerCommand::Enqueue {
                message: msg,
                reply: reply_tx,
            })
            .unwrap();
        }

        // Now register a consumer — should receive all pending messages
        let (consumer_tx, mut consumer_rx) = tokio::sync::mpsc::channel(64);
        tx.send(SchedulerCommand::RegisterConsumer {
            queue_id: "pending-queue".to_string(),
            consumer_id: "c1".to_string(),
            tx: consumer_tx,
        })
        .unwrap();

        tx.send(SchedulerCommand::Shutdown).unwrap();
        scheduler.run();

        // All 5 messages should be delivered
        let mut received_ids = Vec::new();
        while let Ok(ready) = consumer_rx.try_recv() {
            received_ids.push(ready.msg_id);
        }
        assert_eq!(
            received_ids.len(),
            5,
            "all pending messages should be delivered"
        );
        assert_eq!(received_ids, msg_ids, "messages delivered in FIFO order");
    }

    #[test]
    fn lease_creates_entries_in_storage() {
        let (tx, mut scheduler, _dir) = test_setup();

        send_create_queue(&tx, "lease-cf-queue");

        // Register consumer first
        let (consumer_tx, _consumer_rx) = tokio::sync::mpsc::channel(64);
        tx.send(SchedulerCommand::RegisterConsumer {
            queue_id: "lease-cf-queue".to_string(),
            consumer_id: "c1".to_string(),
            tx: consumer_tx,
        })
        .unwrap();

        // Enqueue a message
        let msg = test_message("lease-cf-queue");
        let msg_id = msg.id;
        let (reply_tx, _reply_rx) = tokio::sync::oneshot::channel();
        tx.send(SchedulerCommand::Enqueue {
            message: msg,
            reply: reply_tx,
        })
        .unwrap();

        tx.send(SchedulerCommand::Shutdown).unwrap();
        scheduler.run();

        // Verify a lease was created
        let lease_key = crate::storage::keys::lease_key("lease-cf-queue", &msg_id);
        let lease = scheduler.storage().get_lease(&lease_key).unwrap();
        assert!(lease.is_some(), "lease entry should exist after delivery");
    }

    #[test]
    fn multiple_consumers_get_different_messages() {
        let (tx, mut scheduler, _dir) = test_setup();

        send_create_queue(&tx, "multi-queue");

        // Register two consumers
        let (c1_tx, mut c1_rx) = tokio::sync::mpsc::channel(64);
        tx.send(SchedulerCommand::RegisterConsumer {
            queue_id: "multi-queue".to_string(),
            consumer_id: "c1".to_string(),
            tx: c1_tx,
        })
        .unwrap();

        let (c2_tx, mut c2_rx) = tokio::sync::mpsc::channel(64);
        tx.send(SchedulerCommand::RegisterConsumer {
            queue_id: "multi-queue".to_string(),
            consumer_id: "c2".to_string(),
            tx: c2_tx,
        })
        .unwrap();

        // Enqueue 4 messages
        let mut msg_ids = Vec::new();
        for i in 0u64..4 {
            let mut msg = test_message("multi-queue");
            msg.enqueued_at = i;
            msg_ids.push(msg.id);
            let (reply_tx, _reply_rx) = tokio::sync::oneshot::channel();
            tx.send(SchedulerCommand::Enqueue {
                message: msg,
                reply: reply_tx,
            })
            .unwrap();
        }

        tx.send(SchedulerCommand::Shutdown).unwrap();
        scheduler.run();

        // Collect messages from both consumers
        let mut c1_msgs = Vec::new();
        while let Ok(ready) = c1_rx.try_recv() {
            c1_msgs.push(ready.msg_id);
        }
        let mut c2_msgs = Vec::new();
        while let Ok(ready) = c2_rx.try_recv() {
            c2_msgs.push(ready.msg_id);
        }

        // Both consumers should have received messages
        let total = c1_msgs.len() + c2_msgs.len();
        assert_eq!(
            total, 4,
            "all 4 messages should be delivered across consumers"
        );

        // No message should be delivered to both consumers
        let mut all_ids: Vec<_> = c1_msgs.iter().chain(c2_msgs.iter()).copied().collect();
        all_ids.sort();
        all_ids.dedup();
        assert_eq!(
            all_ids.len(),
            4,
            "each message delivered to exactly one consumer"
        );
    }

    #[test]
    fn unregister_consumer_stops_delivery() {
        let (tx, mut scheduler, _dir) = test_setup();

        send_create_queue(&tx, "unreg-queue");

        // Register then immediately unregister
        let (consumer_tx, mut consumer_rx) = tokio::sync::mpsc::channel(64);
        tx.send(SchedulerCommand::RegisterConsumer {
            queue_id: "unreg-queue".to_string(),
            consumer_id: "c1".to_string(),
            tx: consumer_tx,
        })
        .unwrap();
        tx.send(SchedulerCommand::UnregisterConsumer {
            consumer_id: "c1".to_string(),
        })
        .unwrap();

        // Enqueue a message after unregistration
        let msg = test_message("unreg-queue");
        let (reply_tx, _reply_rx) = tokio::sync::oneshot::channel();
        tx.send(SchedulerCommand::Enqueue {
            message: msg,
            reply: reply_tx,
        })
        .unwrap();

        tx.send(SchedulerCommand::Shutdown).unwrap();
        scheduler.run();

        // Consumer should NOT have received any messages
        assert!(
            consumer_rx.try_recv().is_err(),
            "unregistered consumer should not receive messages"
        );
    }

    #[test]
    fn enqueue_10_messages_lease_receives_all() {
        let (tx, mut scheduler, _dir) = test_setup();

        send_create_queue(&tx, "ten-queue");

        // Register consumer
        let (consumer_tx, mut consumer_rx) = tokio::sync::mpsc::channel(64);
        tx.send(SchedulerCommand::RegisterConsumer {
            queue_id: "ten-queue".to_string(),
            consumer_id: "c1".to_string(),
            tx: consumer_tx,
        })
        .unwrap();

        // Enqueue 10 messages
        let mut msg_ids = Vec::new();
        for i in 0u64..10 {
            let mut msg = test_message("ten-queue");
            msg.enqueued_at = i;
            msg_ids.push(msg.id);
            let (reply_tx, _reply_rx) = tokio::sync::oneshot::channel();
            tx.send(SchedulerCommand::Enqueue {
                message: msg,
                reply: reply_tx,
            })
            .unwrap();
        }

        tx.send(SchedulerCommand::Shutdown).unwrap();
        scheduler.run();

        // All 10 messages should be delivered
        let mut received = Vec::new();
        while let Ok(ready) = consumer_rx.try_recv() {
            received.push(ready.msg_id);
        }
        assert_eq!(received.len(), 10, "all 10 messages should be received");
        assert_eq!(received, msg_ids, "messages received in FIFO order");
    }
}
