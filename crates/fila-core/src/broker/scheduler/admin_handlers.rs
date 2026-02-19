use super::*;

impl Scheduler {
    /// Throttle config key prefix. Keys in the state CF starting with this
    /// prefix are treated as throttle rate configurations.
    pub(super) const THROTTLE_PREFIX: &'static str = "throttle.";

    pub(super) fn handle_set_config(
        &mut self,
        key: &str,
        value: &str,
    ) -> Result<(), crate::error::ConfigError> {
        debug!(%key, %value, "set config");

        if value.is_empty() {
            // Delete the key
            self.storage.delete_state(key)?;

            // If throttle-prefixed, remove the rate
            if let Some(throttle_key) = key.strip_prefix(Self::THROTTLE_PREFIX) {
                if !throttle_key.is_empty() {
                    self.throttle.remove_rate(throttle_key);
                }
            }
        } else {
            // If throttle-prefixed, validate before persisting to avoid storing garbage
            if let Some(throttle_key) = key.strip_prefix(Self::THROTTLE_PREFIX) {
                if throttle_key.is_empty() {
                    return Err(crate::error::ConfigError::InvalidValue(
                        "throttle key name must not be empty".into(),
                    ));
                }
                let (rate, burst) = Self::parse_throttle_value(value)?;
                self.storage.put_state(key, value.as_bytes())?;
                self.throttle.set_rate(throttle_key, rate, burst);
            } else {
                self.storage.put_state(key, value.as_bytes())?;
            }
        }

        Ok(())
    }

    /// Maximum number of entries returned by a single ListConfig call.
    pub(super) const MAX_LIST_CONFIG_ENTRIES: usize = 10_000;

    pub(super) fn handle_list_config(
        &self,
        prefix: &str,
    ) -> Result<Vec<(String, String)>, crate::error::ConfigError> {
        let entries = self
            .storage
            .list_state_by_prefix(prefix, Self::MAX_LIST_CONFIG_ENTRIES)?;
        let count = entries.len();
        debug!(%prefix, %count, "list config");
        entries
            .into_iter()
            .map(|(k, v)| {
                let value_str = String::from_utf8(v).map_err(|_| {
                    crate::error::StorageError::CorruptData(format!(
                        "non-UTF8 config value for key {k}"
                    ))
                })?;
                Ok((k, value_str))
            })
            .collect()
    }

    pub(super) fn handle_get_config(
        &self,
        key: &str,
    ) -> Result<Option<String>, crate::error::ConfigError> {
        let value = self.storage.get_state(key)?;
        match value {
            Some(bytes) => {
                let s = String::from_utf8(bytes).map_err(|_| {
                    crate::error::StorageError::CorruptData(format!(
                        "non-UTF8 config value for key {key}"
                    ))
                })?;
                Ok(Some(s))
            }
            None => Ok(None),
        }
    }

