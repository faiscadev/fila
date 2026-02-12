use std::collections::{HashMap, HashSet, VecDeque};

/// Per-queue DRR scheduling state.
#[derive(Default)]
struct DrrQueueState {
    /// Active fairness keys in round-robin order.
    active_keys: VecDeque<String>,
    /// Set for O(1) membership checks.
    active_set: HashSet<String>,
    /// Remaining deficit per fairness key. A key can deliver messages
    /// as long as its deficit is positive.
    deficits: HashMap<String, i64>,
    /// Weight per fairness key (default 1).
    weights: HashMap<String, u32>,
    /// Current position in the round-robin traversal.
    round_position: usize,
    /// Whether a round is currently active (deficits have been allocated
    /// but not yet exhausted). Prevents `start_new_round` from being
    /// called multiple times before the current round finishes.
    round_active: bool,
}

/// Deficit Round Robin scheduler. Manages per-queue fairness state.
///
/// Each queue maintains an independent active key set, deficit counters,
/// and round position. The scheduler is intended to run on a single thread
/// — no internal synchronization.
pub struct DrrScheduler {
    queues: HashMap<String, DrrQueueState>,
    quantum: u32,
}

impl DrrScheduler {
    pub fn new(quantum: u32) -> Self {
        assert!(quantum > 0, "DRR quantum must be > 0");
        Self {
            queues: HashMap::new(),
            quantum,
        }
    }

    /// Add a fairness key to the active set for a queue. If the key is already
    /// active, this is a no-op (deficit is not reset). Weight is updated if
    /// different from the current value.
    pub fn add_key(&mut self, queue_id: &str, fairness_key: &str, weight: u32) {
        let state = self.queues.entry(queue_id.to_string()).or_default();

        let w = weight.max(1); // weight must be at least 1
        state.weights.insert(fairness_key.to_string(), w);

        if !state.active_set.contains(fairness_key) {
            state.active_set.insert(fairness_key.to_string());
            state.active_keys.push_back(fairness_key.to_string());
            // New keys start with zero deficit — they receive allocation on the
            // next round start.
            state.deficits.entry(fairness_key.to_string()).or_insert(0);
        }
    }

    /// Remove a fairness key from the active set (e.g., when it has no more
    /// pending messages).
    pub fn remove_key(&mut self, queue_id: &str, fairness_key: &str) {
        let Some(state) = self.queues.get_mut(queue_id) else {
            return;
        };

        if state.active_set.remove(fairness_key) {
            // Find and remove by index so we can adjust round_position correctly
            if let Some(removed_idx) = state.active_keys.iter().position(|k| k == fairness_key) {
                state.active_keys.remove(removed_idx);
                state.deficits.remove(fairness_key);
                state.weights.remove(fairness_key);

                if state.active_keys.is_empty() {
                    state.round_position = 0;
                    state.round_active = false;
                } else if removed_idx < state.round_position {
                    // Key was before current position — shift back to avoid skipping
                    state.round_position -= 1;
                } else if state.round_position >= state.active_keys.len() {
                    // Position overshot the end — wrap to start
                    state.round_position = 0;
                }
            }
        }
    }

    /// Return the next fairness key that has positive deficit in the current
    /// round. Advances the round position. Returns `None` when the round is
    /// exhausted (all remaining keys have non-positive deficit), and marks
    /// the round as inactive so a new round can be started.
    pub fn next_key(&mut self, queue_id: &str) -> Option<String> {
        let state = self.queues.get_mut(queue_id)?;
        let len = state.active_keys.len();
        if len == 0 {
            return None;
        }

        let start = state.round_position;
        for i in 0..len {
            let idx = (start + i) % len;
            let key = &state.active_keys[idx];
            let deficit = state.deficits.get(key).copied().unwrap_or(0);
            if deficit > 0 {
                state.round_position = (idx + 1) % len;
                return Some(key.clone());
            }
        }

        // Round exhausted — all keys have non-positive deficit
        state.round_active = false;
        None
    }

