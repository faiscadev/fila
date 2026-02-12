use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Instant;

use mlua::{Error, HookTriggers, Lua, VmState};

/// Circuit breaker state for a single queue's Lua script.
///
/// Tracks consecutive failures and trips (bypasses Lua) when the threshold
/// is reached. After a cooldown period, Lua execution is retried.
pub struct CircuitBreaker {
    consecutive_failures: u32,
    threshold: u32,
    cooldown_ms: u64,
    tripped_at: Option<Instant>,
}

impl CircuitBreaker {
    pub fn new(threshold: u32, cooldown_ms: u64) -> Self {
        Self {
            consecutive_failures: 0,
            threshold,
            cooldown_ms,
            tripped_at: None,
        }
    }

    /// Returns true if Lua should be executed (breaker not tripped or cooldown expired).
    pub fn should_execute(&self) -> bool {
        match self.tripped_at {
            None => true,
            Some(tripped) => tripped.elapsed().as_millis() as u64 >= self.cooldown_ms,
        }
    }

    /// Record a successful Lua execution. Resets the failure counter and clears
    /// the tripped state.
    pub fn record_success(&mut self) {
        self.consecutive_failures = 0;
        self.tripped_at = None;
    }

    /// Record a failed Lua execution. Increments the failure counter and trips
    /// the breaker if the threshold is reached.
    pub fn record_failure(&mut self) {
        self.consecutive_failures += 1;
        if self.consecutive_failures >= self.threshold {
            self.tripped_at = Some(Instant::now());
        }
    }

    /// Returns true if the circuit breaker is currently tripped (bypassing Lua).
    pub fn is_tripped(&self) -> bool {
        self.tripped_at.is_some() && !self.should_execute()
    }

    /// Enter half-open state: reset the failure counter so the breaker gives
    /// a fresh threshold of attempts before re-tripping.
    fn enter_half_open(&mut self) {
        self.consecutive_failures = 0;
        self.tripped_at = None;
    }
}

/// Per-queue safety configuration (effective values after resolving defaults).
pub struct QueueSafetyConfig {
    /// Instruction count limit for script execution timeout.
    pub instruction_limit: u64,
    /// Memory limit in bytes for script execution.
    pub memory_limit_bytes: usize,
}

/// Manages per-queue circuit breakers and safety configuration.
pub struct SafetyManager {
    breakers: HashMap<String, CircuitBreaker>,
    queue_configs: HashMap<String, QueueSafetyConfig>,
    default_threshold: u32,
    default_cooldown_ms: u64,
    default_instruction_limit: u64,
    default_memory_limit_bytes: usize,
}

/// Approximate instructions per millisecond for timeout calibration.
/// This is a rough estimate — operators can tune timeouts as needed.
const INSTRUCTIONS_PER_MS: u64 = 10_000;

impl SafetyManager {
    pub fn new(lua_config: &crate::broker::config::LuaConfig) -> Self {
        Self {
            breakers: HashMap::new(),
            queue_configs: HashMap::new(),
            default_threshold: lua_config.circuit_breaker_threshold,
            default_cooldown_ms: lua_config.circuit_breaker_cooldown_ms,
            default_instruction_limit: lua_config.default_timeout_ms * INSTRUCTIONS_PER_MS,
            default_memory_limit_bytes: lua_config.default_memory_limit_bytes,
        }
    }

    /// Register a queue with optional per-queue overrides.
    pub fn register_queue(
        &mut self,
        queue_id: &str,
        timeout_ms: Option<u64>,
        memory_limit_bytes: Option<usize>,
    ) {
        let instruction_limit = timeout_ms
            .map(|ms| ms * INSTRUCTIONS_PER_MS)
            .unwrap_or(self.default_instruction_limit);
        let memory = memory_limit_bytes.unwrap_or(self.default_memory_limit_bytes);

        self.queue_configs.insert(
            queue_id.to_string(),
            QueueSafetyConfig {
                instruction_limit,
                memory_limit_bytes: memory,
            },
        );
        self.breakers.insert(
            queue_id.to_string(),
            CircuitBreaker::new(self.default_threshold, self.default_cooldown_ms),
        );
    }

    /// Remove a queue's circuit breaker and safety config.
    pub fn remove_queue(&mut self, queue_id: &str) {
        self.breakers.remove(queue_id);
        self.queue_configs.remove(queue_id);
    }

    /// Check if Lua should be executed for the given queue (circuit breaker check).
    ///
    /// When a tripped breaker's cooldown has expired (half-open state), this
    /// resets the failure counter so the breaker gives a fresh threshold of
    /// attempts before re-tripping.
    pub fn should_execute(&mut self, queue_id: &str) -> bool {
        if let Some(cb) = self.breakers.get_mut(queue_id) {
            if cb.should_execute() {
                // Entering half-open: reset counter for a fresh threshold
                if cb.tripped_at.is_some() {
                    cb.enter_half_open();
                }
                true
            } else {
                false
            }
        } else {
            true
        }
    }

