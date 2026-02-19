use super::*;

impl Scheduler {
    /// Scan the `lease_expiry` CF for expired leases and reclaim them.
    ///
    /// For each expired lease:
    /// 1. Delete the lease and lease_expiry entries
    /// 2. Increment the message's attempt_count and clear leased_at
    /// 3. Re-add the fairness key to the DRR active set
    ///
    /// Returns the number of leases reclaimed.
    pub(super) fn reclaim_expired_leases(&mut self) -> u64 {
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

            // Find the message key from leased index (O(1)), falling back to scan
            let msg_key = match self
                .leased_msg_keys
                .remove(&msg_id)
                .map(|k| Ok(Some(k)))
                .unwrap_or_else(|| self.find_message_key(&queue_id, &msg_id))
            {
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
                    key: msg_key.clone(),
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

            // Re-add to pending index (message is back in ready pool)
            self.pending_push(
                &queue_id,
                &updated_msg.fairness_key,
                PendingEntry {
                    msg_key,
                    msg_id,
                    throttle_keys: updated_msg.throttle_keys.clone(),
                },
            );

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
    pub(super) fn recover(&mut self) {
        self.reclaim_expired_leases();

        // Clear in-memory indexes populated by reclaim_expired_leases —
        // the full scan below rebuilds them from scratch, avoiding duplicates.
        self.pending.clear();
        self.pending_by_id.clear();
        self.leased_msg_keys.clear();

        // Rebuild DRR active keys, known queues, and Lua script cache by scanning queues
        match self.storage.list_queues() {
            Ok(queues) => {
                for queue in &queues {
                    self.known_queues.insert(queue.name.clone());
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

                    // Rebuild DRR active keys and pending index by scanning messages
                    let prefix = crate::storage::keys::message_prefix(&queue.name);
                    match self.storage.list_messages(&prefix) {
                        Ok(messages) => {
                            for (key, msg) in messages {
                                self.drr.add_key(&queue.name, &msg.fairness_key, msg.weight);

                                // Only add unleased messages to pending index
                                let lease_key =
                                    crate::storage::keys::lease_key(&queue.name, &msg.id);
                                if self.storage.get_lease(&lease_key).ok().flatten().is_some() {
                                    // Message is leased (in-flight) — track in leased_msg_keys
                                    self.leased_msg_keys.insert(msg.id, key);
                                } else {
                                    // Message is pending (available for delivery)
                                    self.pending_push(
                                        &queue.name,
                                        &msg.fairness_key,
                                        PendingEntry {
                                            msg_key: key,
                                            msg_id: msg.id,
                                            throttle_keys: msg.throttle_keys.clone(),
                                        },
                                    );
                                }
                            }
                        }
                        Err(e) => {
                            warn!(queue = %queue.name, error = %e, "failed to scan messages during DRR recovery");
                        }
                    }
                }
                info!(
                    queue_count = queues.len(),
                    "recovery: queues and messages restored"
                );
            }
            Err(e) => warn!(error = %e, "failed to list queues during recovery"),
        }

        // Restore throttle rates from state CF
        match self
            .storage
            .list_state_by_prefix(Self::THROTTLE_PREFIX, usize::MAX)
        {
            Ok(entries) => {
                let mut restored = 0;
                for (key, value) in &entries {
                    if let Some(throttle_key) = key.strip_prefix(Self::THROTTLE_PREFIX) {
                        match std::str::from_utf8(value) {
                            Ok(value_str) => match Self::parse_throttle_value(value_str) {
                                Ok((rate, burst)) => {
                                    self.throttle.set_rate(throttle_key, rate, burst);
                                    restored += 1;
                                }
                                Err(e) => {
                                    warn!(key = %key, error = %e, "skipping corrupt throttle config during recovery");
                                }
                            },
                            Err(e) => {
                                warn!(key = %key, error = %e, "skipping non-UTF8 throttle config during recovery");
                            }
                        }
                    }
                }
                info!(count = restored, "recovery: throttle rates restored");
            }
            Err(e) => warn!(error = %e, "failed to scan throttle configs during recovery"),
        }
    }
}