    pub(super) fn handle_get_stats(
        &self,
        queue_id: &str,
    ) -> Result<super::super::stats::QueueStats, crate::error::StatsError> {
        use super::super::stats::{FairnessKeyStats, QueueStats, ThrottleKeyStats};

        // Verify queue exists
        if self.storage.get_queue(queue_id)?.is_none() {
            return Err(crate::error::StatsError::QueueNotFound(
                queue_id.to_string(),
            ));
        }

        // Count pending messages per fairness key
        let mut pending_total: u64 = 0;
        let mut pending_by_key: std::collections::HashMap<&str, u64> =
            std::collections::HashMap::new();
        for ((q, fk), entries) in &self.pending {
            if q == queue_id {
                let count = entries.len() as u64;
                pending_total += count;
                pending_by_key.insert(fk, count);
            }
        }

        // Count in-flight (leased) messages for this queue
        let queue_prefix = crate::storage::keys::message_prefix(queue_id);
        let in_flight = self
            .leased_msg_keys
            .values()
            .filter(|key| key.starts_with(&queue_prefix))
            .count() as u64;

        let depth = pending_total + in_flight;

        // Count active consumers for this queue
        let active_consumers = self
            .consumers
            .values()
            .filter(|c| c.queue_id == queue_id)
            .count();
        let active_consumers = u32::try_from(active_consumers).unwrap_or(u32::MAX);

        // Collect per-fairness-key stats from DRR
        let drr_stats = self.drr.key_stats(queue_id);
        let per_key_stats: Vec<FairnessKeyStats> = drr_stats
            .into_iter()
            .map(|(key, deficit, weight)| {
                let pending_count = pending_by_key.get(key.as_str()).copied().unwrap_or(0);
                FairnessKeyStats {
                    key,
                    pending_count,
                    current_deficit: deficit,
                    weight,
                }
            })
            .collect();

        let active_fairness_keys = per_key_stats.len() as u64;

        // Collect throttle stats (global, not per-queue)
        let per_throttle_stats: Vec<ThrottleKeyStats> = self
            .throttle
            .key_stats()
            .into_iter()
            .map(|(key, tokens, rate_per_second, burst)| ThrottleKeyStats {
                key,
                tokens,
                rate_per_second,
                burst,
            })
            .collect();

        let quantum = self.drr.quantum();

        debug!(
            %queue_id,
            %depth,
            %in_flight,
            %active_fairness_keys,
            %active_consumers,
            "get stats"
        );

        Ok(QueueStats {
            depth,
            in_flight,
            active_fairness_keys,
            active_consumers,
            quantum,
            per_key_stats,
            per_throttle_stats,
        })
    }

    pub(super) fn handle_redrive(
        &mut self,
        dlq_queue_id: &str,
        count: u64,
    ) -> Result<u64, crate::error::RedriveError> {
        // Verify DLQ queue exists
        if self.storage.get_queue(dlq_queue_id)?.is_none() {
            return Err(crate::error::RedriveError::QueueNotFound(
                dlq_queue_id.to_string(),
            ));
        }

        // Verify it's actually a DLQ (name ends with .dlq)
        let Some(parent_queue_id) = dlq_queue_id.strip_suffix(".dlq") else {
            return Err(crate::error::RedriveError::NotADLQ(
                dlq_queue_id.to_string(),
            ));
        };

        // Verify parent queue exists
        if self.storage.get_queue(parent_queue_id)?.is_none() {
            return Err(crate::error::RedriveError::ParentQueueNotFound(
                parent_queue_id.to_string(),
            ));
        }

        // Enumerate DLQ messages from storage (ordered by enqueue time)
        let dlq_prefix = crate::storage::keys::message_prefix(dlq_queue_id);
        let messages = self.storage.list_messages(&dlq_prefix)?;

        let limit = if count == 0 { u64::MAX } else { count };
        let mut redriven: u64 = 0;

        for (dlq_key, mut msg) in messages {
            if redriven >= limit {
                break;
            }

            // Skip messages that are currently leased (in-flight in DLQ)
            if self.leased_msg_keys.contains_key(&msg.id) {
                continue;
            }

            // Move message to parent queue: reset attempt_count, set queue_id
            msg.queue_id = parent_queue_id.to_string();
            msg.attempt_count = 0;
            msg.leased_at = None;

            // Generate new storage key for parent queue
            let parent_key = crate::storage::keys::message_key(
                parent_queue_id,
                &msg.fairness_key,
                msg.enqueued_at,
                &msg.id,
            );
            let msg_value = match serde_json::to_vec(&msg) {
                Ok(v) => v,
                Err(e) => {
                    warn!(error = %e, msg_id = %msg.id, "serialization failed during redrive, stopping");
                    break;
                }
            };

            // Atomic move: delete from DLQ, put in parent queue
            let ops = vec![
                WriteBatchOp::DeleteMessage { key: dlq_key },
                WriteBatchOp::PutMessage {
                    key: parent_key.clone(),
                    value: msg_value,
                },
            ];
            if let Err(e) = self.storage.write_batch(ops) {
                warn!(error = %e, msg_id = %msg.id, "write_batch failed during redrive, returning partial count");
                break;
            }

            // Remove from DLQ's in-memory pending index if present
            if let Some(pk) = self.pending_by_id.remove(&msg.id) {
                if let Some(deque) = self.pending.get_mut(&pk) {
                    deque.retain(|e| e.msg_id != msg.id);
                    if deque.is_empty() {
                        self.pending.remove(&pk);
                        // Clean up DRR active set for DLQ if no more pending
                        self.drr.remove_key(&pk.0, &pk.1);
                    }
                }
            }

            // Add to parent queue's in-memory indices
            self.drr
                .add_key(parent_queue_id, &msg.fairness_key, msg.weight);
            self.pending_push(
                parent_queue_id,
                &msg.fairness_key,
                PendingEntry {
                    msg_key: parent_key,
                    msg_id: msg.id,
                    throttle_keys: msg.throttle_keys.clone(),
                },
            );

            redriven += 1;
        }

        // Trigger delivery for parent queue if any messages were moved
        if redriven > 0 {
            self.drr_deliver_queue(parent_queue_id);
        }

        debug!(
            %dlq_queue_id,
            %parent_queue_id,
            %redriven,
            "redrive complete"
        );

        Ok(redriven)
    }