    /// Record a successful Lua execution for the given queue.
    pub fn record_success(&mut self, queue_id: &str) {
        if let Some(cb) = self.breakers.get_mut(queue_id) {
            cb.record_success();
        }
    }

    /// Record a failed Lua execution for the given queue. Returns true if the
    /// circuit breaker just tripped (threshold reached).
    pub fn record_failure(&mut self, queue_id: &str) -> bool {
        if let Some(cb) = self.breakers.get_mut(queue_id) {
            let was_tripped = cb.is_tripped();
            cb.record_failure();
            !was_tripped && cb.is_tripped()
        } else {
            false
        }
    }

    /// Get the safety config for a queue (instruction limit and memory limit).
    pub fn get_config(&self, queue_id: &str) -> Option<&QueueSafetyConfig> {
        self.queue_configs.get(queue_id)
    }

    /// Return the global default instruction limit and memory limit.
    ///
    /// Used as a fail-closed fallback when per-queue config is unexpectedly absent.
    pub fn default_limits(&self) -> (u64, usize) {
        (
            self.default_instruction_limit,
            self.default_memory_limit_bytes,
        )
    }
}

/// Set the instruction count hook on the Lua VM to enforce an execution timeout.
///
/// The hook fires every `batch` instructions and checks against the limit.
/// When exceeded, it returns an error that aborts the script.
pub fn set_instruction_hook(lua: &Lua, limit: u64) {
    let batch: u32 = 100;
    let counter = Arc::new(AtomicU64::new(0));

    lua.set_hook(
        HookTriggers::new().every_nth_instruction(batch),
        move |_lua, _debug| {
            let count = counter.fetch_add(batch as u64, Ordering::Relaxed);
            if count + batch as u64 >= limit {
                Err(Error::runtime("instruction limit exceeded"))
            } else {
                Ok(VmState::Continue)
            }
        },
    );
}

/// Remove the instruction count hook from the Lua VM.
pub fn remove_instruction_hook(lua: &Lua) {
    lua.remove_hook();
}

/// Set the memory limit on the Lua VM.
///
/// The limit is set relative to the current baseline memory usage.
pub fn set_memory_limit(lua: &Lua, limit_bytes: usize) {
    let baseline = lua.used_memory();
    let _ = lua.set_memory_limit(baseline + limit_bytes);
}

