use std::collections::{HashMap, HashSet, VecDeque};
use std::sync::Arc;
use std::time::{Duration, Instant};

use crossbeam_channel::Receiver;
use tracing::{debug, error, info, warn};

use crate::broker::command::{ReadyMessage, SchedulerCommand};
use crate::broker::config::{LuaConfig, SchedulerConfig};
use crate::broker::drr::DrrScheduler;
use crate::broker::metrics::Metrics;
use crate::broker::router::QueueRouter;
use crate::broker::throttle::ThrottleManager;
use crate::lua::LuaEngine;
use crate::storage::{Mutation, StorageEngine};

mod admin_handlers;
mod delivery;
mod handlers;
mod metrics_recording;
mod recovery;

/// A registered consumer waiting for messages.
pub(super) struct ConsumerEntry {
    pub(super) queue_id: String,
    pub(super) tx: tokio::sync::mpsc::Sender<ReadyMessage>,
}

/// An entry in the per-fairness-key pending message index.
/// Tracks messages available for delivery (not currently leased).
pub(super) struct PendingEntry {
    pub(super) msg_key: Vec<u8>,
    pub(super) msg_id: uuid::Uuid,
    pub(super) throttle_keys: Vec<String>,
}

/// Single-threaded scheduler core. Owns all mutable scheduler state and
/// processes commands from IO threads via a crossbeam channel.
pub struct Scheduler {
    storage: Arc<dyn StorageEngine>,
    inbound: Receiver<SchedulerCommand>,
    idle_timeout: Duration,
    running: bool,
    consumers: HashMap<String, ConsumerEntry>,
    /// Per-queue round-robin index for delivering messages to consumers.
    /// Persists across calls so messages are distributed fairly across
    /// consumers within each queue independently.
    consumer_rr_idx: HashMap<String, usize>,
    /// Deficit Round Robin scheduler for fair message delivery across
    /// fairness keys.
    drr: DrrScheduler,
    /// Token bucket rate limiter for throttle keys.
    throttle: ThrottleManager,
    /// Per-(queue_id, fairness_key) FIFO queue of pending (unleased) messages.
    pending: HashMap<(String, String), VecDeque<PendingEntry>>,
    /// Reverse index: msg_id → (queue_id, fairness_key) for O(1) lookup on ack/nack.
    pending_by_id: HashMap<uuid::Uuid, (String, String)>,
    /// Storage key for in-flight (leased) messages, for O(1) lookup on ack/nack.
    leased_msg_keys: HashMap<uuid::Uuid, Vec<u8>>,
    /// Lua rules engine for executing on_enqueue and on_failure scripts.
    lua_engine: Option<LuaEngine>,
    /// All known queue IDs, for zeroing gauges when queues become idle.
    known_queues: HashSet<String>,
    /// Routes (queue_id, fairness_key) → GroupId for Raft group assignment.
    /// Phase 1: trivial 1:1 routing (every key maps to its queue).
    router: QueueRouter,
    /// OTel metrics for recording counters and gauges.
    metrics: Metrics,
    /// Per-(queue_id, fairness_key) delivery counts for fair share ratio calculation.
    /// Reset each time record_gauges() is called.
    fairness_deliveries: HashMap<(String, String), u64>,
    /// Maximum number of commands to drain per coalescing batch.
    write_coalesce_max_batch: usize,
}

impl Scheduler {
    pub fn new(
        storage: Arc<dyn StorageEngine>,
        inbound: Receiver<SchedulerCommand>,
        config: &SchedulerConfig,
        lua_config: &LuaConfig,
        cluster_node_id: u64,
    ) -> Self {
        let lua_engine = match LuaEngine::new(storage.clone(), lua_config) {
            Ok(engine) => Some(engine),
            Err(e) => {
                error!(error = %e, "failed to create Lua engine, scripts will be disabled");
                None
            }
        };

        Self {
            storage,
            inbound,
            idle_timeout: Duration::from_millis(config.idle_timeout_ms),
            running: true,
            consumers: HashMap::new(),
            consumer_rr_idx: HashMap::new(),
            drr: DrrScheduler::new(config.quantum),
            throttle: ThrottleManager::default(),
            pending: HashMap::new(),
            pending_by_id: HashMap::new(),
            leased_msg_keys: HashMap::new(),
            lua_engine,
            known_queues: HashSet::new(),
            router: QueueRouter::new(),
            metrics: if cluster_node_id > 0 {
                Metrics::with_node_id(cluster_node_id)
            } else {
                Metrics::new()
            },
            fairness_deliveries: HashMap::new(),
            write_coalesce_max_batch: config.write_coalesce_max_batch,
        }
    }

