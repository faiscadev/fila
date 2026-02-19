use super::*;

impl Scheduler {
    /// Find the full message key in the messages CF by scanning the queue prefix.
    pub(super) fn find_message_key(
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

    /// Remove all pending and leased index entries for a queue (used on queue deletion).
    pub(super) fn remove_pending_for_queue(&mut self, queue_id: &str) {
        // Remove pending (not-yet-delivered) entries
        let keys_to_remove: Vec<(String, String)> = self
            .pending
            .keys()
            .filter(|(qid, _)| qid == queue_id)
            .cloned()
            .collect();
        for key in keys_to_remove {
            if let Some(entries) = self.pending.remove(&key) {
                for entry in entries {
                    self.pending_by_id.remove(&entry.msg_id);
                }
            }
        }

        // Remove in-flight (leased) entries whose storage key belongs to this queue
        let prefix = crate::storage::keys::message_prefix(queue_id);
        self.leased_msg_keys
            .retain(|_, msg_key| !msg_key.starts_with(&prefix));
    }

    /// Add a message to the pending index.
    pub(super) fn pending_push(&mut self, queue_id: &str, fairness_key: &str, entry: PendingEntry) {
        let pk = (queue_id.to_string(), fairness_key.to_string());
        self.pending_by_id.insert(entry.msg_id, pk.clone());
        self.pending.entry(pk).or_default().push_back(entry);
    }

    /// Run one DRR delivery round across all queues that have active keys
    /// and registered consumers.
    pub(super) fn drr_deliver(&mut self) {
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
    ///
    /// Uses the in-memory pending index instead of scanning storage, giving
    /// O(1) per-message delivery instead of the previous O(n) scan.
    pub(super) fn drr_deliver_queue(&mut self, queue_id: &str) {
        if !self.drr.has_active_keys(queue_id) {
            return;
        }

        // Check there are consumers for this queue — no point running DRR without them
        let has_consumers = self.consumers.values().any(|e| e.queue_id == queue_id);
        if !has_consumers {
            return;
        }

        // Start a new round (refills deficits for all active keys)
        if self.drr.start_new_round(queue_id) {
            self.metrics.record_drr_round(queue_id);
        }

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
            self.metrics.record_drr_key_processed(queue_id);

            let pending_key = (queue_id.to_string(), fairness_key.clone());

            // Peek at the front pending entry; if none, this fairness key is drained
            let Some(front) = self
                .pending
                .get(&pending_key)
                .and_then(|deque| deque.front())
            else {
                self.drr.remove_key(queue_id, &fairness_key);
                self.pending.remove(&pending_key);
                continue;
            };
            let throttle_keys = front.throttle_keys.clone();

            // Throttle check (peek only — no tokens consumed yet): if ANY configured
            // key is exhausted, skip this fairness key entirely.
            if !self.throttle.peek_keys(&throttle_keys) {
                for tk in &throttle_keys {
                    self.metrics.record_throttle_decision(tk, "hit");
                }
                // Throttled — drain all remaining deficit so DRR moves to next key
                // in O(1) rather than iterating once per deficit unit.
                // The key stays in the active set for the next round.
                self.drr.drain_deficit(queue_id, &fairness_key);
                continue;
            }

            // Pop the front entry from pending (we just peeked it above, so both must exist)
            let Some(entry) = self
                .pending
                .get_mut(&pending_key)
                .and_then(|deque| deque.pop_front())
            else {
                continue;
            };
            self.pending_by_id.remove(&entry.msg_id);

            // Load the full message from storage
            let msg = match self.storage.get_message(&entry.msg_key) {
                Ok(Some(msg)) => msg,
                Ok(None) => {
                    warn!(%queue_id, msg_id = %entry.msg_id, "pending message not found in storage, skipping");
                    continue;
                }
                Err(e) => {
                    error!(%queue_id, msg_id = %entry.msg_id, error = %e, "failed to read pending message");
                    // Put the entry back so we don't lose it
                    self.pending_by_id.insert(entry.msg_id, pending_key.clone());
                    self.pending
                        .entry(pending_key)
                        .or_default()
                        .push_front(entry);
                    break;
                }
            };

            let lease_key = crate::storage::keys::lease_key(queue_id, &msg.id);
            if self.try_deliver_to_consumer(queue_id, &msg, &lease_key, visibility_timeout_ms) {
                self.metrics.record_lease(queue_id);
                self.metrics
                    .record_fairness_delivery(queue_id, &fairness_key);
                *self
                    .fairness_deliveries
                    .entry((queue_id.to_string(), fairness_key.clone()))
                    .or_default() += 1;
                self.drr.consume_deficit(queue_id, &fairness_key);
                // Consume throttle tokens only after successful delivery
                self.throttle.consume_keys(&throttle_keys);
                for tk in &throttle_keys {
                    self.metrics.record_throttle_decision(tk, "pass");
                }
                // Track the storage key for O(1) lookup on ack/nack
                self.leased_msg_keys.insert(entry.msg_id, entry.msg_key);
            } else {
                // Couldn't deliver (all consumers full/closed).
                // Put the entry back at the front of the pending queue.
                // No throttle tokens were consumed (peek-only check above).
                self.pending_by_id.insert(entry.msg_id, pending_key.clone());
                self.pending
                    .entry(pending_key)
                    .or_default()
                    .push_front(entry);
                // Break without consuming deficit — the consumer-full condition
                // applies to all keys equally.
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
}
