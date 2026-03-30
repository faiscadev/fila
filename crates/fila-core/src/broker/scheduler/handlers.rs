use super::*;
use crate::broker::command::{AckItem, NackItem};
use prost::Message as ProstMessage;

/// Data for a validated enqueue that passed validation and Lua processing.
struct PreparedEnqueue {
    msg_id: uuid::Uuid,
    queue_id: String,
    fairness_key: String,
    weight: u32,
    throttle_keys: Vec<String>,
    key: Vec<u8>,
    value: Vec<u8>,
}

impl Scheduler {
    /// Process enqueue operations. Validates each message individually,
    /// then writes all valid messages in a single RocksDB WriteBatch
    /// via `apply_mutations()`.
    ///
    /// Returns (per-item results, set of queue_ids that had successful
    /// enqueues for delivery).
    pub(super) fn handle_enqueue(
        &mut self,
        messages: Vec<crate::message::Message>,
    ) -> (
        Vec<Result<uuid::Uuid, crate::error::EnqueueError>>,
        HashSet<String>,
    ) {
        let len = messages.len();
        // Phase 1: Validate each message and prepare mutations.
        // Results are either Ok(PreparedEnqueue) or Err(EnqueueError).
        let mut prepared: Vec<Result<PreparedEnqueue, crate::error::EnqueueError>> =
            Vec::with_capacity(len);

        for message in messages {
            prepared.push(self.prepare_enqueue(message));
        }

        // Phase 2: Accumulate mutations for all successful items and write in
        // a single RocksDB WriteBatch.
        let mut mutations = Vec::new();
        for item in &prepared {
            if let Ok(prep) = item {
                mutations.push(Mutation::PutMessage {
                    key: prep.key.clone(),
                    value: prep.value.clone(),
                });
            }
        }

        // If there are mutations, write them all in one atomic batch.
        let storage_err = if mutations.is_empty() {
            None
        } else {
            self.storage.apply_mutations(mutations).err()
        };

        // Phase 3: Build results and update in-memory state.
        let mut results = Vec::with_capacity(len);
        let mut queues_to_deliver = HashSet::new();

        for item in prepared {
            match item {
                Ok(prep) => {
                    if let Some(ref err) = storage_err {
                        // WriteBatch failed — all items in the batch fail with storage error.
                        results.push(Err(crate::error::EnqueueError::Storage(
                            crate::error::StorageError::Engine(err.to_string()),
                        )));
                    } else {
                        self.metrics.record_enqueue(&prep.queue_id);
                        self.drr
                            .add_key(&prep.queue_id, &prep.fairness_key, prep.weight);
                        self.pending_push(
                            &prep.queue_id,
                            &prep.fairness_key,
                            PendingEntry {
                                msg_key: prep.key,
                                msg_id: prep.msg_id,
                                throttle_keys: prep.throttle_keys,
                            },
                        );
                        queues_to_deliver.insert(prep.queue_id);
                        results.push(Ok(prep.msg_id));
                    }
                }
                Err(e) => {
                    results.push(Err(e));
                }
            }
        }

        (results, queues_to_deliver)
    }