    /// Run the scheduler event loop. This blocks the current thread until
    /// a `Shutdown` command is received or the inbound channel is disconnected.
    pub fn run(&mut self) {
        info!("scheduler started");
        self.recover();

        while self.running {
            // Phase 1: Drain buffered commands (non-blocking), coalescing enqueues.
            let drained = self.drain_and_coalesce();

            // Phase 2: Refill token buckets, then DRR delivery round.
            self.throttle.refill_all(Instant::now());
            self.drr_deliver();

            // Record gauge metrics after state changes.
            self.record_gauges();

            if !self.running {
                break;
            }

            // Phase 3: Park until next command or timeout
            if drained == 0 {
                match self.inbound.recv_timeout(self.idle_timeout) {
                    Ok(cmd) => self.handle_command(cmd),
                    Err(crossbeam_channel::RecvTimeoutError::Timeout) => {
                        // Periodic work: reclaim expired leases and deliver re-queued messages
                        if self.reclaim_expired_leases() > 0 {
                            self.throttle.refill_all(Instant::now());
                            self.drr_deliver();
                        }
                    }
                    Err(crossbeam_channel::RecvTimeoutError::Disconnected) => {
                        info!("inbound channel disconnected, shutting down");
                        self.running = false;
                    }
                }
            }
        }

        // Flush the WAL to ensure all writes are durable before exit
        if let Err(e) = self.storage.flush() {
            warn!(error = %e, "failed to flush WAL during shutdown");
        }

        info!("scheduler stopped");
    }

    /// Drain up to `write_coalesce_max_batch` commands from the channel,
    /// coalescing enqueue commands into a single storage write batch.
    /// Non-enqueue commands are processed inline immediately.
    /// Returns the total number of commands drained.
    fn drain_and_coalesce(&mut self) -> usize {
        let max_batch = self.write_coalesce_max_batch;
        let mut drained = 0;

        // Accumulate enqueue commands for coalesced write
        let mut pending_enqueues: Vec<(
            crate::message::Message,
            tokio::sync::oneshot::Sender<Result<uuid::Uuid, crate::error::EnqueueError>>,
        )> = Vec::new();

        // Drain commands from the channel (non-blocking)
        while drained < max_batch {
            match self.inbound.try_recv() {
                Ok(cmd) => {
                    drained += 1;
                    match cmd {
                        SchedulerCommand::Enqueue { message, reply } => {
                            debug!(
                                queue_id = %message.queue_id,
                                msg_id = %message.id,
                                "enqueue command received (coalescing)"
                            );
                            pending_enqueues.push((message, reply));
                        }
                        other => {
                            // Flush any accumulated enqueues before processing
                            // non-enqueue commands, to maintain ordering.
                            if !pending_enqueues.is_empty() {
                                self.flush_coalesced_enqueues(&mut pending_enqueues);
                            }
                            self.handle_command(other);
                            if !self.running {
                                break;
                            }
                        }
                    }
                }
                Err(_) => break,
            }
        }

        // Flush any remaining enqueues
        if !pending_enqueues.is_empty() {
            self.flush_coalesced_enqueues(&mut pending_enqueues);
        }

        drained
    }

    /// Prepare all pending enqueue commands, commit their mutations in a single
    /// WriteBatch, then finalize in-memory state and send responses.
    fn flush_coalesced_enqueues(
        &mut self,
        pending: &mut Vec<(
            crate::message::Message,
            tokio::sync::oneshot::Sender<Result<uuid::Uuid, crate::error::EnqueueError>>,
        )>,
    ) {
        use crate::storage::Mutation;

        let batch = pending.drain(..);

        // Phase 1: Prepare each enqueue (validation, Lua, serialization).
        // Commands that fail preparation get their error sent immediately.
        let mut prepared: Vec<(
            handlers::PreparedEnqueue,
            tokio::sync::oneshot::Sender<Result<uuid::Uuid, crate::error::EnqueueError>>,
        )> = Vec::new();

        for (message, reply) in batch {
            match self.prepare_enqueue(message) {
                Ok(prep) => {
                    prepared.push((prep, reply));
                }
                Err(err) => {
                    // Validation/Lua failure — send error immediately, no storage write needed
                    let _ = reply.send(Err(err));
                }
            }
        }

        if prepared.is_empty() {
            return;
        }

        // Phase 2: Collect all mutations into a single batch
        let mutations: Vec<Mutation> = prepared
            .iter()
            .map(|(prep, _)| Mutation::PutMessage {
                key: prep.msg_key.clone(),
                value: prep.msg_value.clone(),
            })
            .collect();

        let batch_size = prepared.len();

        // Phase 3: Commit the coalesced batch
        match self.storage.apply_mutations(mutations) {
            Ok(()) => {
                debug!(batch_size, "coalesced enqueue batch committed");

                // Collect queue_ids that need delivery before sending responses
                let mut delivery_queues: HashSet<String> = HashSet::new();

                // Phase 4: Finalize in-memory state and send success responses
                for (prep, reply) in prepared {
                    delivery_queues.insert(prep.queue_id.clone());
                    self.finalize_enqueue(&prep);
                    let _ = reply.send(Ok(prep.msg_id));
                }

                // Deliver immediately for responsiveness
                for queue_id in &delivery_queues {
                    self.drr_deliver_queue(queue_id);
                }
            }
            Err(storage_err) => {
                // Storage failure — all callers in the batch receive the error
                warn!(
                    batch_size,
                    error = %storage_err,
                    "coalesced enqueue batch failed"
                );
                for (_, reply) in prepared {
                    let _ = reply.send(Err(crate::error::EnqueueError::Storage(
                        crate::error::StorageError::Engine(storage_err.to_string()),
                    )));
                }
            }
        }
    }