    /// Decrement the deficit of a fairness key by 1 after delivering a message.
    pub fn consume_deficit(&mut self, queue_id: &str, fairness_key: &str) {
        if let Some(state) = self.queues.get_mut(queue_id) {
            if let Some(d) = state.deficits.get_mut(fairness_key) {
                *d -= 1;
            }
        }
    }

    /// Start a new DRR round for a queue: refill deficit for all active keys
    /// based on `weight * quantum`. No-op if a round is already active
    /// (prevents unbounded deficit accumulation from repeated calls).
    pub fn start_new_round(&mut self, queue_id: &str) {
        let Some(state) = self.queues.get_mut(queue_id) else {
            return;
        };

        if state.round_active {
            return;
        }

        for key in &state.active_keys {
            let weight = state.weights.get(key).copied().unwrap_or(1);
            let deficit = state.deficits.entry(key.clone()).or_insert(0);
            *deficit += (weight as i64) * (self.quantum as i64);
        }

        state.round_position = 0;
        state.round_active = true;
    }

    /// Remove all DRR state for a queue (e.g., when the queue is deleted).
    pub fn remove_queue(&mut self, queue_id: &str) {
        self.queues.remove(queue_id);
    }

    /// Check whether a queue has any active fairness keys.
    pub fn has_active_keys(&self, queue_id: &str) -> bool {
        self.queues
            .get(queue_id)
            .is_some_and(|s| !s.active_keys.is_empty())
    }

    /// Update the weight of a fairness key. Takes effect on the next round start.
    pub fn update_weight(&mut self, queue_id: &str, fairness_key: &str, weight: u32) {
        if let Some(state) = self.queues.get_mut(queue_id) {
            state
                .weights
                .insert(fairness_key.to_string(), weight.max(1));
        }
    }

    /// Return the list of active fairness keys for a queue (for recovery).
    pub fn active_keys(&self, queue_id: &str) -> Vec<String> {
        self.queues
            .get(queue_id)
            .map(|s| s.active_keys.iter().cloned().collect())
            .unwrap_or_default()
    }

