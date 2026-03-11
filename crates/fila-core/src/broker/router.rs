/// Identifies a Raft group that owns a subset of a queue's messages.
///
/// In phase 1 (single-node), every queue maps to exactly one group whose ID
/// equals the queue ID. In phase 2 (hierarchical scaling), messages are
/// partitioned across multiple groups — by fairness key for FIFO queues
/// (preserving per-key ordering) or by message ID for non-FIFO queues
/// (load-balanced distribution).
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct GroupId(pub String);

/// Routes messages to Raft groups based on queue configuration.
///
/// Phase 1: trivial 1:1 routing — every message in a queue maps to the
/// queue itself. The indirection exists in the code path so that phase 2 can
/// replace the implementation without rearchitecting the enqueue path.
///
/// Phase 2 routing strategy depends on queue mode:
/// - **FIFO queues:** route by `consistent_hash(fairness_key)` so all messages
///   for a given fairness key land on the same group, preserving per-key order.
/// - **Non-FIFO queues:** route by `consistent_hash(message_id)` for even
///   load distribution across groups.
pub struct QueueRouter;

impl QueueRouter {
    pub fn new() -> Self {
        Self
    }

    /// Determine which Raft group should handle a message.
    ///
    /// Phase 1: always returns `GroupId(queue_id)` — all other fields ignored.
    /// Phase 2: FIFO queues hash by fairness key, non-FIFO queues hash by
    /// message ID (see struct-level docs).
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