    /// Validate a message and prepare it for storage. Returns the data
    /// needed to create a `Mutation::PutMessage` without actually writing.
    fn prepare_enqueue(
        &mut self,
        mut message: crate::message::Message,
    ) -> Result<PreparedEnqueue, crate::error::EnqueueError> {
        // Verify queue exists
        if self.storage.get_queue(&message.queue_id)?.is_none() {
            return Err(crate::error::EnqueueError::QueueNotFound(
                message.queue_id.clone(),
            ));
        }

        // Run on_enqueue Lua script if one is cached for this queue
        if let Some(ref mut lua_engine) = self.lua_engine {
            let start = std::time::Instant::now();
            if let Some((result, outcome)) = lua_engine.run_on_enqueue(
                &message.queue_id,
                &message.headers,
                message.payload.len(),
                &message.queue_id,
            ) {
                match outcome {
                    crate::lua::LuaExecOutcome::Success => {
                        let duration_us = start.elapsed().as_micros() as f64;
                        self.metrics.record_lua_duration(
                            &message.queue_id,
                            "on_enqueue",
                            duration_us,
                        );
                    }
                    crate::lua::LuaExecOutcome::ScriptError {
                        circuit_breaker_tripped,
                    } => {
                        let duration_us = start.elapsed().as_micros() as f64;
                        self.metrics.record_lua_duration(
                            &message.queue_id,
                            "on_enqueue",
                            duration_us,
                        );
                        self.metrics.record_lua_error(&message.queue_id);
                        if circuit_breaker_tripped {
                            self.metrics.record_lua_cb_activation(&message.queue_id);
                        }
                    }
                    crate::lua::LuaExecOutcome::CircuitBreakerBypassed => {}
                }
                message.fairness_key = result.fairness_key;
                message.weight = result.weight;
                message.throttle_keys = result.throttle_keys;
            }
        }

        let msg_id = message.id;

        let group = self.router.route(&message.queue_id, &message.fairness_key);
        debug!(
            queue_id = %message.queue_id,
            fairness_key = %message.fairness_key,
            group_id = %group.0,
            "routed enqueue"
        );

        let key = crate::storage::keys::message_key(
            &message.queue_id,
            &message.fairness_key,
            message.enqueued_at,
            &msg_id,
        );

        // Clone small fields for PreparedEnqueue before consuming message
        // into proto serialization (avoids cloning the heavy payload/headers).
        let queue_id = message.queue_id.clone();
        let fairness_key = message.fairness_key.clone();
        let weight = message.weight;
        let throttle_keys = message.throttle_keys.clone();

        let proto = fila_proto::Message::from(message);
        let value = proto.encode_to_vec();

        Ok(PreparedEnqueue {
            msg_id,
            queue_id,
            fairness_key,
            weight,
            throttle_keys,
            key,
            value,
        })
    }

    pub(super) fn handle_create_queue(
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

        // Auto-create a dead-letter queue unless this queue is itself a DLQ.
        // Persist the parent queue first so a crash between the two writes
        // leaves the parent without a DLQ reference (safe — nack falls back to retry).
        if !name.ends_with(".dlq") {
            let dlq_name = format!("{name}.dlq");
            config.dlq_queue_id = Some(dlq_name.clone());
            self.storage.put_queue(&name, &config)?;

            // Only create if it doesn't already exist (idempotent)
            if self.storage.get_queue(&dlq_name)?.is_none() {
                let dlq_config = crate::queue::QueueConfig::new(dlq_name.clone());
                self.storage.put_queue(&dlq_name, &dlq_config)?;
                self.known_queues.insert(dlq_name.clone());
                debug!(queue = %name, dlq = %dlq_name, "auto-created dead-letter queue");
            }
        } else {
            // DLQ queues don't get their own DLQ — clear any caller-provided value
            config.dlq_queue_id = None;
            self.storage.put_queue(&name, &config)?;
        }

        self.known_queues.insert(name.clone());
        Ok(name)
    }

    pub(super) fn handle_delete_queue(
        &mut self,
        queue_id: &str,
    ) -> Result<(), crate::error::DeleteQueueError> {
        // Check-then-delete is safe: same single-threaded guarantee as above.
        // RocksDB delete is a no-op on missing keys, so the explicit check is
        // the only way to return a meaningful NotFound error.
        // TODO(cluster): same as handle_create_queue — needs atomic operation.
        let queue_config = self
            .storage
            .get_queue(queue_id)?
            .ok_or_else(|| crate::error::DeleteQueueError::QueueNotFound(queue_id.to_string()))?;

        self.storage.delete_queue(queue_id)?;
        self.drr.remove_queue(queue_id);
        self.consumer_rr_idx.remove(queue_id);
        self.remove_pending_for_queue(queue_id);
        self.known_queues.remove(queue_id);
        if let Some(ref mut lua_engine) = self.lua_engine {
            lua_engine.remove_queue_scripts(queue_id);
        }

        // Only cascade-delete auto-created DLQs (matching {queue_id}.dlq convention).
        // Custom/shared DLQs configured via dlq_queue_id are left untouched.
        let auto_dlq_name = format!("{queue_id}.dlq");
        if queue_config.dlq_queue_id.as_deref() == Some(auto_dlq_name.as_str())
            && self.storage.get_queue(&auto_dlq_name)?.is_some()
        {
            self.storage.delete_queue(&auto_dlq_name)?;
            self.drr.remove_queue(&auto_dlq_name);
            self.consumer_rr_idx.remove(&auto_dlq_name);
            self.remove_pending_for_queue(&auto_dlq_name);
            self.known_queues.remove(&auto_dlq_name);
            if let Some(ref mut lua_engine) = self.lua_engine {
                lua_engine.remove_queue_scripts(&auto_dlq_name);
            }
            debug!(queue = %queue_id, dlq = %auto_dlq_name, "deleted auto-created dead-letter queue");
        }

        Ok(())
    }

