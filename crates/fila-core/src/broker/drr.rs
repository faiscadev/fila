use std::collections::{HashMap, HashSet, VecDeque};

use lasso::Spur;

/// Per-queue DRR scheduling state.
#[derive(Default)]
struct DrrQueueState {
    /// Active fairness keys in round-robin order (canonical order for replenish).
    active_keys: VecDeque<Spur>,
    /// Set for O(1) membership checks.
    active_set: HashSet<Spur>,
    /// Remaining deficit per fairness key. A key can deliver messages
    /// as long as its deficit is positive.
    deficits: HashMap<Spur, i64>,
    /// Weight per fairness key (default 1).
    weights: HashMap<Spur, u32>,
    /// Keys with positive deficit, in round-robin delivery order.
    /// Replaces the O(n) `round_position` scan with O(1) front pop.
    eligible_keys: VecDeque<Spur>,
    /// Whether a round is currently active (deficits have been allocated
    /// but not yet exhausted). Prevents `start_new_round` from being
    /// called multiple times before the current round finishes.
    round_active: bool,
    /// Number of messages delivered from the current burst key. Each key
    /// delivers `weight` messages per turn before yielding. Reset to 0
    /// when advancing to the next key.
    burst_delivered: u32,
    /// The key currently being burst-served. When `consume_deficit` is
    /// called for a different key, the burst resets.
    current_burst_key: Option<Spur>,
}

/// Deficit Round Robin scheduler. Manages per-queue fairness state.
///
/// Each queue maintains an independent active key set, deficit counters,
/// and round position. The scheduler is intended to run on a single thread
/// — no internal synchronization.
///
/// **Phase 2 design property:** The DRR engine operates exclusively on keys
/// registered via `add_key()`. It has no mechanism to discover fairness keys
/// from storage or queue configuration — the key-set is an explicit input.
/// In phase 1, the caller adds all fairness keys for a queue. In phase 2
/// (hierarchical scaling), each Raft group adds only its assigned subset of
/// fairness keys, and the DRR engine works unchanged.
///
/// **String interning:** All queue_id and fairness_key parameters are `Spur`
/// tokens from the scheduler's `ThreadedRodeo` interner. This eliminates
/// per-message string cloning and reduces HashMap key size from 24+ bytes
/// (String) to 4 bytes (Spur, Copy).
pub struct DrrScheduler {
    queues: HashMap<Spur, DrrQueueState>,
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
    pub fn add_key(&mut self, queue_id: Spur, fairness_key: Spur, weight: u32) {
        let state = self.queues.entry(queue_id).or_default();

        let w = weight.max(1); // weight must be at least 1
        state.weights.insert(fairness_key, w);

        if !state.active_set.contains(&fairness_key) {
            state.active_set.insert(fairness_key);
            state.active_keys.push_back(fairness_key);
            // New keys start with zero deficit — they receive allocation on the
            // next round start.
            state.deficits.entry(fairness_key).or_insert(0);
        }
    }

    /// Remove a fairness key from the active set (e.g., when it has no more
    /// pending messages).
    pub fn remove_key(&mut self, queue_id: Spur, fairness_key: Spur) {
        let Some(state) = self.queues.get_mut(&queue_id) else {
            return;
        };

        if state.active_set.remove(&fairness_key) {
            if let Some(idx) = state.active_keys.iter().position(|k| *k == fairness_key) {
                state.active_keys.remove(idx);
            }
            state.deficits.remove(&fairness_key);
            state.weights.remove(&fairness_key);
            state.eligible_keys.retain(|k| *k != fairness_key);

            if state.active_keys.is_empty() {
                state.round_active = false;
                state.burst_delivered = 0;
                state.current_burst_key = None;
            } else {
                // Reset burst if the removed key was the current burst key
                if state.current_burst_key == Some(fairness_key) {
                    state.burst_delivered = 0;
                    state.current_burst_key = None;
                }
            }
        }
    }

    /// Return the next fairness key that has positive deficit in the current
    /// round. This is an idempotent peek — calling it multiple times without
    /// `consume_deficit` returns the same key. State advancement (burst
    /// tracking, eligible deque rotation) happens in `consume_deficit`.
    ///
    /// Returns `None` when the round is exhausted (all remaining keys have
    /// non-positive deficit), and marks the round as inactive so a new
    /// round can be started.
    ///
    /// O(1) — reads from the front of the eligible deque instead of scanning
    /// all active keys.
    pub fn next_key(&mut self, queue_id: Spur) -> Option<Spur> {
        let state = self.queues.get_mut(&queue_id)?;

        if let Some(&key) = state.eligible_keys.front() {
            return Some(key);
        }

        // Eligible deque empty — round exhausted
        state.round_active = false;
        None
    }

