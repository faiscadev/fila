use serde::{Deserialize, Serialize};

/// Queue configuration stored in the `queues` column family.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct QueueConfig {
    pub name: String,
    pub on_enqueue_script: Option<String>,
    pub on_failure_script: Option<String>,
    pub visibility_timeout_ms: u64,
    pub dlq_queue_id: Option<String>,
    /// Per-queue Lua timeout override (ms). If None, uses global default.
    pub lua_timeout_ms: Option<u64>,
    /// Per-queue Lua memory limit override (bytes). If None, uses global default.
    pub lua_memory_limit_bytes: Option<usize>,
}

impl QueueConfig {
    /// Default visibility timeout: 30 seconds.
    pub const DEFAULT_VISIBILITY_TIMEOUT_MS: u64 = 30_000;

    pub fn new(name: String) -> Self {
        Self {
            name,
            on_enqueue_script: None,
            on_failure_script: None,
            visibility_timeout_ms: Self::DEFAULT_VISIBILITY_TIMEOUT_MS,
            dlq_queue_id: None,
            lua_timeout_ms: None,
            lua_memory_limit_bytes: None,
        }
    }
}