    /// Process ack operations. Validates each item individually, then
    /// deletes all leases/messages in a single RocksDB WriteBatch.
    pub(super) fn handle_ack(
        &mut self,
        items: &[AckItem],
    ) -> Vec<Result<(), crate::error::AckError>> {
        // Phase 1: Validate each item and collect mutations.
        let mut per_item: Vec<Result<Vec<Mutation>, crate::error::AckError>> =
            Vec::with_capacity(items.len());
        let mut acked_queues: Vec<String> = Vec::new();

        for item in items {
            match self.prepare_ack(&item.queue_id, &item.msg_id) {
                Ok(ops) => {
                    acked_queues.push(item.queue_id.clone());
                    per_item.push(Ok(ops));
                }
                Err(e) => {
                    per_item.push(Err(e));
                }
            }
        }

        // Phase 2: Accumulate all mutations and write in one batch.
        let all_mutations: Vec<Mutation> = per_item
            .iter()
            .filter_map(|r| r.as_ref().ok())
            .flat_map(|ops| ops.iter())
            .map(|m| match m {
                Mutation::DeleteLease { key } => Mutation::DeleteLease { key: key.clone() },
                Mutation::DeleteLeaseExpiry { key } => {
                    Mutation::DeleteLeaseExpiry { key: key.clone() }
                }
                Mutation::DeleteMessage { key } => Mutation::DeleteMessage { key: key.clone() },
                other => panic!("unexpected mutation type in ack: {other:?}"),
            })
            .collect();

        let storage_err = if all_mutations.is_empty() {
            None
        } else {
            self.storage.apply_mutations(all_mutations).err()
        };

        // Phase 3: Build final results.
        let mut idx = 0;
        per_item
            .into_iter()
            .map(|r| match r {
                Ok(_) => {
                    if let Some(ref err) = storage_err {
                        Err(crate::error::AckError::Storage(
                            crate::error::StorageError::Engine(err.to_string()),
                        ))
                    } else {
                        self.metrics.record_ack(&acked_queues[idx]);
                        idx += 1;
                        Ok(())
                    }
                }
                Err(e) => Err(e),
            })
            .collect()
    }

    /// Validate a single ack and return the mutations needed (without applying them).
    fn prepare_ack(
        &mut self,
        queue_id: &str,
        msg_id: &uuid::Uuid,
    ) -> Result<Vec<Mutation>, crate::error::AckError> {
        let lease_key = crate::storage::keys::lease_key(queue_id, msg_id);
        let lease_value = self.storage.get_lease(&lease_key)?.ok_or_else(|| {
            crate::error::AckError::MessageNotFound(format!(
                "no lease for message {msg_id} in queue {queue_id}"
            ))
        })?;

        let expiry_ns = crate::storage::keys::parse_expiry_from_lease_value(&lease_value)
            .ok_or_else(|| {
                crate::error::AckError::Storage(crate::error::StorageError::CorruptData(format!(
                    "lease value: cannot parse expiry for message {msg_id} in queue {queue_id}"
                )))
            })?;
        let expiry_key = crate::storage::keys::lease_expiry_key(expiry_ns, queue_id, msg_id);

        let msg_key = self
            .leased_msg_keys
            .remove(msg_id)
            .or_else(|| self.find_message_key(queue_id, msg_id).ok().flatten());

        let mut ops = vec![
            Mutation::DeleteLease { key: lease_key },
            Mutation::DeleteLeaseExpiry { key: expiry_key },
        ];
        if let Some(key) = msg_key {
            ops.push(Mutation::DeleteMessage { key });
        }

        Ok(ops)
    }