    /// Decrement the deficit of a fairness key by 1 after delivering a message.
    /// Also tracks burst progression: each key delivers `weight` messages
    /// per turn before yielding to the next eligible key. This ensures
    /// proportional interleaving within any delivery window.
    pub fn consume_deficit(&mut self, queue_id: Spur, fairness_key: Spur) {
        let Some(state) = self.queues.get_mut(&queue_id) else {
            return;
        };

        let remaining = if let Some(d) = state.deficits.get_mut(&fairness_key) {
            *d -= 1;
            *d
        } else {
            0
        };

        // Track burst: if this is a different key than the current burst,
        // reset the burst counter.
        if state.current_burst_key != Some(fairness_key) {
            state.current_burst_key = Some(fairness_key);
            state.burst_delivered = 1;
        } else {
            state.burst_delivered += 1;
        }

        // Rotate when either: the burst completes, or the key's deficit is
        // exhausted (even mid-burst). The old linear-scan `next_key` would
        // skip keys with non-positive deficit; we maintain the same behavior
        // by eagerly removing them from the eligible deque.
        let weight = state.weights.get(&fairness_key).copied().unwrap_or(1);
        let burst_done = state.burst_delivered >= weight;
        let deficit_exhausted = remaining <= 0;

        if burst_done || deficit_exhausted {
            state.burst_delivered = 0;
            state.current_burst_key = None;

            // Remove current key from front of eligible deque
            if state.eligible_keys.front().copied() == Some(fairness_key) {
                state.eligible_keys.pop_front();
            }

            // Re-append at back only if it still has positive deficit
            if remaining > 0 {
                state.eligible_keys.push_back(fairness_key);
            }
        }
    }

    /// Drain all remaining deficit for a fairness key (set to 0).
    /// Used when a key is throttled to skip all its remaining deficit in O(1)
    /// rather than consuming one at a time. Also resets burst state and
    /// removes the key from the eligible deque.
    pub fn drain_deficit(&mut self, queue_id: Spur, fairness_key: Spur) {
        let Some(state) = self.queues.get_mut(&queue_id) else {
            return;
        };
        if let Some(d) = state.deficits.get_mut(&fairness_key) {
            *d = 0;
        }
        // Reset burst if this was the current burst key
        if state.current_burst_key == Some(fairness_key) {
            state.burst_delivered = 0;
            state.current_burst_key = None;
        }
        // Remove from eligible deque so next_key skips it
        state.eligible_keys.retain(|k| *k != fairness_key);
    }

    /// Start a new DRR round for a queue: refill deficit for all active keys
    /// based on `weight * quantum`. No-op if a round is already active
    /// (prevents unbounded deficit accumulation from repeated calls).
    /// Returns `true` if a new round was actually started, `false` if the
    /// previous round is still active.
    pub fn start_new_round(&mut self, queue_id: Spur) -> bool {
        let Some(state) = self.queues.get_mut(&queue_id) else {
            return false;
        };

        if state.round_active {
            return false;
        }

        // Rebuild eligible deque: replenish deficits and include keys with
        // positive deficit in active_keys order.
        state.eligible_keys.clear();
        for &key in &state.active_keys {
            let weight = state.weights.get(&key).copied().unwrap_or(1);
            let deficit = state.deficits.entry(key).or_insert(0);
            *deficit += (weight as i64) * (self.quantum as i64);
            if *deficit > 0 {
                state.eligible_keys.push_back(key);
            }
        }

        state.round_active = true;
        state.burst_delivered = 0;
        state.current_burst_key = None;
        true
    }

    /// Remove all DRR state for a queue (e.g., when the queue is deleted).
    pub fn remove_queue(&mut self, queue_id: Spur) {
        self.queues.remove(&queue_id);
    }

    /// Check whether a queue has any active fairness keys.
    pub fn has_active_keys(&self, queue_id: Spur) -> bool {
        self.queues
            .get(&queue_id)
            .is_some_and(|s| !s.active_keys.is_empty())
    }