    fn handle_command(&mut self, cmd: SchedulerCommand) {
        match cmd {
            SchedulerCommand::Enqueue { message, reply } => {
                debug!(queue_id = %message.queue_id, msg_id = %message.id, "enqueue command received");
                let queue_id = message.queue_id.clone();
                let result = self.handle_enqueue(message);
                let _ = reply.send(result);
                // Deliver immediately for responsiveness
                self.drr_deliver_queue(&queue_id);
            }
            SchedulerCommand::Ack {
                queue_id,
                msg_id,
                reply,
            } => {
                debug!(%queue_id, %msg_id, "ack command received");
                let result = self.handle_ack(&queue_id, &msg_id);
                let _ = reply.send(result);
            }
            SchedulerCommand::Nack {
                queue_id,
                msg_id,
                error,
                reply,
            } => {
                debug!(%queue_id, %msg_id, %error, "nack command received");
                let result = self.handle_nack(&queue_id, &msg_id, &error);
                let ok = result.is_ok();
                let _ = reply.send(result);
                if ok {
                    // Re-deliver: the nacked message is now back in the ready pool
                    self.drr_deliver_queue(&queue_id);
                }
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
                // Deliver pending messages to the newly registered consumer
                self.drr_deliver_queue(&queue_id);
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
            SchedulerCommand::SetThrottleRate {
                key,
                rate_per_second,
                burst,
            } => {
                debug!(%key, %rate_per_second, %burst, "set throttle rate");
                self.throttle.set_rate(&key, rate_per_second, burst);
            }
            SchedulerCommand::RemoveThrottleRate { key } => {
                debug!(%key, "remove throttle rate");
                self.throttle.remove_rate(&key);
            }
            SchedulerCommand::SetConfig { key, value, reply } => {
                let result = self.handle_set_config(&key, &value);
                let _ = reply.send(result);
            }
            SchedulerCommand::GetConfig { key, reply } => {
                let result = self.handle_get_config(&key);
                let _ = reply.send(result);
            }
            SchedulerCommand::ListConfig { prefix, reply } => {
                let result = self.handle_list_config(&prefix);
                let _ = reply.send(result);
            }
            SchedulerCommand::GetStats { queue_id, reply } => {
                let result = self.handle_get_stats(&queue_id);
                let _ = reply.send(result);
            }
            SchedulerCommand::Redrive {
                dlq_queue_id,
                count,
                reply,
            } => {
                let result = self.handle_redrive(&dlq_queue_id, count);
                let _ = reply.send(result);
            }
            SchedulerCommand::ListQueues { reply } => {
                let result = self.handle_list_queues();
                let _ = reply.send(result);
            }
            SchedulerCommand::RecoverQueue { queue_id, reply } => {
                info!(%queue_id, "recover queue command received (leader promotion)");
                self.recover_queue(&queue_id);
                if let Some(tx) = reply {
                    let _ = tx.send(());
                }
            }
            SchedulerCommand::DropQueueConsumers { queue_id } => {
                info!(%queue_id, "drop queue consumers command received (leader loss)");
                self.drop_queue_consumers(&queue_id);
            }
            SchedulerCommand::Shutdown => {
                info!("shutdown command received, draining remaining commands");
                self.running = false;
            }
        }
    }

    /// Access the storage layer (used by tests).
    #[cfg(test)]
    pub fn storage(&self) -> &dyn StorageEngine {
        self.storage.as_ref()
    }
}

#[cfg(test)]
mod tests;