    /// Process nack operations. Each item gets its own Result.
    /// Returns (results, set of queue_ids that had successful nacks for delivery).
    pub(super) fn handle_nack(
        &mut self,
        items: &[NackItem],
    ) -> (Vec<Result<(), crate::error::NackError>>, HashSet<String>) {
        let mut results = Vec::with_capacity(items.len());
        let mut queues_to_deliver = HashSet::new();

        for item in items {
            let result = self.apply_nack(&item.queue_id, &item.msg_id, &item.error);
            if result.is_ok() {
                queues_to_deliver.insert(item.queue_id.clone());
            }
            results.push(result);
        }

        (results, queues_to_deliver)
    }

    fn apply_nack(
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

        // Look up the message key from leased index (O(1)), falling back to scan
        let msg_key = self
            .leased_msg_keys
            .remove(msg_id)
            .or_else(|| self.find_message_key(queue_id, msg_id).ok().flatten())
            .ok_or_else(|| {
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
        self.metrics.record_nack(queue_id);

        // Run on_failure Lua script to decide retry vs dead-letter
        let start = std::time::Instant::now();
        let on_failure_result = self.lua_engine.as_mut().and_then(|engine| {
            engine.run_on_failure(
                queue_id,
                &msg.headers,
                &msg_id.to_string(),
                msg.attempt_count,
                queue_id,
                error,
            )
        });
        let failure_result = on_failure_result.map(|(result, outcome)| {
            match outcome {
                crate::lua::LuaExecOutcome::Success => {
                    let duration_us = start.elapsed().as_micros() as f64;
                    self.metrics
                        .record_lua_duration(queue_id, "on_failure", duration_us);
                }
                crate::lua::LuaExecOutcome::ScriptError {
                    circuit_breaker_tripped,
                } => {
                    let duration_us = start.elapsed().as_micros() as f64;
                    self.metrics
                        .record_lua_duration(queue_id, "on_failure", duration_us);
                    self.metrics.record_lua_error(queue_id);
                    if circuit_breaker_tripped {
                        self.metrics.record_lua_cb_activation(queue_id);
                    }
                }
                crate::lua::LuaExecOutcome::CircuitBreakerBypassed => {}
            }
            result
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
                let proto = fila_proto::Message::from(msg.clone());
                let msg_value = prost::Message::encode_to_vec(&proto);

                let ops = vec![
                    Mutation::DeleteMessage { key: msg_key },
                    Mutation::PutMessage {
                        key: dlq_msg_key.clone(),
                        value: msg_value,
                    },
                    Mutation::DeleteLease { key: lease_key },
                    Mutation::DeleteLeaseExpiry { key: expiry_key },
                ];
                self.storage.apply_mutations(ops)?;

                // Add the message to the DLQ's DRR active set for delivery
                self.drr
                    .add_key(&dlq_queue_id, &msg.fairness_key, msg.weight);

                // Add to DLQ's pending index
                self.pending_push(
                    &dlq_queue_id,
                    &msg.fairness_key,
                    PendingEntry {
                        msg_key: dlq_msg_key,
                        msg_id: msg.id,
                        throttle_keys: msg.throttle_keys.clone(),
                    },
                );

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
        let proto = fila_proto::Message::from(msg.clone());
        let msg_value = prost::Message::encode_to_vec(&proto);

        let ops = vec![
            Mutation::PutMessage {
                key: msg_key.clone(),
                value: msg_value,
            },
            Mutation::DeleteLease { key: lease_key },
            Mutation::DeleteLeaseExpiry { key: expiry_key },
        ];
        self.storage.apply_mutations(ops)?;

        // Re-add the fairness key to DRR active set so the message can be scheduled
        self.drr.add_key(queue_id, &msg.fairness_key, msg.weight);

        // Re-add to pending index (message is back in ready pool)
        self.pending_push(
            queue_id,
            &msg.fairness_key,
            PendingEntry {
                msg_key,
                msg_id: msg.id,
                throttle_keys: msg.throttle_keys.clone(),
            },
        );

        debug!(%queue_id, %msg_id, %error, attempt_count = msg.attempt_count, "nack processed");
        Ok(())
    }
}