    pub(super) fn handle_list_queues(
        &self,
    ) -> Result<Vec<super::super::command::QueueSummary>, crate::error::ListQueuesError> {
        let queues = self.storage.list_queues()?;

        // Pre-compute per-queue pending counts in a single pass over the pending map
        let mut pending_by_queue: HashMap<&str, u64> = HashMap::new();
        for ((qid, _), entries) in &self.pending {
            *pending_by_queue.entry(qid.as_str()).or_default() += entries.len() as u64;
        }

        // Pre-compute per-queue in-flight counts in a single pass over leased keys
        let mut in_flight_by_queue: HashMap<String, u64> = HashMap::new();
        for key in self.leased_msg_keys.values() {
            // Extract queue_id from the message key prefix (length-prefixed encoding)
            if let Some(queue_id) = crate::storage::keys::extract_queue_id(key) {
                *in_flight_by_queue.entry(queue_id).or_default() += 1;
            }
        }

        // Pre-compute per-queue consumer counts in a single pass
        let mut consumers_by_queue: HashMap<&str, u32> = HashMap::new();
        for c in self.consumers.values() {
            *consumers_by_queue.entry(c.queue_id.as_str()).or_default() += 1;
        }

        let mut summaries = Vec::with_capacity(queues.len());
        for q in queues {
            let pending = pending_by_queue.get(q.name.as_str()).copied().unwrap_or(0);
            let in_flight = in_flight_by_queue.get(&q.name).copied().unwrap_or(0);
            let active_consumers = consumers_by_queue
                .get(q.name.as_str())
                .copied()
                .unwrap_or(0);

            summaries.push(super::super::command::QueueSummary {
                name: q.name,
                depth: pending + in_flight,
                in_flight,
                active_consumers,
            });
        }

        Ok(summaries)
    }

    /// Parse a throttle config value in the format "rate_per_second,burst".
    pub(super) fn parse_throttle_value(
        value: &str,
    ) -> Result<(f64, f64), crate::error::ConfigError> {
        let parts: Vec<&str> = value.split(',').collect();
        if parts.len() != 2 {
            return Err(crate::error::ConfigError::InvalidValue(format!(
                "throttle value must be 'rate_per_second,burst', got: {value}"
            )));
        }
        let rate: f64 = parts[0].trim().parse().map_err(|_| {
            crate::error::ConfigError::InvalidValue(format!(
                "invalid rate_per_second: {}",
                parts[0]
            ))
        })?;
        if !rate.is_finite() || rate < 0.0 {
            return Err(crate::error::ConfigError::InvalidValue(format!(
                "rate_per_second must be a finite non-negative number, got: {}",
                parts[0].trim()
            )));
        }
        let burst: f64 = parts[1].trim().parse().map_err(|_| {
            crate::error::ConfigError::InvalidValue(format!("invalid burst: {}", parts[1]))
        })?;
        if !burst.is_finite() || burst < 0.0 {
            return Err(crate::error::ConfigError::InvalidValue(format!(
                "burst must be a finite non-negative number, got: {}",
                parts[1].trim()
            )));
        }
        Ok((rate, burst))
    }
}