/// Remove the memory limit from the Lua VM (set to unlimited).
pub fn reset_memory_limit(lua: &Lua) {
    let _ = lua.set_memory_limit(0);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn circuit_breaker_allows_execution_when_not_tripped() {
        let cb = CircuitBreaker::new(3, 10_000);
        assert!(cb.should_execute());
    }

    #[test]
    fn circuit_breaker_trips_at_exact_threshold() {
        let mut cb = CircuitBreaker::new(3, 10_000);
        cb.record_failure(); // 1
        assert!(cb.should_execute());
        cb.record_failure(); // 2
        assert!(cb.should_execute());
        cb.record_failure(); // 3 = threshold
        assert!(!cb.should_execute(), "should be tripped at threshold");
    }

    #[test]
    fn circuit_breaker_does_not_trip_before_threshold() {
        let mut cb = CircuitBreaker::new(3, 10_000);
        cb.record_failure();
        cb.record_failure();
        assert!(
            cb.should_execute(),
            "should not be tripped before threshold"
        );
    }

    #[test]
    fn circuit_breaker_success_resets_counter() {
        let mut cb = CircuitBreaker::new(3, 10_000);
        cb.record_failure(); // 1
        cb.record_failure(); // 2
        cb.record_success(); // reset
        cb.record_failure(); // 1 again
        cb.record_failure(); // 2 again
        assert!(cb.should_execute(), "should not trip after success reset");
    }

    #[test]
    fn circuit_breaker_retries_after_cooldown() {
        let mut cb = CircuitBreaker::new(2, 50); // 50ms cooldown
        cb.record_failure();
        cb.record_failure(); // trips
        assert!(!cb.should_execute(), "should be tripped");

        // Wait for cooldown
        std::thread::sleep(std::time::Duration::from_millis(60));
        assert!(cb.should_execute(), "should allow execution after cooldown");
    }

    #[test]
    fn circuit_breaker_success_after_cooldown_resets() {
        let mut cb = CircuitBreaker::new(2, 50);
        cb.record_failure();
        cb.record_failure(); // trips

        std::thread::sleep(std::time::Duration::from_millis(60));
        assert!(cb.should_execute());
        cb.record_success();

        // Should be fully reset now
        cb.record_failure(); // 1
        assert!(cb.should_execute(), "counter should be reset after success");
    }

    #[test]
    fn safety_manager_register_and_check() {
        let config = crate::broker::config::LuaConfig {
            default_timeout_ms: 10,
            default_memory_limit_bytes: 1_048_576,
            circuit_breaker_threshold: 3,
            circuit_breaker_cooldown_ms: 10_000,
        };
        let mut mgr = SafetyManager::new(&config);
        mgr.register_queue("q1", None, None);

        assert!(mgr.should_execute("q1"));
        assert!(mgr.should_execute("unknown-queue")); // unknown queues default to allow

        let cfg = mgr.get_config("q1").unwrap();
        assert_eq!(cfg.instruction_limit, 10 * INSTRUCTIONS_PER_MS);
        assert_eq!(cfg.memory_limit_bytes, 1_048_576);
    }

    #[test]
    fn safety_manager_per_queue_overrides() {
        let config = crate::broker::config::LuaConfig {
            default_timeout_ms: 10,
            default_memory_limit_bytes: 1_048_576,
            circuit_breaker_threshold: 3,
            circuit_breaker_cooldown_ms: 10_000,
        };
        let mut mgr = SafetyManager::new(&config);
        mgr.register_queue("q1", Some(20), Some(2_097_152));

        let cfg = mgr.get_config("q1").unwrap();
        assert_eq!(cfg.instruction_limit, 20 * INSTRUCTIONS_PER_MS);
        assert_eq!(cfg.memory_limit_bytes, 2_097_152);
    }

    #[test]
    fn safety_manager_remove_queue() {
        let config = crate::broker::config::LuaConfig::default();
        let mut mgr = SafetyManager::new(&config);
        mgr.register_queue("q1", None, None);
        assert!(mgr.get_config("q1").is_some());

        mgr.remove_queue("q1");
        assert!(mgr.get_config("q1").is_none());
    }

    #[test]
    fn instruction_hook_kills_infinite_loop() {
        let lua = crate::lua::sandbox::create_sandbox().unwrap();
        set_instruction_hook(&lua, 1_000);

        let err = lua.load("while true do end").exec().unwrap_err();
        assert!(
            err.to_string().contains("instruction limit exceeded"),
            "expected instruction limit error, got: {err}"
        );

        remove_instruction_hook(&lua);

        // VM should still be usable
        let result: i64 = lua.load("return 1 + 1").eval().expect("post-hook eval");
        assert_eq!(result, 2);
    }

    #[test]
    fn memory_limit_kills_allocating_script() {
        let lua = crate::lua::sandbox::create_sandbox().unwrap();
        set_memory_limit(&lua, 512 * 1024); // 512 KB

        let result = lua
            .load(
                r#"
                local t = {}
                for i = 1, 10000000 do
                    t[i] = string.rep("x", 1000)
                end
            "#,
            )
            .exec();

        assert!(
            matches!(result, Err(mlua::Error::MemoryError(_))),
            "expected MemoryError, got: {result:?}"
        );

        reset_memory_limit(&lua);

        // VM should still be usable
        let val: i64 = lua.load("return 42").eval().expect("post-limit eval");
        assert_eq!(val, 42);
    }

    #[test]
    fn safety_manager_half_open_resets_counter() {
        // After cooldown, should_execute enters half-open state and resets
        // the failure counter, giving a fresh threshold of attempts.
        let config = crate::broker::config::LuaConfig {
            default_timeout_ms: 10,
            default_memory_limit_bytes: 1_048_576,
            circuit_breaker_threshold: 2,
            circuit_breaker_cooldown_ms: 50,
        };
        let mut mgr = SafetyManager::new(&config);
        mgr.register_queue("q1", None, None);

        // Trip the breaker
        mgr.record_failure("q1");
        mgr.record_failure("q1");
        assert!(!mgr.should_execute("q1"), "should be tripped");

        // Wait for cooldown
        std::thread::sleep(std::time::Duration::from_millis(60));

        // Half-open: should_execute resets the counter
        assert!(mgr.should_execute("q1"), "should allow after cooldown");

        // A single failure should NOT re-trip (counter was reset, threshold is 2)
        mgr.record_failure("q1");
        assert!(
            mgr.should_execute("q1"),
            "one failure after half-open should not re-trip (threshold is 2)"
        );

        // Second failure re-trips
        mgr.record_failure("q1");
        assert!(
            !mgr.should_execute("q1"),
            "two failures after half-open should re-trip"
        );
    }
}
