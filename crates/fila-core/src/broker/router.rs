/// Identifies a Raft group that owns a set of fairness keys for a queue.
///
/// In phase 1 (single-node), every queue maps to exactly one group whose ID
/// equals the queue ID. In phase 2 (hierarchical scaling), a queue's fairness
/// keys are partitioned across multiple groups via consistent hashing.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct GroupId(pub String);

/// Routes `(queue_id, fairness_key)` pairs to Raft groups.
///
/// Phase 1: trivial 1:1 routing — every fairness key in a queue maps to the
/// queue itself. The indirection exists in the code path so that phase 2 can
/// replace the implementation with a placement table and consistent hashing
/// without rearchitecting the enqueue path.
pub struct QueueRouter;

impl QueueRouter {
    pub fn new() -> Self {
        Self
    }

    /// Determine which Raft group should handle a message with the given
    /// queue and fairness key.
    ///
    /// Phase 1: always returns `GroupId(queue_id)` — fairness key is ignored.
    /// Phase 2: will use `consistent_hash(fairness_key) % num_groups`.
    pub fn route(&self, queue_id: &str, _fairness_key: &str) -> GroupId {
        GroupId(queue_id.to_string())
    }
}

impl Default for QueueRouter {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn route_returns_same_group_for_all_fairness_keys() {
        let router = QueueRouter::new();
        let g1 = router.route("orders", "tenant_a");
        let g2 = router.route("orders", "tenant_b");
        let g3 = router.route("orders", "tenant_c");
        assert_eq!(g1, g2);
        assert_eq!(g2, g3);
        assert_eq!(g1, GroupId("orders".to_string()));
    }

    #[test]
    fn route_returns_different_groups_for_different_queues() {
        let router = QueueRouter::new();
        let g1 = router.route("orders", "tenant_a");
        let g2 = router.route("payments", "tenant_a");
        assert_ne!(g1, g2);
    }
}