    /// Return all queue IDs that have DRR state.
    pub fn queue_ids(&self) -> Vec<String> {
        self.queues.keys().cloned().collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn three_equal_weight_keys_get_equal_deficit() {
        let mut drr = DrrScheduler::new(10);
        drr.add_key("q1", "a", 1);
        drr.add_key("q1", "b", 1);
        drr.add_key("q1", "c", 1);

        drr.start_new_round("q1");

        // Each key should get deficit = weight(1) * quantum(10) = 10
        let mut counts = HashMap::new();
        let mut delivered = 0;
        loop {
            let Some(key) = drr.next_key("q1") else {
                break;
            };
            drr.consume_deficit("q1", &key);
            *counts.entry(key).or_insert(0) += 1;
            delivered += 1;
            // Safety: prevent infinite loop
            if delivered > 100 {
                panic!("delivered too many, likely infinite loop");
            }
        }

        assert_eq!(delivered, 30, "3 keys * 10 quantum = 30 total deliveries");
        assert_eq!(counts.get("a"), Some(&10));
        assert_eq!(counts.get("b"), Some(&10));
        assert_eq!(counts.get("c"), Some(&10));
    }

    #[test]
    fn key_removal_removes_from_active_set() {
        let mut drr = DrrScheduler::new(10);
        drr.add_key("q1", "a", 1);
        drr.add_key("q1", "b", 1);
        drr.add_key("q1", "c", 1);

        drr.remove_key("q1", "b");

        assert!(drr.has_active_keys("q1"));
        let keys = drr.active_keys("q1");
        assert_eq!(keys.len(), 2);
        assert!(keys.contains(&"a".to_string()));
        assert!(keys.contains(&"c".to_string()));
        assert!(!keys.contains(&"b".to_string()));
    }

    #[test]
    fn re_adding_key_after_removal() {
        let mut drr = DrrScheduler::new(10);
        drr.add_key("q1", "a", 1);
        drr.remove_key("q1", "a");
        assert!(!drr.has_active_keys("q1"));

        drr.add_key("q1", "a", 1);
        assert!(drr.has_active_keys("q1"));
        assert_eq!(drr.active_keys("q1"), vec!["a".to_string()]);
    }

    #[test]
    fn round_reset_refills_deficits() {
        let mut drr = DrrScheduler::new(5);
        drr.add_key("q1", "a", 2);
        drr.add_key("q1", "b", 1);

        drr.start_new_round("q1");

        // a gets 2*5=10, b gets 1*5=5
        let mut counts = HashMap::new();
        loop {
            let Some(key) = drr.next_key("q1") else {
                break;
            };
            drr.consume_deficit("q1", &key);
            *counts.entry(key).or_insert(0) += 1;
        }
        assert_eq!(counts.get("a"), Some(&10));
        assert_eq!(counts.get("b"), Some(&5));
    }

    #[test]
    fn single_key_gets_all_throughput() {
        let mut drr = DrrScheduler::new(10);
        drr.add_key("q1", "only", 1);
        drr.start_new_round("q1");

        let mut count = 0;
        while drr.next_key("q1").is_some() {
            drr.consume_deficit("q1", "only");
            count += 1;
        }
        assert_eq!(count, 10);
    }

    #[test]
    fn key_added_mid_round_gets_deficit_on_next_round() {
        let mut drr = DrrScheduler::new(10);
        drr.add_key("q1", "a", 1);
        drr.start_new_round("q1");

        // Consume some of a's deficit
        drr.next_key("q1").unwrap();
        drr.consume_deficit("q1", "a");

        // Add a new key mid-round — it should have deficit 0
        drr.add_key("q1", "b", 1);

        // b should not be deliverable in the current round (deficit=0)
        let mut a_count = 0;
        let mut b_count = 0;
        loop {
            let Some(key) = drr.next_key("q1") else {
                break;
            };
            drr.consume_deficit("q1", &key);
            match key.as_str() {
                "a" => a_count += 1,
                "b" => b_count += 1,
                _ => panic!("unexpected key"),
            }
        }
        assert_eq!(
            a_count, 9,
            "a should have 9 remaining (10-1 consumed before)"
        );
        assert_eq!(b_count, 0, "b should have 0 (added mid-round)");

        // After a new round, b should get deficit
        drr.start_new_round("q1");
        let mut a2 = 0;
        let mut b2 = 0;
        loop {
            let Some(key) = drr.next_key("q1") else {
                break;
            };
            drr.consume_deficit("q1", &key);
            match key.as_str() {
                "a" => a2 += 1,
                "b" => b2 += 1,
                _ => panic!("unexpected key"),
            }
        }
        assert_eq!(a2, 10);
        assert_eq!(b2, 10);
    }

    #[test]
    fn add_key_is_idempotent() {
        let mut drr = DrrScheduler::new(10);
        drr.add_key("q1", "a", 1);
        drr.start_new_round("q1");

        // Consume some deficit
        drr.next_key("q1").unwrap();
        drr.consume_deficit("q1", "a");
        // deficit is now 9

        // Re-adding should NOT reset deficit
        drr.add_key("q1", "a", 1);

        let mut count = 0;
        while drr.next_key("q1").is_some() {
            drr.consume_deficit("q1", "a");
            count += 1;
        }
        assert_eq!(count, 9, "deficit should not be reset by re-add");
    }

    #[test]
    fn no_active_keys_returns_false() {
        let drr = DrrScheduler::new(10);
        assert!(!drr.has_active_keys("nonexistent"));
    }

    #[test]
    fn next_key_returns_none_for_empty_queue() {
        let mut drr = DrrScheduler::new(10);
        assert!(drr.next_key("nonexistent").is_none());
    }

    #[test]
    fn weight_update_takes_effect_next_round() {
        let mut drr = DrrScheduler::new(10);
        drr.add_key("q1", "a", 1);
        drr.add_key("q1", "b", 1);

        drr.start_new_round("q1");
        // Both get 10

        // Update a's weight to 3 — should not affect current round
        drr.update_weight("q1", "a", 3);

        let mut counts = HashMap::new();
        loop {
            let Some(key) = drr.next_key("q1") else {
                break;
            };
            drr.consume_deficit("q1", &key);
            *counts.entry(key).or_insert(0) += 1;
        }
        // Current round still uses old deficit allocations
        assert_eq!(counts.get("a"), Some(&10));
        assert_eq!(counts.get("b"), Some(&10));

        // New round uses updated weight
        drr.start_new_round("q1");
        let mut counts2 = HashMap::new();
        loop {
            let Some(key) = drr.next_key("q1") else {
                break;
            };
            drr.consume_deficit("q1", &key);
            *counts2.entry(key).or_insert(0) += 1;
        }
        assert_eq!(counts2.get("a"), Some(&30)); // weight 3 * quantum 10
        assert_eq!(counts2.get("b"), Some(&10)); // weight 1 * quantum 10
    }

    #[test]
    fn independent_queue_state() {
        let mut drr = DrrScheduler::new(10);
        drr.add_key("q1", "a", 1);
        drr.add_key("q2", "b", 1);

        assert!(drr.has_active_keys("q1"));
        assert!(drr.has_active_keys("q2"));

        drr.remove_key("q1", "a");
        assert!(!drr.has_active_keys("q1"));
        assert!(drr.has_active_keys("q2"));
    }

    mod proptests {
        use super::*;
        use proptest::prelude::*;

        /// Strategy for generating a vector of (key_name, weight) tuples.
        /// Keys: 1-20, weights: 1-10. All keys have unlimited messages
        /// (sustained load) to ensure fairness can be measured cleanly.
        fn fairness_scenario() -> impl Strategy<Value = Vec<(String, u32)>> {
            (1usize..=20).prop_flat_map(|num_keys| {
                proptest::collection::vec(1u32..=10, num_keys..=num_keys).prop_map(|weights| {
                    weights
                        .into_iter()
                        .enumerate()
                        .map(|(i, weight)| (format!("key_{i}"), weight))
                        .collect()
                })
            })
        }

        proptest! {
            /// Property: under sustained load (all keys have unlimited messages),
            /// each key receives within 5% of its fair share over a fixed number
            /// of rounds.
            ///
            /// Fair share for key `k` = `weight_k / total_weight`.
            #[test]
            fn fairness_invariant_holds(scenario in fairness_scenario()) {
                let quantum = 100u32;
                let num_rounds = 10;
                let mut drr = DrrScheduler::new(quantum);

                let total_weight: u32 = scenario.iter().map(|(_, w)| *w).sum();

                for (key, weight) in &scenario {
                    drr.add_key("q1", key, *weight);
                }

                // Track deliveries per key — unlimited message supply
                let mut delivered: HashMap<String, usize> = HashMap::new();
                let mut total_delivered = 0usize;

                for _ in 0..num_rounds {
                    drr.start_new_round("q1");

                    loop {
                        let Some(key) = drr.next_key("q1") else {
                            break;
                        };
                        // Unlimited messages — always deliver
                        *delivered.entry(key.clone()).or_insert(0) += 1;
                        total_delivered += 1;
                        drr.consume_deficit("q1", &key);
                    }
                }

                prop_assert!(total_delivered > 0, "should have delivered some messages");

                for (key, weight) in &scenario {
                    let expected_share = *weight as f64 / total_weight as f64;
                    let actual_delivered = delivered.get(key).copied().unwrap_or(0);
                    let actual_share = actual_delivered as f64 / total_delivered as f64;

                    let tolerance = 0.05;
                    let diff = (actual_share - expected_share).abs();
                    prop_assert!(
                        diff <= tolerance,
                        "Key {} (weight={}): expected share {:.4}, actual {:.4}, diff {:.4} > {:.4}. Delivered {}/{}",
                        key, weight, expected_share, actual_share, diff, tolerance,
                        actual_delivered, total_delivered
                    );
                }
            }
        }
    }
}
