use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use crossbeam_channel::Receiver;
use tracing::{debug, error, info, warn};

use crate::broker::command::{ReadyMessage, SchedulerCommand};
use crate::broker::config::{LuaConfig, SchedulerConfig};
use crate::broker::drr::DrrScheduler;
use crate::lua::LuaEngine;
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
    /// Per-queue round-robin index for delivering messages to consumers.
    /// Persists across calls so messages are distributed fairly across
    /// consumers within each queue independently.
    consumer_rr_idx: HashMap<String, usize>,
    /// Deficit Round Robin scheduler for fair message delivery across
    /// fairness keys.
    drr: DrrScheduler,
    /// Lua rules engine for executing on_enqueue and on_failure scripts.
    lua_engine: Option<LuaEngine>,
}

impl Scheduler {
    pub fn new(
        storage: Arc<dyn Storage>,
        inbound: Receiver<SchedulerCommand>,
        config: &SchedulerConfig,
        lua_config: &LuaConfig,
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
            lua_engine,
        }
    }

    /// Run the scheduler event loop. This blocks the current thread until
    /// a `Shutdown` command is received or the inbound channel is disconnected.
    pub fn run(&mut self) {
        info!("scheduler started");
        self.recover();

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

            // Phase 2: DRR delivery round — deliver ready messages fairly.
            // Runs after every drain, including the final iteration before
            // shutdown, so newly enqueued messages are delivered.
            self.drr_deliver();

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
            SchedulerCommand::Shutdown => {
                info!("shutdown command received, draining remaining commands");
                self.running = false;
            }
        }
    }

    fn handle_enqueue(
        &mut self,
        mut message: crate::message::Message,
    ) -> Result<uuid::Uuid, crate::error::EnqueueError> {
        // Verify queue exists
        if self.storage.get_queue(&message.queue_id)?.is_none() {
            return Err(crate::error::EnqueueError::QueueNotFound(
                message.queue_id.clone(),
            ));
        }

        // Run on_enqueue Lua script if one is cached for this queue
        if let Some(ref mut lua_engine) = self.lua_engine {
            if let Some(result) = lua_engine.run_on_enqueue(
                &message.queue_id,
                &message.headers,
                message.payload.len(),
                &message.queue_id,
            ) {
                message.fairness_key = result.fairness_key;
                message.weight = result.weight;
                message.throttle_keys = result.throttle_keys;
            }
        }

        let msg_id = message.id;
        let key = crate::storage::keys::message_key(
            &message.queue_id,
            &message.fairness_key,
            message.enqueued_at,
            &msg_id,
        );

        self.storage.put_message(&key, &message)?;

        // Register the fairness key in DRR active set so it participates in scheduling
        self.drr
            .add_key(&message.queue_id, &message.fairness_key, message.weight);

        Ok(msg_id)
    }

    fn handle_create_queue(
        &mut self,
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

        // Register per-queue safety config and compile Lua scripts if provided
        if config.on_enqueue_script.is_some() || config.on_failure_script.is_some() {
            if let Some(ref mut lua_engine) = self.lua_engine {
                lua_engine.register_queue_safety(
                    &name,
                    config.lua_timeout_ms,
                    config.lua_memory_limit_bytes,
                );
            }
        }

        if let Some(ref script_source) = config.on_enqueue_script {
            if let Some(ref mut lua_engine) = self.lua_engine {
                let bytecode = lua_engine
                    .compile_script(script_source)
                    .map_err(|e| crate::error::CreateQueueError::LuaCompilation(e.to_string()))?;
                lua_engine.cache_on_enqueue(&name, bytecode);
                debug!(queue = %name, "on_enqueue script compiled and cached");
            } else {
                warn!(queue = %name, "on_enqueue script provided but Lua engine is disabled");
            }
        }

        if let Some(ref script_source) = config.on_failure_script {
            if let Some(ref mut lua_engine) = self.lua_engine {
                let bytecode = lua_engine
                    .compile_script(script_source)
                    .map_err(|e| crate::error::CreateQueueError::LuaCompilation(e.to_string()))?;
                lua_engine.cache_on_failure(&name, bytecode);
                debug!(queue = %name, "on_failure script compiled and cached");
            } else {
                warn!(queue = %name, "on_failure script provided but Lua engine is disabled");
            }
        }

        config.name = name.clone();
        self.storage.put_queue(&name, &config)?;
        Ok(name)
    }

    fn handle_delete_queue(
        &mut self,
        queue_id: &str,
    ) -> Result<(), crate::error::DeleteQueueError> {
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
        self.drr.remove_queue(queue_id);
        self.consumer_rr_idx.remove(queue_id);
        if let Some(ref mut lua_engine) = self.lua_engine {
            lua_engine.remove_queue_scripts(queue_id);
        }
        Ok(())
    }

    fn handle_ack(
        &self,
        queue_id: &str,
        msg_id: &uuid::Uuid,
    ) -> Result<(), crate::error::AckError> {
        // Look up the lease — if it doesn't exist, the message is unknown or already acked
        let lease_key = crate::storage::keys::lease_key(queue_id, msg_id);
        let lease_value = self.storage.get_lease(&lease_key)?.ok_or_else(|| {
            crate::error::AckError::MessageNotFound(format!(
                "no lease for message {msg_id} in queue {queue_id}"
            ))
        })?;

        // Parse expiry timestamp from lease value to construct the lease_expiry key
        let expiry_ns = crate::storage::keys::parse_expiry_from_lease_value(&lease_value)
            .ok_or_else(|| {
                crate::error::AckError::Storage(crate::error::StorageError::CorruptData(format!(
                    "lease value: cannot parse expiry for message {msg_id} in queue {queue_id}"
                )))
            })?;
        let expiry_key = crate::storage::keys::lease_expiry_key(expiry_ns, queue_id, msg_id);

        // Find the message key by scanning the queue's messages
        let msg_key = self.find_message_key(queue_id, msg_id)?;

        // Atomically delete the message, lease, and lease expiry
        let mut ops = vec![
            WriteBatchOp::DeleteLease { key: lease_key },
            WriteBatchOp::DeleteLeaseExpiry { key: expiry_key },
        ];
        if let Some(key) = msg_key {
            ops.push(WriteBatchOp::DeleteMessage { key });
        }

        self.storage.write_batch(ops)?;
        Ok(())
    }

    fn handle_nack(
        &mut self,
        queue_id: &str,
        msg_id: &uuid::Uuid,
        error: &str,
    ) -> Result<(), crate::error::NackError> {
        // Look up the lease — if it doesn't exist, the message was never leased or already nacked/acked
        let lease_key = crate::storage::keys::lease_key(queue_id, msg_id);
        let lease_value = self.storage.get_lease(&lease_key)?.ok_or_else(|| {
            crate::error::NackError::MessageNotFound(format!(
                "no lease for message {msg_id} in queue {queue_id}"
            ))
        })?;

        // Parse expiry timestamp from lease value to construct the lease_expiry key
        let expiry_ns = crate::storage::keys::parse_expiry_from_lease_value(&lease_value)
            .ok_or_else(|| {
                crate::error::NackError::Storage(crate::error::StorageError::CorruptData(format!(
                    "lease value: cannot parse expiry for message {msg_id} in queue {queue_id}"
                )))
            })?;
        let expiry_key = crate::storage::keys::lease_expiry_key(expiry_ns, queue_id, msg_id);

        // Find the full message key and retrieve the message
        let msg_key = self.find_message_key(queue_id, msg_id)?.ok_or_else(|| {
            crate::error::NackError::MessageNotFound(format!(
                "message {msg_id} not found in queue {queue_id}"
            ))
        })?;
        let mut msg = self.storage.get_message(&msg_key)?.ok_or_else(|| {
            crate::error::NackError::MessageNotFound(format!(
                "message {msg_id} not found in queue {queue_id}"
            ))
        })?;

        // Increment attempt count and clear lease timestamp
        msg.attempt_count += 1;
        msg.leased_at = None;

        // Run on_failure Lua script to decide retry vs dead-letter
        let failure_result = self.lua_engine.as_mut().and_then(|engine| {
            engine.run_on_failure(
                queue_id,
                &msg.headers,
                &msg_id.to_string(),
                msg.attempt_count,
                queue_id,
                error,
            )
        });

        let should_dlq = matches!(
            failure_result,
            Some(crate::lua::OnFailureResult {
                action: crate::lua::FailureAction::DeadLetter,
                ..
            })
        );

        if should_dlq {
            // Look up the DLQ queue ID from the queue config
            let dlq_queue_id = self
                .storage
                .get_queue(queue_id)?
                .and_then(|config| config.dlq_queue_id.clone());

            if let Some(dlq_queue_id) =
                dlq_queue_id.filter(|id| self.storage.get_queue(id).ok().flatten().is_some())
            {
                // Move message to DLQ atomically: delete from original queue, put in DLQ
                msg.queue_id = dlq_queue_id.clone();
                let dlq_msg_key = crate::storage::keys::message_key(
                    &dlq_queue_id,
                    &msg.fairness_key,
                    msg.enqueued_at,
                    &msg.id,
                );
                let msg_value =
                    serde_json::to_vec(&msg).map_err(crate::error::StorageError::from)?;

                let ops = vec![
                    WriteBatchOp::DeleteMessage { key: msg_key },
                    WriteBatchOp::PutMessage {
                        key: dlq_msg_key,
                        value: msg_value,
                    },
                    WriteBatchOp::DeleteLease { key: lease_key },
                    WriteBatchOp::DeleteLeaseExpiry { key: expiry_key },
                ];
                self.storage.write_batch(ops)?;

                // Add the message to the DLQ's DRR active set for delivery
                self.drr
                    .add_key(&dlq_queue_id, &msg.fairness_key, msg.weight);

                debug!(
                    %queue_id,
                    %msg_id,
                    %error,
                    %dlq_queue_id,
                    attempt_count = msg.attempt_count,
                    "message moved to dead-letter queue"
                );
                return Ok(());
            }

            warn!(
                %queue_id,
                %msg_id,
                "on_failure requested DLQ but no valid dlq_queue_id configured, retrying instead"
            );
            // Fall through to retry path
        }

        // Retry path: update message in-place, clear lease
        let msg_value = serde_json::to_vec(&msg).map_err(crate::error::StorageError::from)?;

        let ops = vec![
            WriteBatchOp::PutMessage {
                key: msg_key,
                value: msg_value,
            },
            WriteBatchOp::DeleteLease { key: lease_key },
            WriteBatchOp::DeleteLeaseExpiry { key: expiry_key },
        ];
        self.storage.write_batch(ops)?;

        // Re-add the fairness key to DRR active set so the message can be scheduled
        self.drr.add_key(queue_id, &msg.fairness_key, msg.weight);

        debug!(%queue_id, %msg_id, %error, attempt_count = msg.attempt_count, "nack processed");
        Ok(())
    }

    /// Find the full message key in the messages CF by scanning the queue prefix.
    fn find_message_key(
        &self,
        queue_id: &str,
        msg_id: &uuid::Uuid,
    ) -> Result<Option<Vec<u8>>, crate::error::StorageError> {
        let prefix = crate::storage::keys::message_prefix(queue_id);
        let messages = self.storage.list_messages(&prefix)?;
        for (key, msg) in messages {
            if msg.id == *msg_id {
                return Ok(Some(key));
            }
        }
        Ok(None)
    }

    /// Run one DRR delivery round across all queues that have active keys
    /// and registered consumers.
    fn drr_deliver(&mut self) {
        // Collect queue IDs that have both consumers and active DRR keys
        let queue_ids: Vec<String> = self
            .drr
            .queue_ids()
            .into_iter()
            .filter(|qid| {
                self.drr.has_active_keys(qid) && self.consumers.values().any(|e| e.queue_id == *qid)
            })
            .collect();

        for queue_id in &queue_ids {
            self.drr_deliver_queue(queue_id);
        }
    }

    /// DRR delivery for a single queue. Starts a new round if needed, then
    /// delivers messages from each fairness key until deficit is exhausted
    /// or all keys are served.
    fn drr_deliver_queue(&mut self, queue_id: &str) {
        if !self.drr.has_active_keys(queue_id) {
            return;
        }

        // Check there are consumers for this queue — no point running DRR without them
        let has_consumers = self.consumers.values().any(|e| e.queue_id == queue_id);
        if !has_consumers {
            return;
        }

        // Start a new round (refills deficits for all active keys)
        self.drr.start_new_round(queue_id);

        let visibility_timeout_ms = self
            .storage
            .get_queue(queue_id)
            .ok()
            .flatten()
            .map(|c| c.visibility_timeout_ms)
            .unwrap_or(30_000);

        loop {
            let Some(fairness_key) = self.drr.next_key(queue_id) else {
                // Round exhausted — all keys have non-positive deficit
                break;
            };

            // Find the next unleased message for this fairness key
            let prefix = crate::storage::keys::message_prefix_with_key(queue_id, &fairness_key);
            let messages = match self.storage.list_messages(&prefix) {
                Ok(msgs) => msgs,
                Err(e) => {
                    error!(%queue_id, %fairness_key, error = %e, "failed to list messages");
                    break;
                }
            };

            // Find the first unleased message for this key
            let mut found_unleased = false;
            let mut delivered = false;
            for (_key, msg) in messages {
                let lease_key = crate::storage::keys::lease_key(queue_id, &msg.id);
                if self.storage.get_lease(&lease_key).ok().flatten().is_some() {
                    continue;
                }

                found_unleased = true;
                if self.try_deliver_to_consumer(queue_id, &msg, &lease_key, visibility_timeout_ms) {
                    self.drr.consume_deficit(queue_id, &fairness_key);
                    delivered = true;
                }
                break; // Only attempt one message per DRR iteration for this key
            }

            if !found_unleased {
                // No unleased messages exist for this key — remove from active set
                self.drr.remove_key(queue_id, &fairness_key);
            } else if !delivered {
                // Messages exist but couldn't deliver (all consumers full/closed).
                // Break without consuming deficit — the consumer-full condition
                // applies to all keys equally, so burning this key's deficit would
                // unfairly penalize whichever key happened to be next in the round.
                break;
            }
        }
    }

    /// Attempt to deliver a single message to a consumer via round-robin.
    /// Returns true if delivery succeeded.
    fn try_deliver_to_consumer(
        &mut self,
        queue_id: &str,
        msg: &crate::message::Message,
        lease_key: &[u8],
        visibility_timeout_ms: u64,
    ) -> bool {
        let queue_consumers: Vec<(String, usize)> = self
            .consumers
            .iter()
            .enumerate()
            .filter(|(_, (_, e))| e.queue_id == queue_id)
            .map(|(i, (cid, _))| (cid.clone(), i))
            .collect();

        if queue_consumers.is_empty() {
            return false;
        }

        let rr_idx = self
            .consumer_rr_idx
            .entry(queue_id.to_string())
            .or_insert(0);
        let mut attempts = 0;

        while attempts < queue_consumers.len() {
            let (ref cid, _) = queue_consumers[*rr_idx % queue_consumers.len()];
            *rr_idx = rr_idx.wrapping_add(1);
            attempts += 1;

            let now_ns = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_nanos() as u64;
            let expiry_ns = now_ns + visibility_timeout_ms * 1_000_000;

            let lease_val = crate::storage::keys::lease_value(cid, expiry_ns);
            let expiry_key = crate::storage::keys::lease_expiry_key(expiry_ns, queue_id, &msg.id);

            if let Err(e) = self.storage.write_batch(vec![
                WriteBatchOp::PutLease {
                    key: lease_key.to_vec(),
                    value: lease_val,
                },
                WriteBatchOp::PutLeaseExpiry {
                    key: expiry_key.clone(),
                },
            ]) {
                error!(msg_id = %msg.id, error = %e, "failed to write lease");
                return false;
            }

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

            // Look up the consumer's tx channel
            let Some(entry) = self.consumers.get(cid) else {
                continue;
            };

            match entry.tx.try_send(ready) {
                Ok(()) => return true,
                Err(tokio::sync::mpsc::error::TrySendError::Full(_)) => {
                    warn!(%cid, msg_id = %msg.id, "consumer channel full, trying next");
                    if let Err(e) = self.storage.write_batch(vec![
                        WriteBatchOp::DeleteLease {
                            key: lease_key.to_vec(),
                        },
                        WriteBatchOp::DeleteLeaseExpiry { key: expiry_key },
                    ]) {
                        error!(%cid, msg_id = %msg.id, error = %e, "failed to roll back lease");
                    }
                }
                Err(tokio::sync::mpsc::error::TrySendError::Closed(_)) => {
                    warn!(%cid, msg_id = %msg.id, "consumer channel closed, trying next");
                    if let Err(e) = self.storage.write_batch(vec![
                        WriteBatchOp::DeleteLease {
                            key: lease_key.to_vec(),
                        },
                        WriteBatchOp::DeleteLeaseExpiry { key: expiry_key },
                    ]) {
                        error!(%cid, msg_id = %msg.id, error = %e, "failed to roll back lease");
                    }
                }
            }
        }

        false
    }

    /// Scan the `lease_expiry` CF for expired leases and reclaim them.
    ///
    /// For each expired lease:
    /// 1. Delete the lease and lease_expiry entries
    /// 2. Increment the message's attempt_count and clear leased_at
    /// 3. Re-add the fairness key to the DRR active set
    ///
    /// Returns the number of leases reclaimed.
    fn reclaim_expired_leases(&mut self) -> u64 {
        let now_ns = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos() as u64;

        // Build an upper-bound key for the current timestamp. The lease_expiry
        // key starts with an 8-byte BE timestamp followed by `:` (0x3A). Using
        // 0xFF after the timestamp ensures we sort after any real key at now_ns.
        let mut up_to = Vec::with_capacity(40);
        up_to.extend_from_slice(&now_ns.to_be_bytes());
        up_to.extend_from_slice(&[0xFF; 32]);

        let expired_keys = match self.storage.list_expired_leases(&up_to) {
            Ok(keys) => keys,
            Err(e) => {
                warn!(error = %e, "failed to scan expired leases");
                return 0;
            }
        };

        let mut reclaimed = 0u64;
        for expiry_key in &expired_keys {
            let Some((queue_id, msg_id)) = crate::storage::keys::parse_lease_expiry_key(expiry_key)
            else {
                warn!("corrupt lease_expiry key, skipping");
                continue;
            };

            let lease_key = crate::storage::keys::lease_key(&queue_id, &msg_id);

            // Find the message and update it (increment attempt_count, clear leased_at)
            let msg_key = match self.find_message_key(&queue_id, &msg_id) {
                Ok(Some(key)) => key,
                Ok(None) => {
                    // Message not found — orphaned lease entry, just clean up
                    warn!(%queue_id, %msg_id, "orphaned lease_expiry entry, message not found");
                    let _ = self.storage.write_batch(vec![
                        WriteBatchOp::DeleteLease {
                            key: lease_key.clone(),
                        },
                        WriteBatchOp::DeleteLeaseExpiry {
                            key: expiry_key.clone(),
                        },
                    ]);
                    reclaimed += 1;
                    continue;
                }
                Err(e) => {
                    warn!(error = %e, %queue_id, %msg_id, "failed to find message for expired lease");
                    continue;
                }
            };

            let msg = match self.storage.get_message(&msg_key) {
                Ok(Some(msg)) => msg,
                Ok(None) => {
                    warn!(%queue_id, %msg_id, "message key found but message missing");
                    let _ = self.storage.write_batch(vec![
                        WriteBatchOp::DeleteLease { key: lease_key },
                        WriteBatchOp::DeleteLeaseExpiry {
                            key: expiry_key.clone(),
                        },
                    ]);
                    reclaimed += 1;
                    continue;
                }
                Err(e) => {
                    warn!(error = %e, %queue_id, %msg_id, "failed to read message for expired lease");
                    continue;
                }
            };

            let mut updated_msg = msg;
            updated_msg.attempt_count += 1;
            updated_msg.leased_at = None;

            let msg_value = match serde_json::to_vec(&updated_msg) {
                Ok(v) => v,
                Err(e) => {
                    warn!(error = %e, %queue_id, %msg_id, "failed to serialize message during lease reclaim");
                    continue;
                }
            };

            if let Err(e) = self.storage.write_batch(vec![
                WriteBatchOp::PutMessage {
                    key: msg_key,
                    value: msg_value,
                },
                WriteBatchOp::DeleteLease { key: lease_key },
                WriteBatchOp::DeleteLeaseExpiry {
                    key: expiry_key.clone(),
                },
            ]) {
                warn!(error = %e, %queue_id, %msg_id, "failed to reclaim expired lease");
                continue;
            }

            self.drr
                .add_key(&queue_id, &updated_msg.fairness_key, updated_msg.weight);

            debug!(
                %queue_id,
                %msg_id,
                attempt_count = updated_msg.attempt_count,
                "reclaimed expired lease"
            );
            reclaimed += 1;
        }

        if reclaimed > 0 {
            info!(reclaimed, "reclaimed expired leases");
        }
        reclaimed
    }

    /// Recover state after a crash or restart.
    ///
    /// RocksDB persists all data to disk, so queue definitions, messages, and
    /// leases survive restarts. Recovery does two things:
    /// 1. Reclaim expired leases so messages re-enter the ready pool
    /// 2. Rebuild DRR active keys by scanning the messages CF
    fn recover(&mut self) {
        self.reclaim_expired_leases();

        // Rebuild DRR active keys and Lua script cache by scanning queues
        match self.storage.list_queues() {
            Ok(queues) => {
                for queue in &queues {
                    // Register per-queue safety config if any scripts are present
                    if queue.on_enqueue_script.is_some() || queue.on_failure_script.is_some() {
                        if let Some(ref mut lua_engine) = self.lua_engine {
                            lua_engine.register_queue_safety(
                                &queue.name,
                                queue.lua_timeout_ms,
                                queue.lua_memory_limit_bytes,
                            );
                        }
                    }

                    // Re-compile and cache on_enqueue scripts
                    if let Some(ref script_source) = queue.on_enqueue_script {
                        if let Some(ref mut lua_engine) = self.lua_engine {
                            match lua_engine.compile_script(script_source) {
                                Ok(bytecode) => {
                                    lua_engine.cache_on_enqueue(&queue.name, bytecode);
                                    debug!(queue = %queue.name, "recovered on_enqueue script");
                                }
                                Err(e) => {
                                    warn!(queue = %queue.name, error = %e, "failed to compile on_enqueue script during recovery");
                                }
                            }
                        }
                    }

                    // Re-compile and cache on_failure scripts
                    if let Some(ref script_source) = queue.on_failure_script {
                        if let Some(ref mut lua_engine) = self.lua_engine {
                            match lua_engine.compile_script(script_source) {
                                Ok(bytecode) => {
                                    lua_engine.cache_on_failure(&queue.name, bytecode);
                                    debug!(queue = %queue.name, "recovered on_failure script");
                                }
                                Err(e) => {
                                    warn!(queue = %queue.name, error = %e, "failed to compile on_failure script during recovery");
                                }
                            }
                        }
                    }

                    // Rebuild DRR active keys by scanning messages
                    let prefix = crate::storage::keys::message_prefix(&queue.name);
                    match self.storage.list_messages(&prefix) {
                        Ok(messages) => {
                            for (_, msg) in &messages {
                                self.drr.add_key(&queue.name, &msg.fairness_key, msg.weight);
                            }
                        }
                        Err(e) => {
                            warn!(queue = %queue.name, error = %e, "failed to scan messages during DRR recovery");
                        }
                    }
                }
                info!(queue_count = queues.len(), "recovery complete");
            }
            Err(e) => warn!(error = %e, "failed to list queues during recovery"),
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
            quantum: 1000,
        };
        let (tx, rx) = crossbeam_channel::bounded(config.command_channel_capacity);
        let lua_config = LuaConfig::default();
        let scheduler = Scheduler::new(storage, rx, &config, &lua_config);
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
    fn ack_without_lease_returns_error() {
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

        // Ack without a lease should fail
        let err = reply_rx.try_recv().unwrap().unwrap_err();
        assert!(matches!(err, crate::error::AckError::MessageNotFound(_)));
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

    #[test]
    fn delivery_skips_closed_consumer_and_delivers_to_next() {
        let (tx, mut scheduler, _dir) = test_setup();

        send_create_queue(&tx, "closed-queue");

        // Register c1 with a channel we immediately close (drop receiver)
        let (c1_tx, c1_rx) = tokio::sync::mpsc::channel(64);
        tx.send(SchedulerCommand::RegisterConsumer {
            queue_id: "closed-queue".to_string(),
            consumer_id: "c1".to_string(),
            tx: c1_tx,
        })
        .unwrap();
        drop(c1_rx);

        // Register c2 with a healthy channel
        let (c2_tx, mut c2_rx) = tokio::sync::mpsc::channel(64);
        tx.send(SchedulerCommand::RegisterConsumer {
            queue_id: "closed-queue".to_string(),
            consumer_id: "c2".to_string(),
            tx: c2_tx,
        })
        .unwrap();

        // Enqueue a message
        let msg = test_message("closed-queue");
        let msg_id = msg.id;
        let (reply_tx, _reply_rx) = tokio::sync::oneshot::channel();
        tx.send(SchedulerCommand::Enqueue {
            message: msg,
            reply: reply_tx,
        })
        .unwrap();

        tx.send(SchedulerCommand::Shutdown).unwrap();
        scheduler.run();

        // c2 should have received the message (c1 was closed)
        let ready = c2_rx.try_recv().unwrap();
        assert_eq!(ready.msg_id, msg_id);

        // A lease should exist for the delivered message
        let lease_key = crate::storage::keys::lease_key("closed-queue", &msg_id);
        assert!(
            scheduler.storage().get_lease(&lease_key).unwrap().is_some(),
            "lease should exist for the delivered message"
        );
    }

    #[test]
    fn delivery_rolls_back_lease_when_all_consumers_closed() {
        let (tx, mut scheduler, _dir) = test_setup();

        send_create_queue(&tx, "all-closed-queue");

        // Register two consumers, close both channels
        let (c1_tx, c1_rx) = tokio::sync::mpsc::channel(64);
        tx.send(SchedulerCommand::RegisterConsumer {
            queue_id: "all-closed-queue".to_string(),
            consumer_id: "c1".to_string(),
            tx: c1_tx,
        })
        .unwrap();
        drop(c1_rx);

        let (c2_tx, c2_rx) = tokio::sync::mpsc::channel(64);
        tx.send(SchedulerCommand::RegisterConsumer {
            queue_id: "all-closed-queue".to_string(),
            consumer_id: "c2".to_string(),
            tx: c2_tx,
        })
        .unwrap();
        drop(c2_rx);

        // Enqueue a message
        let msg = test_message("all-closed-queue");
        let msg_id = msg.id;
        let (reply_tx, _reply_rx) = tokio::sync::oneshot::channel();
        tx.send(SchedulerCommand::Enqueue {
            message: msg,
            reply: reply_tx,
        })
        .unwrap();

        tx.send(SchedulerCommand::Shutdown).unwrap();
        scheduler.run();

        // No lease should remain — both were rolled back
        let lease_key = crate::storage::keys::lease_key("all-closed-queue", &msg_id);
        assert!(
            scheduler.storage().get_lease(&lease_key).unwrap().is_none(),
            "lease should be rolled back when all consumers are closed"
        );

        // Message should still exist in storage (not lost)
        let prefix = crate::storage::keys::message_prefix("all-closed-queue");
        let messages = scheduler.storage().list_messages(&prefix).unwrap();
        assert_eq!(
            messages.len(),
            1,
            "message should still be in storage for retry"
        );
    }

    #[test]
    fn delivery_skips_full_consumer_and_delivers_to_next() {
        let (tx, mut scheduler, _dir) = test_setup();

        send_create_queue(&tx, "full-queue");

        // Register c1 with capacity 1, then pre-fill it
        let (c1_tx, mut c1_rx) = tokio::sync::mpsc::channel(1);
        tx.send(SchedulerCommand::RegisterConsumer {
            queue_id: "full-queue".to_string(),
            consumer_id: "c1".to_string(),
            tx: c1_tx,
        })
        .unwrap();

        // Register c2 with plenty of capacity
        let (c2_tx, mut c2_rx) = tokio::sync::mpsc::channel(64);
        tx.send(SchedulerCommand::RegisterConsumer {
            queue_id: "full-queue".to_string(),
            consumer_id: "c2".to_string(),
            tx: c2_tx,
        })
        .unwrap();

        // Enqueue 2 messages — they fill c1 (capacity 1) and overflow to c2
        let mut msg_ids = Vec::new();
        for i in 0u64..2 {
            let mut msg = test_message("full-queue");
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

        // Collect from both consumers
        let mut c1_msgs = Vec::new();
        while let Ok(ready) = c1_rx.try_recv() {
            c1_msgs.push(ready.msg_id);
        }
        let mut c2_msgs = Vec::new();
        while let Ok(ready) = c2_rx.try_recv() {
            c2_msgs.push(ready.msg_id);
        }

        // Both messages should be delivered across the two consumers
        let total = c1_msgs.len() + c2_msgs.len();
        assert_eq!(total, 2, "both messages should be delivered");

        // c2 should have received at least one message (overflow from c1)
        assert!(
            !c2_msgs.is_empty(),
            "c2 should receive messages when c1 is full"
        );

        // Each delivered message should have a lease
        for msg_id in msg_ids {
            let lease_key = crate::storage::keys::lease_key("full-queue", &msg_id);
            assert!(
                scheduler.storage().get_lease(&lease_key).unwrap().is_some(),
                "lease should exist for msg {msg_id}"
            );
        }
    }

    #[test]
    fn delivery_rolls_back_lease_when_all_consumers_full() {
        let (tx, mut scheduler, _dir) = test_setup();

        send_create_queue(&tx, "all-full-queue");

        // Register two consumers, both with capacity 1
        let (c1_tx, mut c1_rx) = tokio::sync::mpsc::channel(1);
        tx.send(SchedulerCommand::RegisterConsumer {
            queue_id: "all-full-queue".to_string(),
            consumer_id: "c1".to_string(),
            tx: c1_tx,
        })
        .unwrap();

        let (c2_tx, mut c2_rx) = tokio::sync::mpsc::channel(1);
        tx.send(SchedulerCommand::RegisterConsumer {
            queue_id: "all-full-queue".to_string(),
            consumer_id: "c2".to_string(),
            tx: c2_tx,
        })
        .unwrap();

        // Enqueue 3 messages — first 2 fill both consumers, 3rd has nowhere to go
        let mut msg_ids = Vec::new();
        for i in 0u64..3 {
            let mut msg = test_message("all-full-queue");
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

        // First 2 messages should have been delivered
        let mut c1_msgs = Vec::new();
        while let Ok(ready) = c1_rx.try_recv() {
            c1_msgs.push(ready.msg_id);
        }
        let mut c2_msgs = Vec::new();
        while let Ok(ready) = c2_rx.try_recv() {
            c2_msgs.push(ready.msg_id);
        }
        let delivered: Vec<Uuid> = c1_msgs.iter().chain(c2_msgs.iter()).copied().collect();
        assert_eq!(delivered.len(), 2, "only 2 messages should be delivered");

        // The 3rd message (not delivered) should have no lease
        let undelivered_id = msg_ids
            .iter()
            .find(|id| !delivered.contains(id))
            .expect("one message should be undelivered");
        let lease_key = crate::storage::keys::lease_key("all-full-queue", undelivered_id);
        assert!(
            scheduler.storage().get_lease(&lease_key).unwrap().is_none(),
            "undelivered message should have no lease (rolled back)"
        );

        // The undelivered message should still be in storage
        let prefix = crate::storage::keys::message_prefix("all-full-queue");
        let messages = scheduler.storage().list_messages(&prefix).unwrap();
        assert_eq!(
            messages.len(),
            3,
            "all 3 messages should still be in storage"
        );
    }

    #[test]
    fn ack_removes_message_lease_and_expiry() {
        let (tx, mut scheduler, _dir) = test_setup();

        send_create_queue(&tx, "ack-queue");

        // Register consumer to get a lease
        let (consumer_tx, mut consumer_rx) = tokio::sync::mpsc::channel(64);
        tx.send(SchedulerCommand::RegisterConsumer {
            queue_id: "ack-queue".to_string(),
            consumer_id: "c1".to_string(),
            tx: consumer_tx,
        })
        .unwrap();

        // Enqueue a message (will be delivered and leased)
        let msg = test_message("ack-queue");
        let msg_id = msg.id;
        let (enq_tx, _enq_rx) = tokio::sync::oneshot::channel();
        tx.send(SchedulerCommand::Enqueue {
            message: msg,
            reply: enq_tx,
        })
        .unwrap();

        // Ack the message
        let (ack_tx, mut ack_rx) = tokio::sync::oneshot::channel();
        tx.send(SchedulerCommand::Ack {
            queue_id: "ack-queue".to_string(),
            msg_id,
            reply: ack_tx,
        })
        .unwrap();

        tx.send(SchedulerCommand::Shutdown).unwrap();
        scheduler.run();

        // Consumer should have received the message
        assert!(consumer_rx.try_recv().is_ok());

        // Ack should succeed
        assert!(ack_rx.try_recv().unwrap().is_ok());

        // Message should be gone from messages CF
        let msg_key =
            crate::storage::keys::message_key("ack-queue", "default", 1_000_000_000, &msg_id);
        assert!(
            scheduler.storage().get_message(&msg_key).unwrap().is_none(),
            "message should be deleted after ack"
        );

        // Lease should be gone
        let lease_key = crate::storage::keys::lease_key("ack-queue", &msg_id);
        assert!(
            scheduler.storage().get_lease(&lease_key).unwrap().is_none(),
            "lease should be deleted after ack"
        );
    }

    #[test]
    fn ack_unknown_message_returns_not_found() {
        let (tx, mut scheduler, _dir) = test_setup();

        send_create_queue(&tx, "ack-unknown-queue");

        let msg_id = Uuid::now_v7();
        let (ack_tx, mut ack_rx) = tokio::sync::oneshot::channel();
        tx.send(SchedulerCommand::Ack {
            queue_id: "ack-unknown-queue".to_string(),
            msg_id,
            reply: ack_tx,
        })
        .unwrap();

        tx.send(SchedulerCommand::Shutdown).unwrap();
        scheduler.run();

        let err = ack_rx.try_recv().unwrap().unwrap_err();
        assert!(
            matches!(err, crate::error::AckError::MessageNotFound(_)),
            "expected MessageNotFound, got {err:?}"
        );
    }

    // --- Recovery tests ---

    /// Helper: create a scheduler sharing an existing storage (for restart tests).
    fn test_setup_with_storage(
        storage: Arc<dyn Storage>,
    ) -> (crossbeam_channel::Sender<SchedulerCommand>, Scheduler) {
        let config = SchedulerConfig {
            command_channel_capacity: 256,
            idle_timeout_ms: 10,
            quantum: 1000,
        };
        let (tx, rx) = crossbeam_channel::bounded(config.command_channel_capacity);
        let lua_config = LuaConfig::default();
        let scheduler = Scheduler::new(storage, rx, &config, &lua_config);
        (tx, scheduler)
    }

    #[test]
    fn recovery_preserves_messages_after_restart() {
        let dir = tempfile::tempdir().unwrap();
        let storage: Arc<dyn Storage> = Arc::new(RocksDbStorage::open(dir.path()).unwrap());

        // Phase 1: enqueue messages, then shut down the scheduler
        let (tx, mut scheduler) = test_setup_with_storage(Arc::clone(&storage));
        send_create_queue(&tx, "recover-queue");

        let mut msg_ids = Vec::new();
        for i in 0u64..5 {
            let mut msg = test_message("recover-queue");
            msg.enqueued_at = i;
            msg_ids.push(msg.id);
            let (reply_tx, _) = tokio::sync::oneshot::channel();
            tx.send(SchedulerCommand::Enqueue {
                message: msg,
                reply: reply_tx,
            })
            .unwrap();
        }
        tx.send(SchedulerCommand::Shutdown).unwrap();
        scheduler.run();
        drop(tx);

        // Phase 2: create a brand-new scheduler on the same storage
        let (tx2, mut scheduler2) = test_setup_with_storage(Arc::clone(&storage));

        // Register consumer — should receive all 5 messages (they had no leases)
        let (consumer_tx, mut consumer_rx) = tokio::sync::mpsc::channel(64);
        tx2.send(SchedulerCommand::RegisterConsumer {
            queue_id: "recover-queue".to_string(),
            consumer_id: "c1".to_string(),
            tx: consumer_tx,
        })
        .unwrap();
        tx2.send(SchedulerCommand::Shutdown).unwrap();
        scheduler2.run();

        let mut received = Vec::new();
        while let Ok(ready) = consumer_rx.try_recv() {
            received.push(ready.msg_id);
        }
        assert_eq!(received.len(), 5, "all 5 messages should survive restart");
        assert_eq!(received, msg_ids, "messages in correct FIFO order");
    }

    #[test]
    fn recovery_reclaims_expired_leases() {
        let dir = tempfile::tempdir().unwrap();
        let storage: Arc<dyn Storage> = Arc::new(RocksDbStorage::open(dir.path()).unwrap());

        // Phase 1: set up a queue, enqueue a message, and manually create an
        // expired lease (simulating a crash while a message was in-flight)
        let config = crate::queue::QueueConfig::new("reclaim-queue".to_string());
        storage.put_queue("reclaim-queue", &config).unwrap();

        let msg = test_message("reclaim-queue");
        let msg_id = msg.id;
        let msg_key = crate::storage::keys::message_key(
            "reclaim-queue",
            &msg.fairness_key,
            msg.enqueued_at,
            &msg_id,
        );
        storage.put_message(&msg_key, &msg).unwrap();

        // Create a lease with an expiry in the past (1 nanosecond)
        let lease_key = crate::storage::keys::lease_key("reclaim-queue", &msg_id);
        let lease_val = crate::storage::keys::lease_value("old-consumer", 1);
        let expiry_key = crate::storage::keys::lease_expiry_key(1, "reclaim-queue", &msg_id);
        storage
            .write_batch(vec![
                WriteBatchOp::PutLease {
                    key: lease_key.clone(),
                    value: lease_val,
                },
                WriteBatchOp::PutLeaseExpiry {
                    key: expiry_key.clone(),
                },
            ])
            .unwrap();

        // Verify the lease exists before recovery
        assert!(storage.get_lease(&lease_key).unwrap().is_some());

        // Phase 2: start a new scheduler — recovery should reclaim the expired lease
        let (tx, mut scheduler) = test_setup_with_storage(Arc::clone(&storage));

        // Register consumer — the message should be delivered since the expired
        // lease was reclaimed during recovery
        let (consumer_tx, mut consumer_rx) = tokio::sync::mpsc::channel(64);
        tx.send(SchedulerCommand::RegisterConsumer {
            queue_id: "reclaim-queue".to_string(),
            consumer_id: "c1".to_string(),
            tx: consumer_tx,
        })
        .unwrap();
        tx.send(SchedulerCommand::Shutdown).unwrap();
        scheduler.run();

        // The message should have been delivered to the consumer — this proves
        // the expired lease was reclaimed during recovery, since try_deliver_pending
        // skips messages that have an active lease.
        let ready = consumer_rx
            .try_recv()
            .expect("consumer should receive the reclaimed message");
        assert_eq!(ready.msg_id, msg_id);
        assert_eq!(
            ready.attempt_count, 1,
            "attempt_count should be incremented after lease expiry reclaim"
        );

        // The old lease_expiry entry should be gone (recovery deleted it)
        let old_up_to =
            crate::storage::keys::lease_expiry_key(2, "reclaim-queue", &uuid::Uuid::max());
        let old_expired = storage.list_expired_leases(&old_up_to).unwrap();
        assert!(
            old_expired.is_empty(),
            "expired lease_expiry entry at ts=1 should be removed by recovery"
        );
    }

    #[test]
    fn recovery_preserves_queue_definitions() {
        let dir = tempfile::tempdir().unwrap();
        let storage: Arc<dyn Storage> = Arc::new(RocksDbStorage::open(dir.path()).unwrap());

        // Phase 1: create queues, shut down
        let (tx, mut scheduler) = test_setup_with_storage(Arc::clone(&storage));
        for name in &["q1", "q2", "q3"] {
            send_create_queue(&tx, name);
        }
        tx.send(SchedulerCommand::Shutdown).unwrap();
        scheduler.run();
        drop(tx);

        // Phase 2: reopen — queues should still be there
        let queues = storage.list_queues().unwrap();
        assert_eq!(
            queues.len(),
            3,
            "all 3 queue definitions should survive restart"
        );

        let names: Vec<&str> = queues.iter().map(|q| q.name.as_str()).collect();
        assert!(names.contains(&"q1"));
        assert!(names.contains(&"q2"));
        assert!(names.contains(&"q3"));
    }

    #[test]
    fn shutdown_flushes_wal() {
        let dir = tempfile::tempdir().unwrap();
        let msg_id;

        // Phase 1: enqueue a message and shut down gracefully (which flushes WAL)
        {
            let storage: Arc<dyn Storage> = Arc::new(RocksDbStorage::open(dir.path()).unwrap());
            let (tx, mut scheduler) = test_setup_with_storage(Arc::clone(&storage));
            send_create_queue(&tx, "flush-queue");

            let msg = test_message("flush-queue");
            msg_id = msg.id;
            let (reply_tx, _) = tokio::sync::oneshot::channel();
            tx.send(SchedulerCommand::Enqueue {
                message: msg,
                reply: reply_tx,
            })
            .unwrap();
            tx.send(SchedulerCommand::Shutdown).unwrap();
            scheduler.run();
            // storage + scheduler dropped here, releasing RocksDB lock
        }

        // Phase 2: reopen the database and verify data survived
        let storage2: Arc<dyn Storage> = Arc::new(RocksDbStorage::open(dir.path()).unwrap());
        let prefix = crate::storage::keys::message_prefix("flush-queue");
        let messages = storage2.list_messages(&prefix).unwrap();
        assert_eq!(
            messages.len(),
            1,
            "message should survive WAL flush + reopen"
        );
        assert_eq!(messages[0].1.id, msg_id);
    }

    #[test]
    fn recovery_skips_corrupt_lease_expiry_key() {
        let dir = tempfile::tempdir().unwrap();
        let storage: Arc<dyn Storage> = Arc::new(RocksDbStorage::open(dir.path()).unwrap());

        // Set up a queue with a message
        let config = crate::queue::QueueConfig::new("corrupt-queue".to_string());
        storage.put_queue("corrupt-queue", &config).unwrap();

        let msg = test_message("corrupt-queue");
        let msg_id = msg.id;
        let msg_key = crate::storage::keys::message_key(
            "corrupt-queue",
            &msg.fairness_key,
            msg.enqueued_at,
            &msg_id,
        );
        storage.put_message(&msg_key, &msg).unwrap();

        // Create a valid lease pointing at this message
        let lease_key = crate::storage::keys::lease_key("corrupt-queue", &msg_id);
        let lease_val = crate::storage::keys::lease_value("c1", 1);
        storage
            .write_batch(vec![WriteBatchOp::PutLease {
                key: lease_key.clone(),
                value: lease_val,
            }])
            .unwrap();

        // Write a corrupt lease_expiry key (expired, but unparseable)
        // Use a valid timestamp prefix so the scanner finds it, but garbage after
        let mut corrupt_expiry_key = Vec::new();
        corrupt_expiry_key.extend_from_slice(&1u64.to_be_bytes()); // expired timestamp
        corrupt_expiry_key.push(b':');
        corrupt_expiry_key.extend_from_slice(&[0xFF; 4]); // garbage
        storage
            .write_batch(vec![WriteBatchOp::PutLeaseExpiry {
                key: corrupt_expiry_key.clone(),
            }])
            .unwrap();

        // Start scheduler — recovery should skip the corrupt key without panicking
        let (tx, mut scheduler) = test_setup_with_storage(Arc::clone(&storage));
        tx.send(SchedulerCommand::Shutdown).unwrap();
        scheduler.run();

        // The corrupt lease_expiry entry should still be there (skipped, not deleted)
        let up_to = crate::storage::keys::lease_expiry_key(2, "z", &uuid::Uuid::max());
        let remaining = storage.list_expired_leases(&up_to).unwrap();
        assert!(
            remaining.contains(&corrupt_expiry_key),
            "corrupt expiry key should be skipped, not deleted"
        );

        // The valid lease should also still be there (recovery couldn't match it
        // to the corrupt expiry key, so it wasn't reclaimed)
        assert!(
            storage.get_lease(&lease_key).unwrap().is_some(),
            "lease should survive when expiry key is corrupt"
        );
    }

    #[test]
    fn recovery_preserves_non_expired_leases() {
        let dir = tempfile::tempdir().unwrap();
        let storage: Arc<dyn Storage> = Arc::new(RocksDbStorage::open(dir.path()).unwrap());

        // Set up a queue with a message
        let config = crate::queue::QueueConfig::new("active-lease-queue".to_string());
        storage.put_queue("active-lease-queue", &config).unwrap();

        let msg = test_message("active-lease-queue");
        let msg_id = msg.id;
        let msg_key = crate::storage::keys::message_key(
            "active-lease-queue",
            &msg.fairness_key,
            msg.enqueued_at,
            &msg_id,
        );
        storage.put_message(&msg_key, &msg).unwrap();

        // Create a lease with an expiry far in the future
        let future_expiry = u64::MAX;
        let lease_key = crate::storage::keys::lease_key("active-lease-queue", &msg_id);
        let lease_val = crate::storage::keys::lease_value("active-consumer", future_expiry);
        let expiry_key =
            crate::storage::keys::lease_expiry_key(future_expiry, "active-lease-queue", &msg_id);
        storage
            .write_batch(vec![
                WriteBatchOp::PutLease {
                    key: lease_key.clone(),
                    value: lease_val,
                },
                WriteBatchOp::PutLeaseExpiry {
                    key: expiry_key.clone(),
                },
            ])
            .unwrap();

        // Start scheduler — recovery should NOT reclaim this lease
        let (tx, mut scheduler) = test_setup_with_storage(Arc::clone(&storage));

        // Register a consumer — the message should NOT be delivered because
        // it still has an active (non-expired) lease
        let (consumer_tx, mut consumer_rx) = tokio::sync::mpsc::channel(64);
        tx.send(SchedulerCommand::RegisterConsumer {
            queue_id: "active-lease-queue".to_string(),
            consumer_id: "c1".to_string(),
            tx: consumer_tx,
        })
        .unwrap();
        tx.send(SchedulerCommand::Shutdown).unwrap();
        scheduler.run();

        // Consumer should NOT have received the message
        assert!(
            consumer_rx.try_recv().is_err(),
            "message with active lease should not be delivered"
        );

        // Lease and expiry should still exist
        assert!(
            storage.get_lease(&lease_key).unwrap().is_some(),
            "non-expired lease should survive recovery"
        );
    }

    // --- DRR integration tests ---

    /// Helper: create a message with a specific fairness key and weight.
    fn test_message_with_key_and_weight(
        queue_id: &str,
        fairness_key: &str,
        weight: u32,
        enqueued_at: u64,
    ) -> Message {
        Message {
            id: Uuid::now_v7(),
            queue_id: queue_id.to_string(),
            headers: HashMap::new(),
            payload: vec![1, 2, 3],
            fairness_key: fairness_key.to_string(),
            weight,
            throttle_keys: vec![],
            attempt_count: 0,
            enqueued_at,
            leased_at: None,
        }
    }

    /// Helper: create a message with a specific fairness key.
    fn test_message_with_key(queue_id: &str, fairness_key: &str, enqueued_at: u64) -> Message {
        Message {
            id: Uuid::now_v7(),
            queue_id: queue_id.to_string(),
            headers: HashMap::new(),
            payload: vec![1, 2, 3],
            fairness_key: fairness_key.to_string(),
            weight: 1,
            throttle_keys: vec![],
            attempt_count: 0,
            enqueued_at,
            leased_at: None,
        }
    }

    #[test]
    fn drr_three_equal_weight_keys_get_equal_delivery() {
        let (tx, mut scheduler, _dir) = test_setup();

        send_create_queue(&tx, "drr-equal");

        // Register a consumer with enough capacity
        let (consumer_tx, mut consumer_rx) = tokio::sync::mpsc::channel(256);
        tx.send(SchedulerCommand::RegisterConsumer {
            queue_id: "drr-equal".to_string(),
            consumer_id: "c1".to_string(),
            tx: consumer_tx,
        })
        .unwrap();

        // Enqueue 30 messages: 10 for each of 3 fairness keys
        let mut ts = 0u64;
        for key in &["tenant_a", "tenant_b", "tenant_c"] {
            for _ in 0..10 {
                let msg = test_message_with_key("drr-equal", key, ts);
                ts += 1;
                let (reply_tx, _) = tokio::sync::oneshot::channel();
                tx.send(SchedulerCommand::Enqueue {
                    message: msg,
                    reply: reply_tx,
                })
                .unwrap();
            }
        }

        tx.send(SchedulerCommand::Shutdown).unwrap();
        scheduler.run();

        // Collect delivered messages and count per fairness key
        let mut counts: HashMap<String, usize> = HashMap::new();
        while let Ok(ready) = consumer_rx.try_recv() {
            *counts.entry(ready.fairness_key.clone()).or_insert(0) += 1;
        }

        // All 30 messages should be delivered
        let total: usize = counts.values().sum();
        assert_eq!(total, 30, "all 30 messages should be delivered");

        // Each key should get exactly 10 (equal weight, 10 messages each)
        assert_eq!(
            counts.get("tenant_a"),
            Some(&10),
            "tenant_a should get 10 messages"
        );
        assert_eq!(
            counts.get("tenant_b"),
            Some(&10),
            "tenant_b should get 10 messages"
        );
        assert_eq!(
            counts.get("tenant_c"),
            Some(&10),
            "tenant_c should get 10 messages"
        );
    }

    #[test]
    fn drr_single_key_backward_compatible() {
        let (tx, mut scheduler, _dir) = test_setup();

        send_create_queue(&tx, "drr-single");

        // Register consumer
        let (consumer_tx, mut consumer_rx) = tokio::sync::mpsc::channel(64);
        tx.send(SchedulerCommand::RegisterConsumer {
            queue_id: "drr-single".to_string(),
            consumer_id: "c1".to_string(),
            tx: consumer_tx,
        })
        .unwrap();

        // Enqueue 10 messages with default fairness key (Epic 1 behavior)
        let mut msg_ids = Vec::new();
        for i in 0u64..10 {
            let msg = test_message_with_key("drr-single", "default", i);
            msg_ids.push(msg.id);
            let (reply_tx, _) = tokio::sync::oneshot::channel();
            tx.send(SchedulerCommand::Enqueue {
                message: msg,
                reply: reply_tx,
            })
            .unwrap();
        }

        tx.send(SchedulerCommand::Shutdown).unwrap();
        scheduler.run();

        // All 10 messages should be delivered in FIFO order
        let mut received = Vec::new();
        while let Ok(ready) = consumer_rx.try_recv() {
            received.push(ready.msg_id);
        }
        assert_eq!(received.len(), 10, "all 10 messages should be delivered");
        assert_eq!(received, msg_ids, "single-key DRR delivers in FIFO order");
    }

    #[test]
    fn drr_key_exhaustion_continues_other_keys() {
        let (tx, mut scheduler, _dir) = test_setup();

        send_create_queue(&tx, "drr-exhaust");

        // Register consumer
        let (consumer_tx, mut consumer_rx) = tokio::sync::mpsc::channel(256);
        tx.send(SchedulerCommand::RegisterConsumer {
            queue_id: "drr-exhaust".to_string(),
            consumer_id: "c1".to_string(),
            tx: consumer_tx,
        })
        .unwrap();

        // Enqueue: tenant_a has 2 messages, tenant_b has 10 messages
        // tenant_a will exhaust after 2, tenant_b should continue to 10
        let mut ts = 0u64;
        for _ in 0..2 {
            let msg = test_message_with_key("drr-exhaust", "tenant_a", ts);
            ts += 1;
            let (reply_tx, _) = tokio::sync::oneshot::channel();
            tx.send(SchedulerCommand::Enqueue {
                message: msg,
                reply: reply_tx,
            })
            .unwrap();
        }
        for _ in 0..10 {
            let msg = test_message_with_key("drr-exhaust", "tenant_b", ts);
            ts += 1;
            let (reply_tx, _) = tokio::sync::oneshot::channel();
            tx.send(SchedulerCommand::Enqueue {
                message: msg,
                reply: reply_tx,
            })
            .unwrap();
        }

        tx.send(SchedulerCommand::Shutdown).unwrap();
        scheduler.run();

        // Collect delivered messages
        let mut counts: HashMap<String, usize> = HashMap::new();
        while let Ok(ready) = consumer_rx.try_recv() {
            *counts.entry(ready.fairness_key.clone()).or_insert(0) += 1;
        }

        let total: usize = counts.values().sum();
        assert_eq!(total, 12, "all 12 messages should be delivered");
        assert_eq!(
            counts.get("tenant_a"),
            Some(&2),
            "tenant_a exhausted after 2"
        );
        assert_eq!(
            counts.get("tenant_b"),
            Some(&10),
            "tenant_b should get all 10 messages"
        );
    }

    #[test]
    fn drr_weighted_keys_proportional_delivery() {
        let (tx, mut scheduler, _dir) = test_setup();

        send_create_queue(&tx, "drr-weighted");

        // Register consumer
        let (consumer_tx, mut consumer_rx) = tokio::sync::mpsc::channel(256);
        tx.send(SchedulerCommand::RegisterConsumer {
            queue_id: "drr-weighted".to_string(),
            consumer_id: "c1".to_string(),
            tx: consumer_tx,
        })
        .unwrap();

        // Enqueue: tenant_a (weight=3) gets 20 messages, tenant_b (weight=1) gets 20 messages
        // With quantum=1000: tenant_a gets 3000 deficit, tenant_b gets 1000 deficit per round
        // Both have 20 messages each, so both should deliver all 20 within one round
        let mut ts = 0u64;
        for _ in 0..20 {
            let msg = test_message_with_key_and_weight("drr-weighted", "tenant_a", 3, ts);
            ts += 1;
            let (reply_tx, _) = tokio::sync::oneshot::channel();
            tx.send(SchedulerCommand::Enqueue {
                message: msg,
                reply: reply_tx,
            })
            .unwrap();
        }
        for _ in 0..20 {
            let msg = test_message_with_key_and_weight("drr-weighted", "tenant_b", 1, ts);
            ts += 1;
            let (reply_tx, _) = tokio::sync::oneshot::channel();
            tx.send(SchedulerCommand::Enqueue {
                message: msg,
                reply: reply_tx,
            })
            .unwrap();
        }

        tx.send(SchedulerCommand::Shutdown).unwrap();
        scheduler.run();

        // Collect delivered messages
        let mut counts: HashMap<String, usize> = HashMap::new();
        while let Ok(ready) = consumer_rx.try_recv() {
            *counts.entry(ready.fairness_key.clone()).or_insert(0) += 1;
        }

        let total: usize = counts.values().sum();
        assert_eq!(total, 40, "all 40 messages should be delivered");
        assert_eq!(
            counts.get("tenant_a"),
            Some(&20),
            "tenant_a (weight=3) should get all 20 messages"
        );
        assert_eq!(
            counts.get("tenant_b"),
            Some(&20),
            "tenant_b (weight=1) should get all 20 messages"
        );
    }

    /// Full-path fairness accuracy test with 10k+ messages (AC#6).
    ///
    /// Messages are enqueued BEFORE registering the consumer, so the DRR
    /// scheduler exercises fairness under contention (all keys have pending
    /// messages simultaneously). The scheduler runs on a background thread;
    /// the main thread collects deliveries and sends Shutdown after all
    /// messages are received.
    ///
    /// This test is slow in debug mode (~3 minutes) because each DRR delivery
    /// iteration scans the messages CF prefix to find the first unleased message,
    /// giving O(n²) total scan cost. The proptest in drr.rs verifies the same
    /// fairness invariant at arbitrary scale without storage overhead.
    ///
    /// Run explicitly: `cargo test -- --ignored drr_fairness_accuracy`
    #[test]
    #[ignore]
    fn drr_fairness_accuracy_10k_messages_6_keys() {
        // 6 keys with weights 1,2,3,4,5,6 → total_weight = 21
        // quantum=100: per round delivers 21*100 = 2100 messages
        // Each key gets weight * 500 messages → total = 21*500 = 10,500 messages (≥ 10k AC)
        // With quantum=100, each key exhausts after 5 rounds: 5*2100 = 10,500 deliveries
        let weights: Vec<(&str, u32)> = vec![
            ("tenant_a", 1),
            ("tenant_b", 2),
            ("tenant_c", 3),
            ("tenant_d", 4),
            ("tenant_e", 5),
            ("tenant_f", 6),
        ];
        let total_weight: u32 = weights.iter().map(|(_, w)| *w).sum();
        let msgs_per_weight_unit = 500usize;
        let total_msgs: usize = weights
            .iter()
            .map(|(_, w)| *w as usize * msgs_per_weight_unit)
            .sum();

        // Custom setup: larger channel (all commands fit), smaller quantum
        let dir = tempfile::tempdir().unwrap();
        let storage: Arc<dyn Storage> = Arc::new(RocksDbStorage::open(dir.path()).unwrap());
        let config = SchedulerConfig {
            command_channel_capacity: total_msgs + 100, // room for all commands
            idle_timeout_ms: 50,
            quantum: 100,
        };
        let (tx, rx) = crossbeam_channel::bounded(config.command_channel_capacity);

        send_create_queue(&tx, "drr-accuracy");

        // Enqueue ALL messages BEFORE registering consumer — no delivery
        // happens during enqueue because has_consumers check returns false.
        let mut ts = 0u64;
        for (key, weight) in &weights {
            let count = (*weight as usize) * msgs_per_weight_unit;
            for _ in 0..count {
                let msg = test_message_with_key_and_weight("drr-accuracy", key, *weight, ts);
                ts += 1;
                let (reply_tx, _) = tokio::sync::oneshot::channel();
                tx.send(SchedulerCommand::Enqueue {
                    message: msg,
                    reply: reply_tx,
                })
                .unwrap();
            }
        }

        // Register consumer AFTER enqueues — DRR sees all messages at once
        let (consumer_tx, mut consumer_rx) = tokio::sync::mpsc::channel(total_msgs + 100);
        tx.send(SchedulerCommand::RegisterConsumer {
            queue_id: "drr-accuracy".to_string(),
            consumer_id: "c1".to_string(),
            tx: consumer_tx,
        })
        .unwrap();

        // Spawn scheduler on background thread — Scheduler contains Lua (not Send),
        // so it must be created inside the thread.
        let lua_config = LuaConfig::default();
        let handle = std::thread::spawn(move || {
            let mut s = Scheduler::new(storage, rx, &config, &lua_config);
            s.run();
        });

        // Collect delivered messages from consumer channel
        let mut counts: HashMap<String, usize> = HashMap::new();
        let mut total_delivered = 0usize;
        while total_delivered < total_msgs {
            match consumer_rx.blocking_recv() {
                Some(ready) => {
                    *counts.entry(ready.fairness_key.clone()).or_insert(0) += 1;
                    total_delivered += 1;
                }
                None => break, // scheduler dropped consumer_tx
            }
        }

        // Shutdown and join
        let _ = tx.send(SchedulerCommand::Shutdown);
        drop(tx);
        handle.join().unwrap();

        assert!(
            total_delivered >= 10_000,
            "should deliver at least 10,000 messages, got {total_delivered}"
        );

        // Verify each key's share is within 5% of its fair share
        for (key, weight) in &weights {
            let expected_share = *weight as f64 / total_weight as f64;
            let actual = counts.get(*key).copied().unwrap_or(0);
            let actual_share = actual as f64 / total_delivered as f64;
            let diff = (actual_share - expected_share).abs();

            assert!(
                diff <= 0.05,
                "Key {key} (weight={weight}): expected share {expected_share:.4}, actual {actual_share:.4}, diff {diff:.4} > 0.05. Delivered {actual}/{total_delivered}"
            );
        }
    }

    #[test]
    fn ack_same_message_twice_returns_not_found() {
        let (tx, mut scheduler, _dir) = test_setup();

        send_create_queue(&tx, "double-ack-queue");

        // Register consumer
        let (consumer_tx, mut _consumer_rx) = tokio::sync::mpsc::channel(64);
        tx.send(SchedulerCommand::RegisterConsumer {
            queue_id: "double-ack-queue".to_string(),
            consumer_id: "c1".to_string(),
            tx: consumer_tx,
        })
        .unwrap();

        // Enqueue and let it be leased
        let msg = test_message("double-ack-queue");
        let msg_id = msg.id;
        let (enq_tx, _enq_rx) = tokio::sync::oneshot::channel();
        tx.send(SchedulerCommand::Enqueue {
            message: msg,
            reply: enq_tx,
        })
        .unwrap();

        // First ack
        let (ack1_tx, mut ack1_rx) = tokio::sync::oneshot::channel();
        tx.send(SchedulerCommand::Ack {
            queue_id: "double-ack-queue".to_string(),
            msg_id,
            reply: ack1_tx,
        })
        .unwrap();

        // Second ack (same message)
        let (ack2_tx, mut ack2_rx) = tokio::sync::oneshot::channel();
        tx.send(SchedulerCommand::Ack {
            queue_id: "double-ack-queue".to_string(),
            msg_id,
            reply: ack2_tx,
        })
        .unwrap();

        tx.send(SchedulerCommand::Shutdown).unwrap();
        scheduler.run();

        // First ack should succeed
        assert!(
            ack1_rx.try_recv().unwrap().is_ok(),
            "first ack should succeed"
        );

        // Second ack should return NOT_FOUND
        let err = ack2_rx.try_recv().unwrap().unwrap_err();
        assert!(
            matches!(err, crate::error::AckError::MessageNotFound(_)),
            "second ack should return MessageNotFound, got {err:?}"
        );
    }

    #[test]
    fn drr_default_weight_is_one() {
        // Messages enqueued via test_message_with_key use weight=1 (default).
        // Two keys with default weight should get equal delivery.
        let (tx, mut scheduler, _dir) = test_setup();
        send_create_queue(&tx, "default-weight");

        let (consumer_tx, mut consumer_rx) = tokio::sync::mpsc::channel(64);
        tx.send(SchedulerCommand::RegisterConsumer {
            queue_id: "default-weight".to_string(),
            consumer_id: "c1".to_string(),
            tx: consumer_tx,
        })
        .unwrap();

        // Enqueue 20 messages per key, using the default-weight helper
        for i in 0u64..20 {
            let msg = test_message_with_key("default-weight", "key_a", i);
            let (reply_tx, _) = tokio::sync::oneshot::channel();
            tx.send(SchedulerCommand::Enqueue {
                message: msg,
                reply: reply_tx,
            })
            .unwrap();
        }
        for i in 20u64..40 {
            let msg = test_message_with_key("default-weight", "key_b", i);
            let (reply_tx, _) = tokio::sync::oneshot::channel();
            tx.send(SchedulerCommand::Enqueue {
                message: msg,
                reply: reply_tx,
            })
            .unwrap();
        }

        tx.send(SchedulerCommand::Shutdown).unwrap();
        scheduler.run();

        let mut counts: HashMap<String, usize> = HashMap::new();
        while let Ok(ready) = consumer_rx.try_recv() {
            *counts.entry(ready.fairness_key.clone()).or_insert(0) += 1;
        }

        let a = counts.get("key_a").copied().unwrap_or(0);
        let b = counts.get("key_b").copied().unwrap_or(0);
        assert_eq!(a, 20, "key_a should get all 20 messages");
        assert_eq!(b, 20, "key_b should get all 20 messages");
    }

    #[test]
    fn drr_weight_zero_treated_as_one() {
        // AC#3: messages with weight=0 should be clamped to weight=1.
        // Two keys with weight=0 behave the same as weight=1 (equal delivery).
        let (tx, mut scheduler, _dir) = test_setup();
        send_create_queue(&tx, "weight-zero");

        let (consumer_tx, mut consumer_rx) = tokio::sync::mpsc::channel(64);
        tx.send(SchedulerCommand::RegisterConsumer {
            queue_id: "weight-zero".to_string(),
            consumer_id: "c1".to_string(),
            tx: consumer_tx,
        })
        .unwrap();

        for i in 0u64..10 {
            let msg = test_message_with_key_and_weight("weight-zero", "key_a", 0, i);
            let (reply_tx, _) = tokio::sync::oneshot::channel();
            tx.send(SchedulerCommand::Enqueue {
                message: msg,
                reply: reply_tx,
            })
            .unwrap();
        }
        for i in 10u64..20 {
            let msg = test_message_with_key_and_weight("weight-zero", "key_b", 0, i);
            let (reply_tx, _) = tokio::sync::oneshot::channel();
            tx.send(SchedulerCommand::Enqueue {
                message: msg,
                reply: reply_tx,
            })
            .unwrap();
        }

        tx.send(SchedulerCommand::Shutdown).unwrap();
        scheduler.run();

        let mut counts: HashMap<String, usize> = HashMap::new();
        while let Ok(ready) = consumer_rx.try_recv() {
            *counts.entry(ready.fairness_key.clone()).or_insert(0) += 1;
        }

        let a = counts.get("key_a").copied().unwrap_or(0);
        let b = counts.get("key_b").copied().unwrap_or(0);
        assert_eq!(a, 10, "key_a (weight=0→1) should get all 10 messages");
        assert_eq!(b, 10, "key_b (weight=0→1) should get all 10 messages");
    }

    #[test]
    fn drr_weight_update_changes_proportions() {
        // Verify AC#4: weight changes take effect on the next scheduling round.
        //
        // Strategy: enqueue all messages BEFORE registering the consumer.
        // During enqueue (no consumer), drr_deliver_queue returns immediately,
        // so no delivery happens. After RegisterConsumer, the DRR runs with
        // final weights. Shutdown follows immediately, so only ~2 DRR rounds
        // execute, delivering a subset of messages proportional to weights.
        //
        // quantum=5, key_a weight starts at 1 then updates to 3, key_b stays 1.
        // Per round: key_a gets 3*5=15 msgs, key_b gets 1*5=5 msgs.
        // ~2 rounds → key_a≈30, key_b≈10 → 3:1 ratio.
        let dir = tempfile::tempdir().unwrap();
        let storage: Arc<dyn Storage> = Arc::new(RocksDbStorage::open(dir.path()).unwrap());
        let config = SchedulerConfig {
            command_channel_capacity: 256,
            idle_timeout_ms: 10,
            quantum: 5,
        };
        let (tx, rx) = crossbeam_channel::bounded(config.command_channel_capacity);
        let lua_config = LuaConfig::default();
        let mut scheduler = Scheduler::new(storage, rx, &config, &lua_config);

        send_create_queue(&tx, "weight-update");

        // Establish key_a with weight=1
        let msg = test_message_with_key_and_weight("weight-update", "key_a", 1, 0);
        let (reply_tx, _) = tokio::sync::oneshot::channel();
        tx.send(SchedulerCommand::Enqueue {
            message: msg,
            reply: reply_tx,
        })
        .unwrap();

        // Update key_a's weight to 3 via subsequent enqueues
        let mut ts = 1u64;
        for _ in 0..49 {
            let msg = test_message_with_key_and_weight("weight-update", "key_a", 3, ts);
            ts += 1;
            let (reply_tx, _) = tokio::sync::oneshot::channel();
            tx.send(SchedulerCommand::Enqueue {
                message: msg,
                reply: reply_tx,
            })
            .unwrap();
        }

        // key_b: 50 messages with weight=1
        for _ in 0..50 {
            let msg = test_message_with_key_and_weight("weight-update", "key_b", 1, ts);
            ts += 1;
            let (reply_tx, _) = tokio::sync::oneshot::channel();
            tx.send(SchedulerCommand::Enqueue {
                message: msg,
                reply: reply_tx,
            })
            .unwrap();
        }

        // Register consumer AFTER all enqueues — DRR sees all messages at once
        let (consumer_tx, mut consumer_rx) = tokio::sync::mpsc::channel(256);
        tx.send(SchedulerCommand::RegisterConsumer {
            queue_id: "weight-update".to_string(),
            consumer_id: "c1".to_string(),
            tx: consumer_tx,
        })
        .unwrap();

        tx.send(SchedulerCommand::Shutdown).unwrap();
        scheduler.run();

        let mut counts: HashMap<String, usize> = HashMap::new();
        while let Ok(ready) = consumer_rx.try_recv() {
            *counts.entry(ready.fairness_key.clone()).or_insert(0) += 1;
        }

        let total: usize = counts.values().sum();
        let a = counts.get("key_a").copied().unwrap_or(0);

        // With weight 3:1 and quantum=5, each round delivers 15+5=20.
        // 2 rounds → key_a=30, key_b=10 → exactly 75%:25%.
        assert!(total > 0, "should deliver some messages");
        let a_share = a as f64 / total as f64;
        assert!(
            (a_share - 0.75).abs() <= 0.05,
            "key_a (weight=3) expected ~75% share, got {:.1}% ({a}/{total})",
            a_share * 100.0
        );
    }

    #[test]
    fn nack_requeues_message_with_incremented_attempt_count() {
        // AC#8: enqueue → lease → nack → lease again → verify attempt count incremented
        let (tx, mut scheduler, _dir) = test_setup();
        send_create_queue(&tx, "nack-queue");

        // Register consumer
        let (consumer_tx, mut consumer_rx) = tokio::sync::mpsc::channel(64);
        tx.send(SchedulerCommand::RegisterConsumer {
            queue_id: "nack-queue".to_string(),
            consumer_id: "c1".to_string(),
            tx: consumer_tx,
        })
        .unwrap();

        // Enqueue a message (attempt_count starts at 0)
        let msg = test_message("nack-queue");
        let msg_id = msg.id;
        let (enq_tx, _) = tokio::sync::oneshot::channel();
        tx.send(SchedulerCommand::Enqueue {
            message: msg,
            reply: enq_tx,
        })
        .unwrap();

        // Nack the message
        let (nack_tx, mut nack_rx) = tokio::sync::oneshot::channel();
        tx.send(SchedulerCommand::Nack {
            queue_id: "nack-queue".to_string(),
            msg_id,
            error: "processing failed".to_string(),
            reply: nack_tx,
        })
        .unwrap();

        tx.send(SchedulerCommand::Shutdown).unwrap();
        scheduler.run();

        // First delivery: attempt_count = 0
        let first = consumer_rx
            .try_recv()
            .expect("should receive first delivery");
        assert_eq!(first.msg_id, msg_id);
        assert_eq!(first.attempt_count, 0);

        // Nack should succeed
        assert!(nack_rx.try_recv().unwrap().is_ok(), "nack should succeed");

        // Second delivery: attempt_count = 1 (incremented by nack)
        let second = consumer_rx
            .try_recv()
            .expect("should receive second delivery after nack");
        assert_eq!(second.msg_id, msg_id);
        assert_eq!(
            second.attempt_count, 1,
            "attempt count should be incremented after nack"
        );

        // Message should still exist in storage (not deleted — only ack deletes)
        let msg_key =
            crate::storage::keys::message_key("nack-queue", "default", 1_000_000_000, &msg_id);
        assert!(
            scheduler.storage().get_message(&msg_key).unwrap().is_some(),
            "message should still exist after nack (not deleted)"
        );
    }

    #[test]
    fn nack_removes_lease_and_lease_expiry() {
        let (tx, mut scheduler, _dir) = test_setup();
        send_create_queue(&tx, "nack-lease-queue");

        let (consumer_tx, mut consumer_rx) = tokio::sync::mpsc::channel(64);
        tx.send(SchedulerCommand::RegisterConsumer {
            queue_id: "nack-lease-queue".to_string(),
            consumer_id: "c1".to_string(),
            tx: consumer_tx,
        })
        .unwrap();

        let msg = test_message("nack-lease-queue");
        let msg_id = msg.id;
        let (enq_tx, _) = tokio::sync::oneshot::channel();
        tx.send(SchedulerCommand::Enqueue {
            message: msg,
            reply: enq_tx,
        })
        .unwrap();

        // Unregister consumer so nack doesn't immediately re-deliver
        tx.send(SchedulerCommand::UnregisterConsumer {
            consumer_id: "c1".to_string(),
        })
        .unwrap();

        // Nack the message to release the lease
        let (nack_tx, mut nack_rx) = tokio::sync::oneshot::channel();
        tx.send(SchedulerCommand::Nack {
            queue_id: "nack-lease-queue".to_string(),
            msg_id,
            error: "retry please".to_string(),
            reply: nack_tx,
        })
        .unwrap();

        tx.send(SchedulerCommand::Shutdown).unwrap();
        scheduler.run();

        // Consume first delivery
        let _ = consumer_rx.try_recv();
        assert!(nack_rx.try_recv().unwrap().is_ok());

        // Lease should be gone after nack (no re-delivery since consumer unregistered)
        let lease_key = crate::storage::keys::lease_key("nack-lease-queue", &msg_id);
        assert!(
            scheduler.storage().get_lease(&lease_key).unwrap().is_none(),
            "lease should be deleted after nack"
        );

        // Lease expiry entry should also be gone
        let far_future = crate::storage::keys::lease_expiry_key(u64::MAX, "", &Uuid::nil());
        let expired = scheduler
            .storage()
            .list_expired_leases(&far_future)
            .unwrap();
        assert!(
            expired.is_empty(),
            "lease_expiry CF should be empty after nack, found {} entries",
            expired.len()
        );
    }

    #[test]
    fn nack_unknown_message_returns_not_found() {
        let (tx, mut scheduler, _dir) = test_setup();
        send_create_queue(&tx, "nack-unknown-queue");

        let msg_id = Uuid::now_v7();
        let (nack_tx, mut nack_rx) = tokio::sync::oneshot::channel();
        tx.send(SchedulerCommand::Nack {
            queue_id: "nack-unknown-queue".to_string(),
            msg_id,
            error: "test error".to_string(),
            reply: nack_tx,
        })
        .unwrap();

        tx.send(SchedulerCommand::Shutdown).unwrap();
        scheduler.run();

        let err = nack_rx.try_recv().unwrap().unwrap_err();
        assert!(
            matches!(err, crate::error::NackError::MessageNotFound(_)),
            "expected MessageNotFound, got {err:?}"
        );
    }

    #[test]
    fn double_nack_returns_not_found() {
        // First nack succeeds, second nack returns NOT_FOUND because lease is gone
        let (tx, mut scheduler, _dir) = test_setup();
        send_create_queue(&tx, "double-nack-queue");

        let (consumer_tx, mut consumer_rx) = tokio::sync::mpsc::channel(64);
        tx.send(SchedulerCommand::RegisterConsumer {
            queue_id: "double-nack-queue".to_string(),
            consumer_id: "c1".to_string(),
            tx: consumer_tx,
        })
        .unwrap();

        let msg = test_message("double-nack-queue");
        let msg_id = msg.id;
        let (enq_tx, _) = tokio::sync::oneshot::channel();
        tx.send(SchedulerCommand::Enqueue {
            message: msg,
            reply: enq_tx,
        })
        .unwrap();

        // Unregister consumer so nack doesn't immediately re-deliver (which would create a new lease)
        tx.send(SchedulerCommand::UnregisterConsumer {
            consumer_id: "c1".to_string(),
        })
        .unwrap();

        // First nack — should succeed
        let (nack1_tx, mut nack1_rx) = tokio::sync::oneshot::channel();
        tx.send(SchedulerCommand::Nack {
            queue_id: "double-nack-queue".to_string(),
            msg_id,
            error: "first nack".to_string(),
            reply: nack1_tx,
        })
        .unwrap();

        // Second nack — lease is already gone, should return NOT_FOUND
        let (nack2_tx, mut nack2_rx) = tokio::sync::oneshot::channel();
        tx.send(SchedulerCommand::Nack {
            queue_id: "double-nack-queue".to_string(),
            msg_id,
            error: "second nack".to_string(),
            reply: nack2_tx,
        })
        .unwrap();

        tx.send(SchedulerCommand::Shutdown).unwrap();
        scheduler.run();

        // Consume the initial delivery
        let _ = consumer_rx.try_recv();

        assert!(
            nack1_rx.try_recv().unwrap().is_ok(),
            "first nack should succeed"
        );
        let err = nack2_rx.try_recv().unwrap().unwrap_err();
        assert!(
            matches!(err, crate::error::NackError::MessageNotFound(_)),
            "second nack should return MessageNotFound, got {err:?}"
        );
    }

    #[test]
    fn nack_then_ack_completes_message_lifecycle() {
        // Full lifecycle: enqueue → lease → nack → lease again → ack
        let (tx, mut scheduler, _dir) = test_setup();
        send_create_queue(&tx, "nack-ack-queue");

        let (consumer_tx, mut consumer_rx) = tokio::sync::mpsc::channel(64);
        tx.send(SchedulerCommand::RegisterConsumer {
            queue_id: "nack-ack-queue".to_string(),
            consumer_id: "c1".to_string(),
            tx: consumer_tx,
        })
        .unwrap();

        let msg = test_message("nack-ack-queue");
        let msg_id = msg.id;
        let (enq_tx, _) = tokio::sync::oneshot::channel();
        tx.send(SchedulerCommand::Enqueue {
            message: msg,
            reply: enq_tx,
        })
        .unwrap();

        // Nack the message
        let (nack_tx, mut nack_rx) = tokio::sync::oneshot::channel();
        tx.send(SchedulerCommand::Nack {
            queue_id: "nack-ack-queue".to_string(),
            msg_id,
            error: "first attempt failed".to_string(),
            reply: nack_tx,
        })
        .unwrap();

        // Ack the redelivered message
        let (ack_tx, mut ack_rx) = tokio::sync::oneshot::channel();
        tx.send(SchedulerCommand::Ack {
            queue_id: "nack-ack-queue".to_string(),
            msg_id,
            reply: ack_tx,
        })
        .unwrap();

        tx.send(SchedulerCommand::Shutdown).unwrap();
        scheduler.run();

        // First delivery
        let first = consumer_rx.try_recv().expect("first delivery");
        assert_eq!(first.attempt_count, 0);

        // Nack succeeds
        assert!(nack_rx.try_recv().unwrap().is_ok());

        // Second delivery with incremented count
        let second = consumer_rx.try_recv().expect("second delivery after nack");
        assert_eq!(second.attempt_count, 1);

        // Ack succeeds
        assert!(ack_rx.try_recv().unwrap().is_ok());

        // Message should be deleted after ack
        let msg_key =
            crate::storage::keys::message_key("nack-ack-queue", "default", 1_000_000_000, &msg_id);
        assert!(
            scheduler.storage().get_message(&msg_key).unwrap().is_none(),
            "message should be deleted after ack"
        );
    }

    /// Helper: create a queue with a custom visibility timeout (in ms).
    fn send_create_queue_with_timeout(
        tx: &crossbeam_channel::Sender<SchedulerCommand>,
        name: &str,
        visibility_timeout_ms: u64,
    ) {
        let (reply_tx, _reply_rx) = tokio::sync::oneshot::channel();
        let mut config = crate::queue::QueueConfig::new(name.to_string());
        config.visibility_timeout_ms = visibility_timeout_ms;
        tx.send(SchedulerCommand::CreateQueue {
            name: name.to_string(),
            config,
            reply: reply_tx,
        })
        .unwrap();
    }

    #[test]
    fn lease_expiry_redelivers_message_with_incremented_attempt_count() {
        // Use a short visibility timeout (50ms) and idle_timeout (10ms)
        // so the scheduler wakes up frequently and reclaims quickly.
        let dir = tempfile::tempdir().unwrap();
        let storage: Arc<dyn Storage> = Arc::new(RocksDbStorage::open(dir.path()).unwrap());
        let config = SchedulerConfig {
            command_channel_capacity: 256,
            idle_timeout_ms: 10,
            quantum: 1000,
        };
        let (tx, rx) = crossbeam_channel::bounded(config.command_channel_capacity);

        send_create_queue_with_timeout(&tx, "expiry-queue", 50);

        let (consumer_tx, mut consumer_rx) = tokio::sync::mpsc::channel(64);
        tx.send(SchedulerCommand::RegisterConsumer {
            queue_id: "expiry-queue".to_string(),
            consumer_id: "c1".to_string(),
            tx: consumer_tx,
        })
        .unwrap();

        let msg = test_message("expiry-queue");
        let msg_id = msg.id;
        let (enq_tx, _) = tokio::sync::oneshot::channel();
        tx.send(SchedulerCommand::Enqueue {
            message: msg,
            reply: enq_tx,
        })
        .unwrap();

        // Run scheduler on background thread — Scheduler contains Lua (not Send),
        // so it must be created inside the thread.
        let handle = std::thread::spawn(move || {
            let lua_config = LuaConfig::default();
            let mut scheduler = Scheduler::new(storage, rx, &config, &lua_config);
            scheduler.run();
        });

        // First delivery — should happen immediately
        let first = consumer_rx.blocking_recv().expect("first delivery");
        assert_eq!(first.msg_id, msg_id);
        assert_eq!(
            first.attempt_count, 0,
            "first delivery should have attempt_count=0"
        );

        // Wait for the visibility timeout to expire (50ms) plus some buffer
        std::thread::sleep(Duration::from_millis(100));

        // Second delivery — after lease expiry, the message should be redelivered
        let second = consumer_rx
            .blocking_recv()
            .expect("second delivery after expiry");
        assert_eq!(second.msg_id, msg_id);
        assert_eq!(
            second.attempt_count, 1,
            "redelivery after expiry should have attempt_count=1"
        );

        tx.send(SchedulerCommand::Shutdown).unwrap();
        handle.join().unwrap();
    }

    #[test]
    fn lease_expiry_clears_lease_and_expiry_entries() {
        // Verify that lease and lease_expiry CFs are cleaned up after expiry reclaim
        let dir = tempfile::tempdir().unwrap();
        let storage: Arc<dyn Storage> = Arc::new(RocksDbStorage::open(dir.path()).unwrap());
        let config = SchedulerConfig {
            command_channel_capacity: 256,
            idle_timeout_ms: 10,
            quantum: 1000,
        };
        let (tx, rx) = crossbeam_channel::bounded(config.command_channel_capacity);
        let storage_for_thread = Arc::clone(&storage);

        send_create_queue_with_timeout(&tx, "expiry-clean-queue", 50);

        let (consumer_tx, mut consumer_rx) = tokio::sync::mpsc::channel(64);
        tx.send(SchedulerCommand::RegisterConsumer {
            queue_id: "expiry-clean-queue".to_string(),
            consumer_id: "c1".to_string(),
            tx: consumer_tx,
        })
        .unwrap();

        let msg = test_message("expiry-clean-queue");
        let msg_id = msg.id;
        let (enq_tx, _) = tokio::sync::oneshot::channel();
        tx.send(SchedulerCommand::Enqueue {
            message: msg,
            reply: enq_tx,
        })
        .unwrap();

        // Run scheduler on background thread — Scheduler contains Lua (not Send),
        // so it must be created inside the thread.
        let handle = std::thread::spawn(move || {
            let lua_config = LuaConfig::default();
            let mut scheduler = Scheduler::new(storage_for_thread, rx, &config, &lua_config);
            scheduler.run();
        });

        // First delivery
        let _ = consumer_rx.blocking_recv().expect("first delivery");

        // Unregister consumer so expiry reclaim doesn't immediately redeliver
        tx.send(SchedulerCommand::UnregisterConsumer {
            consumer_id: "c1".to_string(),
        })
        .unwrap();

        // Wait for expiry
        std::thread::sleep(Duration::from_millis(100));

        tx.send(SchedulerCommand::Shutdown).unwrap();
        handle.join().unwrap();

        // After reclaim, lease should be gone
        let lease_key = crate::storage::keys::lease_key("expiry-clean-queue", &msg_id);
        assert!(
            storage.get_lease(&lease_key).unwrap().is_none(),
            "lease should be deleted after expiry reclaim"
        );

        // lease_expiry CF should be empty
        let far_future = crate::storage::keys::lease_expiry_key(u64::MAX, "", &Uuid::nil());
        let expired = storage.list_expired_leases(&far_future).unwrap();
        assert!(
            expired.is_empty(),
            "lease_expiry CF should be empty after reclaim, found {} entries",
            expired.len()
        );

        // Message should still exist with leased_at cleared (AC#4)
        let prefix = crate::storage::keys::message_prefix("expiry-clean-queue");
        let messages = storage.list_messages(&prefix).unwrap();
        assert_eq!(
            messages.len(),
            1,
            "message should still exist after expiry reclaim"
        );
        let (_, msg) = &messages[0];
        assert_eq!(msg.id, msg_id);
        assert!(
            msg.leased_at.is_none(),
            "leased_at should be cleared after expiry reclaim"
        );
        assert_eq!(msg.attempt_count, 1, "attempt_count should be incremented");
    }

    #[test]
    fn lease_expiry_multiple_messages_different_timeouts() {
        // Two queues with different visibility timeouts: 50ms and 200ms.
        // After 100ms, only the first message should be redelivered.
        let dir = tempfile::tempdir().unwrap();
        let storage = Arc::new(RocksDbStorage::open(dir.path()).unwrap());
        let config = SchedulerConfig {
            command_channel_capacity: 256,
            idle_timeout_ms: 10,
            quantum: 1000,
        };
        let (tx, rx) = crossbeam_channel::bounded(config.command_channel_capacity);

        send_create_queue_with_timeout(&tx, "fast-queue", 50);
        send_create_queue_with_timeout(&tx, "slow-queue", 200);

        let (consumer_tx_fast, mut consumer_rx_fast) = tokio::sync::mpsc::channel(64);
        tx.send(SchedulerCommand::RegisterConsumer {
            queue_id: "fast-queue".to_string(),
            consumer_id: "c-fast".to_string(),
            tx: consumer_tx_fast,
        })
        .unwrap();

        let (consumer_tx_slow, mut consumer_rx_slow) = tokio::sync::mpsc::channel(64);
        tx.send(SchedulerCommand::RegisterConsumer {
            queue_id: "slow-queue".to_string(),
            consumer_id: "c-slow".to_string(),
            tx: consumer_tx_slow,
        })
        .unwrap();

        let msg_fast = test_message("fast-queue");
        let msg_fast_id = msg_fast.id;
        let (enq_tx1, _) = tokio::sync::oneshot::channel();
        tx.send(SchedulerCommand::Enqueue {
            message: msg_fast,
            reply: enq_tx1,
        })
        .unwrap();

        let msg_slow = test_message("slow-queue");
        let (enq_tx2, _) = tokio::sync::oneshot::channel();
        tx.send(SchedulerCommand::Enqueue {
            message: msg_slow,
            reply: enq_tx2,
        })
        .unwrap();

        // Run scheduler on background thread — Scheduler contains Lua (not Send),
        // so it must be created inside the thread.
        let handle = std::thread::spawn(move || {
            let lua_config = LuaConfig::default();
            let mut scheduler = Scheduler::new(storage, rx, &config, &lua_config);
            scheduler.run();
        });

        // Both messages should be delivered immediately
        let first_fast = consumer_rx_fast
            .blocking_recv()
            .expect("fast first delivery");
        assert_eq!(first_fast.attempt_count, 0);
        let first_slow = consumer_rx_slow
            .blocking_recv()
            .expect("slow first delivery");
        assert_eq!(first_slow.attempt_count, 0);

        // Wait 100ms — fast (50ms) should expire, slow (200ms) should not
        std::thread::sleep(Duration::from_millis(100));

        // Fast message should be redelivered
        let second_fast = consumer_rx_fast
            .blocking_recv()
            .expect("fast second delivery");
        assert_eq!(second_fast.msg_id, msg_fast_id);
        assert_eq!(second_fast.attempt_count, 1);

        // Slow message should NOT have been redelivered yet
        assert!(
            consumer_rx_slow.try_recv().is_err(),
            "slow message should not have expired yet"
        );

        tx.send(SchedulerCommand::Shutdown).unwrap();
        handle.join().unwrap();
    }

    #[test]
    fn ack_before_expiry_prevents_redelivery() {
        // Ack within the visibility timeout prevents the message from being redelivered
        let dir = tempfile::tempdir().unwrap();
        let storage = Arc::new(RocksDbStorage::open(dir.path()).unwrap());
        let config = SchedulerConfig {
            command_channel_capacity: 256,
            idle_timeout_ms: 10,
            quantum: 1000,
        };
        let (tx, rx) = crossbeam_channel::bounded(config.command_channel_capacity);

        send_create_queue_with_timeout(&tx, "ack-before-expiry-queue", 100);

        let (consumer_tx, mut consumer_rx) = tokio::sync::mpsc::channel(64);
        tx.send(SchedulerCommand::RegisterConsumer {
            queue_id: "ack-before-expiry-queue".to_string(),
            consumer_id: "c1".to_string(),
            tx: consumer_tx,
        })
        .unwrap();

        let msg = test_message("ack-before-expiry-queue");
        let msg_id = msg.id;
        let (enq_tx, _) = tokio::sync::oneshot::channel();
        tx.send(SchedulerCommand::Enqueue {
            message: msg,
            reply: enq_tx,
        })
        .unwrap();

        // Run scheduler on background thread — Scheduler contains Lua (not Send),
        // so it must be created inside the thread.
        let handle = std::thread::spawn(move || {
            let lua_config = LuaConfig::default();
            let mut scheduler = Scheduler::new(storage, rx, &config, &lua_config);
            scheduler.run();
        });

        // First delivery
        let first = consumer_rx.blocking_recv().expect("first delivery");
        assert_eq!(first.msg_id, msg_id);

        // Ack immediately (before the 100ms visibility timeout)
        let (ack_tx, ack_rx) = tokio::sync::oneshot::channel();
        tx.send(SchedulerCommand::Ack {
            queue_id: "ack-before-expiry-queue".to_string(),
            msg_id,
            reply: ack_tx,
        })
        .unwrap();

        // Wait to ensure ack is processed
        std::thread::sleep(Duration::from_millis(10));
        assert!(
            ack_rx.blocking_recv().unwrap().is_ok(),
            "ack should succeed"
        );

        // Wait past the visibility timeout
        std::thread::sleep(Duration::from_millis(150));

        // No redelivery should happen — the message was acked
        assert!(
            consumer_rx.try_recv().is_err(),
            "acked message should not be redelivered after visibility timeout"
        );

        tx.send(SchedulerCommand::Shutdown).unwrap();
        handle.join().unwrap();
    }

    // --- Lua on_enqueue integration tests ---

    /// Helper: create a queue with an on_enqueue Lua script.
    fn send_create_queue_with_script(
        tx: &crossbeam_channel::Sender<SchedulerCommand>,
        name: &str,
        script: &str,
    ) -> tokio::sync::oneshot::Receiver<Result<String, crate::error::CreateQueueError>> {
        let (reply_tx, reply_rx) = tokio::sync::oneshot::channel();
        let mut config = crate::queue::QueueConfig::new(name.to_string());
        config.on_enqueue_script = Some(script.to_string());
        tx.send(SchedulerCommand::CreateQueue {
            name: name.to_string(),
            config,
            reply: reply_tx,
        })
        .unwrap();
        reply_rx
    }

    /// Helper: create a message with specific headers.
    fn test_message_with_headers(queue_id: &str, headers: HashMap<String, String>) -> Message {
        Message {
            id: Uuid::now_v7(),
            queue_id: queue_id.to_string(),
            headers,
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
    fn on_enqueue_assigns_fairness_key_from_header() {
        let (tx, mut scheduler, _dir) = test_setup();

        let script = r#"
            function on_enqueue(msg)
                return { fairness_key = msg.headers["tenant_id"] or "unknown" }
            end
        "#;
        send_create_queue_with_script(&tx, "lua-fk-queue", script);

        let mut headers = HashMap::new();
        headers.insert("tenant_id".to_string(), "acme".to_string());
        let msg = test_message_with_headers("lua-fk-queue", headers);
        let msg_id = msg.id;
        let (reply_tx, _) = tokio::sync::oneshot::channel();
        tx.send(SchedulerCommand::Enqueue {
            message: msg,
            reply: reply_tx,
        })
        .unwrap();

        tx.send(SchedulerCommand::Shutdown).unwrap();
        scheduler.run();

        // The message should be stored with fairness_key="acme" from the Lua script
        let key = crate::storage::keys::message_key("lua-fk-queue", "acme", 1_000_000_000, &msg_id);
        let stored = scheduler.storage().get_message(&key).unwrap();
        assert!(
            stored.is_some(),
            "message should be stored with fairness_key='acme' from Lua script"
        );
        let stored_msg = stored.unwrap();
        assert_eq!(stored_msg.fairness_key, "acme");
    }

    #[test]
    fn on_enqueue_assigns_weight_and_throttle_keys() {
        let (tx, mut scheduler, _dir) = test_setup();

        let script = r#"
            function on_enqueue(msg)
                return {
                    fairness_key = "tenant_x",
                    weight = 5,
                    throttle_keys = { "rate:global", "rate:tenant_x" },
                }
            end
        "#;
        send_create_queue_with_script(&tx, "lua-wt-queue", script);

        let msg = test_message("lua-wt-queue");
        let msg_id = msg.id;
        let (reply_tx, _) = tokio::sync::oneshot::channel();
        tx.send(SchedulerCommand::Enqueue {
            message: msg,
            reply: reply_tx,
        })
        .unwrap();

        tx.send(SchedulerCommand::Shutdown).unwrap();
        scheduler.run();

        let key =
            crate::storage::keys::message_key("lua-wt-queue", "tenant_x", 1_000_000_000, &msg_id);
        let stored = scheduler.storage().get_message(&key).unwrap();
        assert!(
            stored.is_some(),
            "message should exist with fairness_key='tenant_x'"
        );
        let stored_msg = stored.unwrap();
        assert_eq!(stored_msg.weight, 5);
        assert_eq!(
            stored_msg.throttle_keys,
            vec!["rate:global", "rate:tenant_x"]
        );
    }

    #[test]
    fn queue_without_script_uses_defaults() {
        let (tx, mut scheduler, _dir) = test_setup();

        send_create_queue(&tx, "no-script-queue");

        let msg = test_message("no-script-queue");
        let msg_id = msg.id;
        let (reply_tx, _) = tokio::sync::oneshot::channel();
        tx.send(SchedulerCommand::Enqueue {
            message: msg,
            reply: reply_tx,
        })
        .unwrap();

        tx.send(SchedulerCommand::Shutdown).unwrap();
        scheduler.run();

        let key =
            crate::storage::keys::message_key("no-script-queue", "default", 1_000_000_000, &msg_id);
        let stored = scheduler.storage().get_message(&key).unwrap();
        assert!(
            stored.is_some(),
            "message should exist with default fairness_key"
        );
        let stored_msg = stored.unwrap();
        assert_eq!(stored_msg.fairness_key, "default");
        assert_eq!(stored_msg.weight, 1);
        assert!(stored_msg.throttle_keys.is_empty());
    }

    #[test]
    fn on_enqueue_reads_config_via_fila_get() {
        let (tx, mut scheduler, _dir) = test_setup();

        // Write a config value to the state CF that the Lua script will read
        scheduler
            .storage()
            .put_state("default_tenant", b"megacorp")
            .unwrap();

        let script = r#"
            function on_enqueue(msg)
                local tenant = msg.headers["tenant_id"]
                if tenant == nil or tenant == "" then
                    tenant = fila.get("default_tenant") or "fallback"
                end
                return { fairness_key = tenant }
            end
        "#;
        send_create_queue_with_script(&tx, "lua-fila-get-queue", script);

        // Message without tenant_id header — script should fall back to fila.get()
        let msg = test_message("lua-fila-get-queue");
        let msg_id = msg.id;
        let (reply_tx, _) = tokio::sync::oneshot::channel();
        tx.send(SchedulerCommand::Enqueue {
            message: msg,
            reply: reply_tx,
        })
        .unwrap();

        tx.send(SchedulerCommand::Shutdown).unwrap();
        scheduler.run();

        let key = crate::storage::keys::message_key(
            "lua-fila-get-queue",
            "megacorp",
            1_000_000_000,
            &msg_id,
        );
        let stored = scheduler.storage().get_message(&key).unwrap();
        assert!(
            stored.is_some(),
            "message should have fairness_key='megacorp' from fila.get()"
        );
        assert_eq!(stored.unwrap().fairness_key, "megacorp");
    }

    #[test]
    fn create_queue_with_invalid_script_returns_error() {
        let (tx, mut scheduler, _dir) = test_setup();

        let mut reply_rx =
            send_create_queue_with_script(&tx, "bad-script-queue", "not valid lua %%%");

        tx.send(SchedulerCommand::Shutdown).unwrap();
        scheduler.run();

        let result = reply_rx.try_recv().unwrap();
        assert!(result.is_err(), "invalid script should return an error");
        assert!(
            matches!(
                result.unwrap_err(),
                crate::error::CreateQueueError::LuaCompilation(_)
            ),
            "error should be LuaCompilation"
        );
    }

    // --- Lua safety integration tests (Story 3.2) ---

    #[test]
    fn on_enqueue_infinite_loop_falls_back_to_defaults() {
        // A script that loops forever should be killed by the instruction hook
        // and the message should get safe defaults.
        let (tx, mut scheduler, _dir) = test_setup();

        let script = r#"
            function on_enqueue(msg)
                while true do end
                return { fairness_key = "unreachable" }
            end
        "#;
        send_create_queue_with_script(&tx, "infinite-loop-queue", script);

        let msg = test_message("infinite-loop-queue");
        let msg_id = msg.id;
        let (reply_tx, mut reply_rx) = tokio::sync::oneshot::channel();
        tx.send(SchedulerCommand::Enqueue {
            message: msg,
            reply: reply_tx,
        })
        .unwrap();

        tx.send(SchedulerCommand::Shutdown).unwrap();
        scheduler.run();

        // Enqueue should succeed (Lua failure falls back to defaults)
        assert!(reply_rx.try_recv().unwrap().is_ok());

        // Message should have default fairness_key
        let key = crate::storage::keys::message_key(
            "infinite-loop-queue",
            "default",
            1_000_000_000,
            &msg_id,
        );
        let stored = scheduler.storage().get_message(&key).unwrap();
        assert!(
            stored.is_some(),
            "message should be stored with default fairness_key after script timeout"
        );
    }

    #[test]
    fn on_enqueue_memory_bomb_falls_back_to_defaults() {
        // A script that tries to allocate too much memory should be killed
        // and the message should get safe defaults.
        let (tx, mut scheduler, _dir) = test_setup();

        let script = r#"
            function on_enqueue(msg)
                local t = {}
                for i = 1, 10000000 do
                    t[i] = string.rep("x", 1000)
                end
                return { fairness_key = "unreachable" }
            end
        "#;
        send_create_queue_with_script(&tx, "memory-bomb-queue", script);

        let msg = test_message("memory-bomb-queue");
        let msg_id = msg.id;
        let (reply_tx, mut reply_rx) = tokio::sync::oneshot::channel();
        tx.send(SchedulerCommand::Enqueue {
            message: msg,
            reply: reply_tx,
        })
        .unwrap();

        tx.send(SchedulerCommand::Shutdown).unwrap();
        scheduler.run();

        // Enqueue should succeed (Lua failure falls back to defaults)
        assert!(reply_rx.try_recv().unwrap().is_ok());

        // Message should have default fairness_key
        let key = crate::storage::keys::message_key(
            "memory-bomb-queue",
            "default",
            1_000_000_000,
            &msg_id,
        );
        let stored = scheduler.storage().get_message(&key).unwrap();
        assert!(
            stored.is_some(),
            "message should be stored with default fairness_key after memory limit"
        );
    }

    #[test]
    fn circuit_breaker_trips_and_bypasses_lua() {
        // After enough consecutive failures, the circuit breaker should trip
        // and bypass Lua entirely (returning defaults without running the script).
        // Default threshold is 3 failures.
        let (tx, mut scheduler, _dir) = test_setup();

        // Script that always fails
        let script = r#"
            function on_enqueue(msg)
                error("always fails")
            end
        "#;
        send_create_queue_with_script(&tx, "cb-queue", script);

        // Enqueue 5 messages — first 3 trip the breaker, last 2 bypass Lua
        let mut msg_ids = Vec::new();
        for i in 0u64..5 {
            let mut msg = test_message("cb-queue");
            msg.enqueued_at = i;
            msg_ids.push(msg.id);
            let (reply_tx, _) = tokio::sync::oneshot::channel();
            tx.send(SchedulerCommand::Enqueue {
                message: msg,
                reply: reply_tx,
            })
            .unwrap();
        }

        tx.send(SchedulerCommand::Shutdown).unwrap();
        scheduler.run();

        // All 5 messages should be stored with default fairness_key
        for (i, msg_id) in msg_ids.iter().enumerate() {
            let key = crate::storage::keys::message_key("cb-queue", "default", i as u64, msg_id);
            let stored = scheduler.storage().get_message(&key).unwrap();
            assert!(
                stored.is_some(),
                "message {i} should be stored with default fairness_key"
            );
        }
    }

    #[test]
    fn failed_script_does_not_break_subsequent_good_scripts() {
        // A failing script for one queue should not affect another queue's scripts.
        // Also verifies the VM is usable after safety hooks fire.
        let (tx, mut scheduler, _dir) = test_setup();

        // Queue A: script always errors
        let bad_script = r#"
            function on_enqueue(msg)
                error("queue A failure")
            end
        "#;
        send_create_queue_with_script(&tx, "bad-queue", bad_script);

        // Queue B: normal script
        let good_script = r#"
            function on_enqueue(msg)
                return { fairness_key = "good" }
            end
        "#;
        send_create_queue_with_script(&tx, "good-queue", good_script);

        // Enqueue to bad queue first, then good queue
        let bad_msg = test_message("bad-queue");
        let (reply_tx1, _) = tokio::sync::oneshot::channel();
        tx.send(SchedulerCommand::Enqueue {
            message: bad_msg,
            reply: reply_tx1,
        })
        .unwrap();

        let good_msg = test_message("good-queue");
        let good_msg_id = good_msg.id;
        let (reply_tx2, _) = tokio::sync::oneshot::channel();
        tx.send(SchedulerCommand::Enqueue {
            message: good_msg,
            reply: reply_tx2,
        })
        .unwrap();

        tx.send(SchedulerCommand::Shutdown).unwrap();
        scheduler.run();

        // Good queue's message should have the correct fairness_key from Lua
        let key =
            crate::storage::keys::message_key("good-queue", "good", 1_000_000_000, &good_msg_id);
        let stored = scheduler.storage().get_message(&key).unwrap();
        assert!(
            stored.is_some(),
            "good queue's Lua script should work even after bad queue's script failed"
        );
        assert_eq!(stored.unwrap().fairness_key, "good");
    }

    // --- on_failure integration tests ---

    /// Helper: create a queue with an on_failure script and optional DLQ configuration.
    fn send_create_queue_with_on_failure(
        tx: &crossbeam_channel::Sender<SchedulerCommand>,
        name: &str,
        on_failure_script: &str,
        dlq_queue_id: Option<&str>,
    ) {
        let (reply_tx, _reply_rx) = tokio::sync::oneshot::channel();
        let mut config = crate::queue::QueueConfig::new(name.to_string());
        config.on_failure_script = Some(on_failure_script.to_string());
        config.dlq_queue_id = dlq_queue_id.map(|s| s.to_string());
        tx.send(SchedulerCommand::CreateQueue {
            name: name.to_string(),
            config,
            reply: reply_tx,
        })
        .unwrap();
    }

    #[test]
    fn on_failure_retry_requeues_message() {
        let (tx, mut scheduler, _dir) = test_setup();

        let on_failure_script = r#"
            function on_failure(msg)
                return { action = "retry" }
            end
        "#;
        send_create_queue_with_on_failure(&tx, "retry-queue", on_failure_script, None);

        // Register consumer to receive messages
        let (consumer_tx, mut consumer_rx) = tokio::sync::mpsc::channel(64);
        tx.send(SchedulerCommand::RegisterConsumer {
            queue_id: "retry-queue".to_string(),
            consumer_id: "c1".to_string(),
            tx: consumer_tx,
        })
        .unwrap();

        // Enqueue a message
        let msg = test_message("retry-queue");
        let msg_id = msg.id;
        let (reply_tx, _) = tokio::sync::oneshot::channel();
        tx.send(SchedulerCommand::Enqueue {
            message: msg,
            reply: reply_tx,
        })
        .unwrap();

        // Nack the message — on_failure returns retry
        let (nack_tx, mut nack_rx) = tokio::sync::oneshot::channel();
        tx.send(SchedulerCommand::Nack {
            queue_id: "retry-queue".to_string(),
            msg_id,
            error: "transient error".to_string(),
            reply: nack_tx,
        })
        .unwrap();

        tx.send(SchedulerCommand::Shutdown).unwrap();
        scheduler.run();

        // Nack should succeed
        assert!(nack_rx.try_recv().unwrap().is_ok(), "nack should succeed");

        // Should receive the message twice: initial delivery + retry after nack
        let first = consumer_rx.try_recv().expect("first delivery");
        assert_eq!(first.msg_id, msg_id);
        assert_eq!(first.attempt_count, 0);

        let second = consumer_rx.try_recv().expect("second delivery after retry");
        assert_eq!(second.msg_id, msg_id);
        assert_eq!(second.attempt_count, 1);
    }

    #[test]
    fn on_failure_dlq_moves_message_to_dead_letter_queue() {
        let (tx, mut scheduler, _dir) = test_setup();

        // Create the DLQ queue first
        send_create_queue(&tx, "my-dlq");

        // Create the main queue with on_failure script pointing to the DLQ
        let on_failure_script = r#"
            function on_failure(msg)
                return { action = "dlq" }
            end
        "#;
        send_create_queue_with_on_failure(&tx, "main-queue", on_failure_script, Some("my-dlq"));

        // Register consumers for both queues
        let (main_tx, mut main_rx) = tokio::sync::mpsc::channel(64);
        tx.send(SchedulerCommand::RegisterConsumer {
            queue_id: "main-queue".to_string(),
            consumer_id: "main-consumer".to_string(),
            tx: main_tx,
        })
        .unwrap();

        let (dlq_tx, mut dlq_rx) = tokio::sync::mpsc::channel(64);
        tx.send(SchedulerCommand::RegisterConsumer {
            queue_id: "my-dlq".to_string(),
            consumer_id: "dlq-consumer".to_string(),
            tx: dlq_tx,
        })
        .unwrap();

        // Enqueue a message to the main queue
        let msg = test_message("main-queue");
        let msg_id = msg.id;
        let (reply_tx, _) = tokio::sync::oneshot::channel();
        tx.send(SchedulerCommand::Enqueue {
            message: msg,
            reply: reply_tx,
        })
        .unwrap();

        // Nack the message — on_failure returns dlq
        let (nack_tx, mut nack_rx) = tokio::sync::oneshot::channel();
        tx.send(SchedulerCommand::Nack {
            queue_id: "main-queue".to_string(),
            msg_id,
            error: "permanent error".to_string(),
            reply: nack_tx,
        })
        .unwrap();

        tx.send(SchedulerCommand::Shutdown).unwrap();
        scheduler.run();

        assert!(nack_rx.try_recv().unwrap().is_ok(), "nack should succeed");

        // First delivery on main queue (before nack)
        let first = main_rx.try_recv().expect("initial delivery on main queue");
        assert_eq!(first.msg_id, msg_id);

        // No second delivery on main queue (message was DLQ'd, not retried)
        assert!(
            main_rx.try_recv().is_err(),
            "message should not be retried on main queue"
        );

        // Message should appear on DLQ
        let dlq_msg = dlq_rx
            .try_recv()
            .expect("message should be delivered to DLQ");
        assert_eq!(dlq_msg.msg_id, msg_id);
        assert_eq!(
            dlq_msg.attempt_count, 1,
            "attempt_count should be incremented"
        );

        // Verify message is gone from main queue storage
        let main_prefix = crate::storage::keys::message_prefix("main-queue");
        let main_msgs = scheduler.storage().list_messages(&main_prefix).unwrap();
        assert!(
            main_msgs.is_empty(),
            "message should be removed from main queue"
        );

        // Verify message exists in DLQ storage
        let dlq_prefix = crate::storage::keys::message_prefix("my-dlq");
        let dlq_msgs = scheduler.storage().list_messages(&dlq_prefix).unwrap();
        assert_eq!(dlq_msgs.len(), 1, "message should exist in DLQ");
        assert_eq!(dlq_msgs[0].1.id, msg_id);
    }

    #[test]
    fn on_failure_dlq_without_dlq_configured_falls_back_to_retry() {
        let (tx, mut scheduler, _dir) = test_setup();

        // Create queue with on_failure that returns DLQ, but NO dlq_queue_id configured
        let on_failure_script = r#"
            function on_failure(msg)
                return { action = "dlq" }
            end
        "#;
        send_create_queue_with_on_failure(&tx, "no-dlq-queue", on_failure_script, None);

        let (consumer_tx, mut consumer_rx) = tokio::sync::mpsc::channel(64);
        tx.send(SchedulerCommand::RegisterConsumer {
            queue_id: "no-dlq-queue".to_string(),
            consumer_id: "c1".to_string(),
            tx: consumer_tx,
        })
        .unwrap();

        let msg = test_message("no-dlq-queue");
        let msg_id = msg.id;
        let (reply_tx, _) = tokio::sync::oneshot::channel();
        tx.send(SchedulerCommand::Enqueue {
            message: msg,
            reply: reply_tx,
        })
        .unwrap();

        // Nack — on_failure says DLQ but no DLQ configured, should retry instead
        let (nack_tx, mut nack_rx) = tokio::sync::oneshot::channel();
        tx.send(SchedulerCommand::Nack {
            queue_id: "no-dlq-queue".to_string(),
            msg_id,
            error: "some error".to_string(),
            reply: nack_tx,
        })
        .unwrap();

        tx.send(SchedulerCommand::Shutdown).unwrap();
        scheduler.run();

        assert!(nack_rx.try_recv().unwrap().is_ok());

        // Should receive message twice (initial + retry fallback)
        let first = consumer_rx.try_recv().expect("first delivery");
        assert_eq!(first.msg_id, msg_id);
        assert_eq!(first.attempt_count, 0);

        let second = consumer_rx.try_recv().expect("retry after DLQ fallback");
        assert_eq!(second.msg_id, msg_id);
        assert_eq!(second.attempt_count, 1);
    }

    #[test]
    fn on_failure_receives_attempt_count_and_error() {
        let (tx, mut scheduler, _dir) = test_setup();

        // Create DLQ queue
        send_create_queue(&tx, "dlq-for-attempts");

        // Script that DLQs only when attempts >= 3
        let on_failure_script = r#"
            function on_failure(msg)
                if msg.attempts >= 3 and msg.error == "fatal" then
                    return { action = "dlq" }
                end
                return { action = "retry" }
            end
        "#;
        send_create_queue_with_on_failure(
            &tx,
            "attempts-queue",
            on_failure_script,
            Some("dlq-for-attempts"),
        );

        let (consumer_tx, mut consumer_rx) = tokio::sync::mpsc::channel(64);
        tx.send(SchedulerCommand::RegisterConsumer {
            queue_id: "attempts-queue".to_string(),
            consumer_id: "c1".to_string(),
            tx: consumer_tx,
        })
        .unwrap();

        let (dlq_consumer_tx, mut dlq_consumer_rx) = tokio::sync::mpsc::channel(64);
        tx.send(SchedulerCommand::RegisterConsumer {
            queue_id: "dlq-for-attempts".to_string(),
            consumer_id: "dlq-c1".to_string(),
            tx: dlq_consumer_tx,
        })
        .unwrap();

        let msg = test_message("attempts-queue");
        let msg_id = msg.id;
        let (reply_tx, _) = tokio::sync::oneshot::channel();
        tx.send(SchedulerCommand::Enqueue {
            message: msg,
            reply: reply_tx,
        })
        .unwrap();

        // Nack 1: attempt_count becomes 1, error is "transient" → retry
        let (nack1_tx, _) = tokio::sync::oneshot::channel();
        tx.send(SchedulerCommand::Nack {
            queue_id: "attempts-queue".to_string(),
            msg_id,
            error: "transient".to_string(),
            reply: nack1_tx,
        })
        .unwrap();

        // Nack 2: attempt_count becomes 2, error is "fatal" → retry (attempts < 3)
        let (nack2_tx, _) = tokio::sync::oneshot::channel();
        tx.send(SchedulerCommand::Nack {
            queue_id: "attempts-queue".to_string(),
            msg_id,
            error: "fatal".to_string(),
            reply: nack2_tx,
        })
        .unwrap();

        // Nack 3: attempt_count becomes 3, error is "fatal" → DLQ (attempts >= 3 AND error == "fatal")
        let (nack3_tx, mut nack3_rx) = tokio::sync::oneshot::channel();
        tx.send(SchedulerCommand::Nack {
            queue_id: "attempts-queue".to_string(),
            msg_id,
            error: "fatal".to_string(),
            reply: nack3_tx,
        })
        .unwrap();

        tx.send(SchedulerCommand::Shutdown).unwrap();
        scheduler.run();

        assert!(nack3_rx.try_recv().unwrap().is_ok());

        // Main queue: initial delivery (0) + retry after nack 1 (1) + retry after nack 2 (2) = 3 deliveries
        let d0 = consumer_rx.try_recv().expect("delivery 0");
        assert_eq!(d0.attempt_count, 0);
        let d1 = consumer_rx.try_recv().expect("delivery 1");
        assert_eq!(d1.attempt_count, 1);
        let d2 = consumer_rx.try_recv().expect("delivery 2");
        assert_eq!(d2.attempt_count, 2);

        // No more deliveries on main queue (nack 3 sent to DLQ)
        assert!(
            consumer_rx.try_recv().is_err(),
            "no more deliveries after DLQ"
        );

        // DLQ should have the message with attempt_count = 3
        let dlq_msg = dlq_consumer_rx
            .try_recv()
            .expect("message should be in DLQ");
        assert_eq!(dlq_msg.msg_id, msg_id);
        assert_eq!(dlq_msg.attempt_count, 3);
    }

    #[test]
    fn on_failure_no_script_uses_default_retry() {
        // Backward compatibility: no on_failure script → default retry behavior
        let (tx, mut scheduler, _dir) = test_setup();

        send_create_queue(&tx, "no-script-queue");

        let (consumer_tx, mut consumer_rx) = tokio::sync::mpsc::channel(64);
        tx.send(SchedulerCommand::RegisterConsumer {
            queue_id: "no-script-queue".to_string(),
            consumer_id: "c1".to_string(),
            tx: consumer_tx,
        })
        .unwrap();

        let msg = test_message("no-script-queue");
        let msg_id = msg.id;
        let (reply_tx, _) = tokio::sync::oneshot::channel();
        tx.send(SchedulerCommand::Enqueue {
            message: msg,
            reply: reply_tx,
        })
        .unwrap();

        let (nack_tx, mut nack_rx) = tokio::sync::oneshot::channel();
        tx.send(SchedulerCommand::Nack {
            queue_id: "no-script-queue".to_string(),
            msg_id,
            error: "some error".to_string(),
            reply: nack_tx,
        })
        .unwrap();

        tx.send(SchedulerCommand::Shutdown).unwrap();
        scheduler.run();

        assert!(nack_rx.try_recv().unwrap().is_ok());

        // Should get 2 deliveries: initial + retry (default behavior)
        let first = consumer_rx.try_recv().expect("first delivery");
        assert_eq!(first.attempt_count, 0);
        let second = consumer_rx.try_recv().expect("second delivery after nack");
        assert_eq!(second.attempt_count, 1);
    }

    #[test]
    fn recovery_restores_on_failure_scripts() {
        let dir = tempfile::tempdir().unwrap();
        let storage: Arc<dyn Storage> = Arc::new(RocksDbStorage::open(dir.path()).unwrap());

        // Phase 1: create queue with on_failure script, enqueue a message, shut down
        let (tx, mut scheduler) = test_setup_with_storage(Arc::clone(&storage));

        let on_failure_script = r#"
            function on_failure(msg)
                return { action = "dlq" }
            end
        "#;
        send_create_queue(&tx, "recovery-dlq");
        send_create_queue_with_on_failure(
            &tx,
            "recovery-queue",
            on_failure_script,
            Some("recovery-dlq"),
        );

        let msg = test_message("recovery-queue");
        let msg_id = msg.id;
        let (reply_tx, _) = tokio::sync::oneshot::channel();
        tx.send(SchedulerCommand::Enqueue {
            message: msg,
            reply: reply_tx,
        })
        .unwrap();

        // Register consumer so message gets delivered and leased
        let (consumer_tx, mut consumer_rx) = tokio::sync::mpsc::channel(64);
        tx.send(SchedulerCommand::RegisterConsumer {
            queue_id: "recovery-queue".to_string(),
            consumer_id: "c1".to_string(),
            tx: consumer_tx,
        })
        .unwrap();

        tx.send(SchedulerCommand::Shutdown).unwrap();
        scheduler.run();
        drop(tx);

        // Consume the initial delivery
        let delivered = consumer_rx.try_recv().expect("initial delivery");
        assert_eq!(delivered.msg_id, msg_id);

        // Phase 2: start new scheduler on same storage — on_failure script should be recovered
        let (tx2, mut scheduler2) = test_setup_with_storage(Arc::clone(&storage));

        // Register consumers for both queues
        let (main_consumer_tx, _main_consumer_rx) = tokio::sync::mpsc::channel(64);
        tx2.send(SchedulerCommand::RegisterConsumer {
            queue_id: "recovery-queue".to_string(),
            consumer_id: "c2".to_string(),
            tx: main_consumer_tx,
        })
        .unwrap();

        let (dlq_consumer_tx, mut dlq_consumer_rx) = tokio::sync::mpsc::channel(64);
        tx2.send(SchedulerCommand::RegisterConsumer {
            queue_id: "recovery-dlq".to_string(),
            consumer_id: "dlq-c2".to_string(),
            tx: dlq_consumer_tx,
        })
        .unwrap();

        // Nack the message — should trigger on_failure script (recovered from storage)
        let (nack_tx, mut nack_rx) = tokio::sync::oneshot::channel();
        tx2.send(SchedulerCommand::Nack {
            queue_id: "recovery-queue".to_string(),
            msg_id,
            error: "error after recovery".to_string(),
            reply: nack_tx,
        })
        .unwrap();

        tx2.send(SchedulerCommand::Shutdown).unwrap();
        scheduler2.run();

        assert!(
            nack_rx.try_recv().unwrap().is_ok(),
            "nack should succeed after recovery"
        );

        // Message should be in DLQ (on_failure script was recovered and returned "dlq")
        let dlq_msg = dlq_consumer_rx
            .try_recv()
            .expect("on_failure script should be restored — message should be in DLQ");
        assert_eq!(dlq_msg.msg_id, msg_id);
    }
}
