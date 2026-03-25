use super::*;

impl Scheduler {
    /// Record gauge metrics for queue depth and active leases.
    /// Iterates all known queues to ensure gauges are zeroed when queues become idle.
    pub(super) fn record_gauges(&mut self) {
        // Compute per-queue pending counts from the pending index
        let mut queue_depths: HashMap<&str, u64> = HashMap::new();
        for ((queue_id_spur, _), entries) in &self.pending {
            let queue_id_str = self.interner.resolve(queue_id_spur);
            *queue_depths.entry(queue_id_str).or_default() += entries.len() as u64;
        }

        // Compute per-queue lease counts from leased_msg_keys storage keys
        let mut queue_leases: HashMap<String, u64> = HashMap::new();
        for msg_key in self.leased_msg_keys.values() {
            if let Some(queue_id) = crate::storage::keys::extract_queue_id(msg_key) {
                *queue_leases.entry(queue_id).or_default() += 1;
            }
        }

        // Report gauges for ALL known queues, defaulting to 0 for idle ones
        for queue_id in &self.known_queues {
            let depth = queue_depths.get(queue_id.as_str()).copied().unwrap_or(0);
            let leases = queue_leases.get(queue_id).copied().unwrap_or(0);
            self.metrics.set_queue_depth(queue_id, depth);
            self.metrics.set_leases_active(queue_id, leases);

            // DRR active keys gauge + per-key deficit and weight gauges
            let queue_spur = self.intern(queue_id);
            let key_stats = self.drr.key_stats(queue_spur);
            self.metrics
                .set_drr_active_keys(queue_id, key_stats.len() as u64);
            for (key_spur, deficit, weight) in &key_stats {
                let key_str = self.resolve(*key_spur);
                self.metrics.set_drr_deficit(queue_id, key_str, *deficit);
                self.metrics
                    .set_drr_weight(queue_id, key_str, *weight as u64);
            }
        }

        // Compute and record fair share ratios from delivery tracking
        // Group deliveries by queue to compute per-queue totals
        let mut queue_totals: HashMap<Spur, u64> = HashMap::new();
        for ((queue_id_spur, _), count) in &self.fairness_deliveries {
            *queue_totals.entry(*queue_id_spur).or_default() += count;
        }

        // Collect entries to iterate (avoid borrow conflict with self.drr/self.metrics)
        let delivery_entries: Vec<(Spur, Spur, u64)> = self
            .fairness_deliveries
            .iter()
            .map(|((q, f), c)| (*q, *f, *c))
            .collect();

        for (queue_id_spur, fairness_key_spur, count) in &delivery_entries {
            let total = queue_totals.get(queue_id_spur).copied().unwrap_or(0);
            if total == 0 {
                continue;
            }

            let queue_id_str = self.resolve(*queue_id_spur);
            let fairness_key_str = self.resolve(*fairness_key_spur);

            let stats = self.drr.key_stats(*queue_id_spur);
            let total_weight: u32 = stats.iter().map(|(_, _, w)| *w).sum();
            let Some(key_weight) = stats
                .iter()
                .find(|(k, _, _)| *k == *fairness_key_spur)
                .map(|(_, _, w)| *w)
            else {
                // Key no longer in DRR stats — skip to avoid incorrect ratio
                continue;
            };

            if total_weight == 0 {
                continue;
            }

            let fair_share = key_weight as f64 / total_weight as f64;
            let actual_share = *count as f64 / total as f64;
            let ratio = actual_share / fair_share;

            self.metrics
                .set_fairness_ratio(queue_id_str, fairness_key_str, ratio);
        }

        // Reset delivery tracking for next reporting window
        self.fairness_deliveries.clear();

        // Report throttle token levels per key
        for (key, tokens, _, _) in self.throttle.key_stats() {
            self.metrics.set_throttle_tokens(&key, tokens);
        }
    }
}
