use super::*;

impl Scheduler {
    pub(super) fn handle_enqueue(
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
            let start = std::time::Instant::now();
            if let Some((result, outcome)) = lua_engine.run_on_enqueue(
                &message.queue_id,
                &message.headers,
                message.payload.len(),
                &message.queue_id,
            ) {
                // Record Lua metrics based on execution outcome
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
                    crate::lua::LuaExecOutcome::CircuitBreakerBypassed => {
                        // No duration or error recording — script was not executed
                    }
                }
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

        self.metrics.record_enqueue(&message.queue_id);

        // Register the fairness key in DRR active set so it participates in scheduling
        self.drr
            .add_key(&message.queue_id, &message.fairness_key, message.weight);

        // Add to pending index
        self.pending_push(
            &message.queue_id,
            &message.fairness_key,
            PendingEntry {
                msg_key: key,
                msg_id,
                throttle_keys: message.throttle_keys.clone(),
            },
        );

        Ok(msg_id)
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

    pub(super) fn handle_ack(
        &mut self,
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

        // Look up the message key from the leased index (O(1)), falling back to scan
        let msg_key = self
            .leased_msg_keys
            .remove(msg_id)
            .or_else(|| self.find_message_key(queue_id, msg_id).ok().flatten());

        // Atomically delete the message, lease, and lease expiry
        let mut ops = vec![
            WriteBatchOp::DeleteLease { key: lease_key },
            WriteBatchOp::DeleteLeaseExpiry { key: expiry_key },
        ];
        if let Some(key) = msg_key {
            ops.push(WriteBatchOp::DeleteMessage { key });
        }

        self.storage.write_batch(ops)?;
        self.metrics.record_ack(queue_id);
        Ok(())
    }

    pub(super) fn handle_nack(
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
                let msg_value =
                    serde_json::to_vec(&msg).map_err(crate::error::StorageError::from)?;

                let ops = vec![
                    WriteBatchOp::DeleteMessage { key: msg_key },
                    WriteBatchOp::PutMessage {
                        key: dlq_msg_key.clone(),
                        value: msg_value,
                    },
                    WriteBatchOp::DeleteLease { key: lease_key },
                    WriteBatchOp::DeleteLeaseExpiry { key: expiry_key },
                ];
                self.storage.write_batch(ops)?;

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
        let msg_value = serde_json::to_vec(&msg).map_err(crate::error::StorageError::from)?;

        let ops = vec![
            WriteBatchOp::PutMessage {
                key: msg_key.clone(),
                value: msg_value,
            },
            WriteBatchOp::DeleteLease { key: lease_key },
            WriteBatchOp::DeleteLeaseExpiry { key: expiry_key },
        ];
        self.storage.write_batch(ops)?;

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
