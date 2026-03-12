/// Stats for a single fairness key within a queue.
pub struct FairnessKeyStats {
    pub key: String,
    pub pending_count: u64,
    pub current_deficit: i64,
    pub weight: u32,
}

/// Stats for a single throttle key (global, not per-queue).
pub struct ThrottleKeyStats {
    pub key: String,
    pub tokens: f64,
    pub rate_per_second: f64,
    pub burst: f64,
}

/// Aggregate stats for a single queue.
pub struct QueueStats {
    pub depth: u64,
    pub in_flight: u64,
    pub active_fairness_keys: u64,
    pub active_consumers: u32,
    pub quantum: u32,
    pub per_key_stats: Vec<FairnessKeyStats>,
    pub per_throttle_stats: Vec<ThrottleKeyStats>,
    /// Raft leader node ID for this queue (0 = not clustered).
    pub leader_node_id: u64,
    /// Number of replicas in the queue's Raft group (0 = not clustered).
    pub replication_count: u32,
}