    /// Update the weight of a fairness key. Takes effect on the next round start.
    pub fn update_weight(&mut self, queue_id: Spur, fairness_key: Spur, weight: u32) {
        if let Some(state) = self.queues.get_mut(&queue_id) {
            state.weights.insert(fairness_key, weight.max(1));
        }
    }

    /// Return the list of active fairness key spurs for a queue (for recovery).
    pub fn active_keys(&self, queue_id: Spur) -> Vec<Spur> {
        self.queues
            .get(&queue_id)
            .map(|s| s.active_keys.iter().copied().collect())
            .unwrap_or_default()
    }

    /// Return all queue ID spurs that have DRR state.
    pub fn queue_ids(&self) -> Vec<Spur> {
        self.queues.keys().copied().collect()
    }

    /// Return the global DRR quantum value.
    pub fn quantum(&self) -> u32 {
        self.quantum
    }

    /// Return per-key stats for a queue: `(spur, deficit, weight)` tuples.
    pub fn key_stats(&self, queue_id: Spur) -> Vec<(Spur, i64, u32)> {
        let Some(state) = self.queues.get(&queue_id) else {
            return Vec::new();
        };
        state
            .active_keys
            .iter()
            .map(|&key| {
                let deficit = state.deficits.get(&key).copied().unwrap_or(0);
                let weight = state.weights.get(&key).copied().unwrap_or(1);
                (key, deficit, weight)
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use lasso::ThreadedRodeo;

    #[test]
    fn three_equal_weight_keys_get_equal_deficit() {
        let interner = ThreadedRodeo::default();
        let q1 = interner.get_or_intern("q1");
        let a = interner.get_or_intern("a");
        let b = interner.get_or_intern("b");
        let c = interner.get_or_intern("c");

        let mut drr = DrrScheduler::new(10);
        drr.add_key(q1, a, 1);
        drr.add_key(q1, b, 1);
        drr.add_key(q1, c, 1);

        drr.start_new_round(q1);

        // Each key should get deficit = weight(1) * quantum(10) = 10
        let mut counts = HashMap::new();
        let mut delivered = 0;
        loop {
            let Some(key) = drr.next_key(q1) else {
                break;
            };
            drr.consume_deficit(q1, key);
            *counts
                .entry(interner.resolve(&key).to_string())
                .or_insert(0) += 1;
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
        let interner = ThreadedRodeo::default();
        let q1 = interner.get_or_intern("q1");
        let a = interner.get_or_intern("a");
        let b = interner.get_or_intern("b");
        let c = interner.get_or_intern("c");

        let mut drr = DrrScheduler::new(10);
        drr.add_key(q1, a, 1);
        drr.add_key(q1, b, 1);
        drr.add_key(q1, c, 1);

        drr.remove_key(q1, b);

        assert!(drr.has_active_keys(q1));
        let keys: Vec<String> = drr
            .active_keys(q1)
            .iter()
            .map(|s| interner.resolve(s).to_string())
            .collect();
        assert_eq!(keys.len(), 2);
        assert!(keys.contains(&"a".to_string()));
        assert!(keys.contains(&"c".to_string()));
        assert!(!keys.contains(&"b".to_string()));
    }

    #[test]
    fn re_adding_key_after_removal() {
        let interner = ThreadedRodeo::default();
        let q1 = interner.get_or_intern("q1");
        let a = interner.get_or_intern("a");

        let mut drr = DrrScheduler::new(10);
        drr.add_key(q1, a, 1);
        drr.remove_key(q1, a);
        assert!(!drr.has_active_keys(q1));

        drr.add_key(q1, a, 1);
        assert!(drr.has_active_keys(q1));
        assert_eq!(drr.active_keys(q1), vec![a]);
    }

    #[test]
    fn round_reset_refills_deficits() {
        let interner = ThreadedRodeo::default();
        let q1 = interner.get_or_intern("q1");
        let a = interner.get_or_intern("a");
        let b = interner.get_or_intern("b");

        let mut drr = DrrScheduler::new(5);
        drr.add_key(q1, a, 2);
        drr.add_key(q1, b, 1);

        drr.start_new_round(q1);

        // a gets 2*5=10, b gets 1*5=5
        let mut counts = HashMap::new();
        loop {
            let Some(key) = drr.next_key(q1) else {
                break;
            };
            drr.consume_deficit(q1, key);
            *counts
                .entry(interner.resolve(&key).to_string())
                .or_insert(0) += 1;
        }
        assert_eq!(counts.get("a"), Some(&10));
        assert_eq!(counts.get("b"), Some(&5));
    }

    #[test]
    fn single_key_gets_all_throughput() {
        let interner = ThreadedRodeo::default();
        let q1 = interner.get_or_intern("q1");
        let only = interner.get_or_intern("only");

        let mut drr = DrrScheduler::new(10);
        drr.add_key(q1, only, 1);
        drr.start_new_round(q1);

        let mut count = 0;
        while drr.next_key(q1).is_some() {
            drr.consume_deficit(q1, only);
            count += 1;
        }
        assert_eq!(count, 10);
    }

    #[test]
    fn key_added_mid_round_gets_deficit_on_next_round() {
        let interner = ThreadedRodeo::default();
        let q1 = interner.get_or_intern("q1");
        let a = interner.get_or_intern("a");
        let b = interner.get_or_intern("b");

        let mut drr = DrrScheduler::new(10);
        drr.add_key(q1, a, 1);
        drr.start_new_round(q1);

        // Consume some of a's deficit
        drr.next_key(q1).unwrap();
        drr.consume_deficit(q1, a);

        // Add a new key mid-round — it should have deficit 0
        drr.add_key(q1, b, 1);

        // b should not be deliverable in the current round (deficit=0)
        let mut a_count = 0;
        let mut b_count = 0;
        loop {
            let Some(key) = drr.next_key(q1) else {
                break;
            };
            drr.consume_deficit(q1, key);
            if key == a {
                a_count += 1;
            } else if key == b {
                b_count += 1;
            } else {
                panic!("unexpected key");
            }
        }
        assert_eq!(
            a_count, 9,
            "a should have 9 remaining (10-1 consumed before)"
        );
        assert_eq!(b_count, 0, "b should have 0 (added mid-round)");

        // After a new round, b should get deficit
        drr.start_new_round(q1);
        let mut a2 = 0;
        let mut b2 = 0;
        loop {
            let Some(key) = drr.next_key(q1) else {
                break;
            };
            drr.consume_deficit(q1, key);
            if key == a {
                a2 += 1;
            } else if key == b {
                b2 += 1;
            } else {
                panic!("unexpected key");
            }
        }
        assert_eq!(a2, 10);
        assert_eq!(b2, 10);
    }

    #[test]
    fn add_key_is_idempotent() {
        let interner = ThreadedRodeo::default();
        let q1 = interner.get_or_intern("q1");
        let a = interner.get_or_intern("a");

        let mut drr = DrrScheduler::new(10);
        drr.add_key(q1, a, 1);
        drr.start_new_round(q1);

        // Consume some deficit
        drr.next_key(q1).unwrap();
        drr.consume_deficit(q1, a);
        // deficit is now 9

        // Re-adding should NOT reset deficit
        drr.add_key(q1, a, 1);

        let mut count = 0;
        while drr.next_key(q1).is_some() {
            drr.consume_deficit(q1, a);
            count += 1;
        }
        assert_eq!(count, 9, "deficit should not be reset by re-add");
    }

    #[test]
    fn no_active_keys_returns_false() {
        let interner = ThreadedRodeo::default();
        let nonexistent = interner.get_or_intern("nonexistent");
        let drr = DrrScheduler::new(10);
        assert!(!drr.has_active_keys(nonexistent));
    }

    #[test]
    fn next_key_returns_none_for_empty_queue() {
        let interner = ThreadedRodeo::default();
        let nonexistent = interner.get_or_intern("nonexistent");
        let mut drr = DrrScheduler::new(10);
        assert!(drr.next_key(nonexistent).is_none());
    }

    #[test]
    fn weight_update_takes_effect_next_round() {
        let interner = ThreadedRodeo::default();
        let q1 = interner.get_or_intern("q1");
        let a = interner.get_or_intern("a");
        let b = interner.get_or_intern("b");

        let mut drr = DrrScheduler::new(10);
        drr.add_key(q1, a, 1);
        drr.add_key(q1, b, 1);

        drr.start_new_round(q1);
        // Both get 10

        // Update a's weight to 3 — should not affect current round
        drr.update_weight(q1, a, 3);

        let mut counts = HashMap::new();
        loop {
            let Some(key) = drr.next_key(q1) else {
                break;
            };
            drr.consume_deficit(q1, key);
            *counts
                .entry(interner.resolve(&key).to_string())
                .or_insert(0) += 1;
        }
        // Current round still uses old deficit allocations
        assert_eq!(counts.get("a"), Some(&10));
        assert_eq!(counts.get("b"), Some(&10));

        // New round uses updated weight
        drr.start_new_round(q1);
        let mut counts2 = HashMap::new();
        loop {
            let Some(key) = drr.next_key(q1) else {
                break;
            };
            drr.consume_deficit(q1, key);
            *counts2
                .entry(interner.resolve(&key).to_string())
                .or_insert(0) += 1;
        }
        assert_eq!(counts2.get("a"), Some(&30)); // weight 3 * quantum 10
        assert_eq!(counts2.get("b"), Some(&10)); // weight 1 * quantum 10
    }

    #[test]
    fn independent_queue_state() {
        let interner = ThreadedRodeo::default();
        let q1 = interner.get_or_intern("q1");
        let q2 = interner.get_or_intern("q2");
        let a = interner.get_or_intern("a");
        let b = interner.get_or_intern("b");

        let mut drr = DrrScheduler::new(10);
        drr.add_key(q1, a, 1);
        drr.add_key(q2, b, 1);

        assert!(drr.has_active_keys(q1));
        assert!(drr.has_active_keys(q2));

        drr.remove_key(q1, a);
        assert!(!drr.has_active_keys(q1));
        assert!(drr.has_active_keys(q2));
    }

    #[test]
    fn weighted_keys_deliver_proportionally_within_window() {
        let interner = ThreadedRodeo::default();
        let q1 = interner.get_or_intern("q1");
        let key_1 = interner.get_or_intern("key_1");
        let key_2 = interner.get_or_intern("key_2");
        let key_3 = interner.get_or_intern("key_3");

        let mut drr = DrrScheduler::new(60);
        drr.add_key(q1, key_1, 1);
        drr.add_key(q1, key_2, 2);
        drr.add_key(q1, key_3, 3);

        drr.start_new_round(q1);

        let window = 180;
        let mut counts = HashMap::new();
        for _ in 0..window {
            let Some(key) = drr.next_key(q1) else {
                break;
            };
            drr.consume_deficit(q1, key);
            *counts
                .entry(interner.resolve(&key).to_string())
                .or_insert(0) += 1;
        }

        let total: usize = counts.values().sum();
        assert_eq!(total, 180, "should deliver exactly 180 messages");

        for (key, weight, expected_share) in [
            ("key_1", 1, 1.0 / 6.0),
            ("key_2", 2, 2.0 / 6.0),
            ("key_3", 3, 3.0 / 6.0),
        ] {
            let actual = counts.get(key).copied().unwrap_or(0);
            let actual_share = actual as f64 / total as f64;
            let diff = (actual_share - expected_share).abs();
            assert!(
                diff <= 0.10,
                "Key {} (weight={}): expected share {:.4}, actual {:.4}, diff {:.4} > 0.10",
                key,
                weight,
                expected_share,
                actual_share,
                diff,
            );
        }
    }

    mod proptests {
        use super::*;
        use lasso::ThreadedRodeo;
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
                let interner = ThreadedRodeo::default();
                let q1 = interner.get_or_intern("q1");
                let mut drr = DrrScheduler::new(quantum);

                let total_weight: u32 = scenario.iter().map(|(_, w)| *w).sum();

                // Intern all keys and add to DRR
                let interned_keys: Vec<(Spur, String, u32)> = scenario
                    .iter()
                    .map(|(key, weight)| {
                        let spur = interner.get_or_intern(key);
                        drr.add_key(q1, spur, *weight);
                        (spur, key.clone(), *weight)
                    })
                    .collect();

                // Track deliveries per key — unlimited message supply
                let mut delivered: HashMap<String, usize> = HashMap::new();
                let mut total_delivered = 0usize;

                for _ in 0..num_rounds {
                    drr.start_new_round(q1);

                    loop {
                        let Some(key) = drr.next_key(q1) else {
                            break;
                        };
                        // Unlimited messages — always deliver
                        let key_str = interner.resolve(&key).to_string();
                        *delivered.entry(key_str).or_insert(0) += 1;
                        total_delivered += 1;
                        drr.consume_deficit(q1, key);
                    }
                }

                prop_assert!(total_delivered > 0, "should have delivered some messages");

                for (_, key, weight) in &interned_keys {
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
