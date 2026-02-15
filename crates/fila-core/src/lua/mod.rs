pub mod bridge;
pub mod on_enqueue;
pub mod on_failure;
pub mod safety;
pub mod sandbox;

use std::collections::HashMap;
use std::sync::Arc;

use mlua::Lua;

use crate::broker::config::LuaConfig;
use crate::storage::Storage;

pub use on_enqueue::OnEnqueueResult;
pub use on_failure::{FailureAction, OnFailureResult};
pub use safety::SafetyManager;

/// Outcome of a Lua script execution, surfaced to the scheduler for metric recording.
#[derive(Debug, PartialEq)]
pub enum LuaExecOutcome {
    /// Script executed successfully.
    Success,
    /// Script failed with an error. If `circuit_breaker_tripped` is true,
    /// this failure caused the circuit breaker to transition to open state.
    ScriptError { circuit_breaker_tripped: bool },
    /// Script was not executed because the circuit breaker is active.
    CircuitBreakerBypassed,
}

/// Pre-compiled Lua bytecode for a script.
pub type CompiledScript = Vec<u8>;

/// The Lua engine manages a sandboxed Lua VM instance and pre-compiled scripts.
///
/// Owns a single `mlua::Lua` VM that lives on the scheduler thread.
/// Scripts are pre-compiled to bytecode at queue creation time and stored
/// in a cache keyed by queue_id. Execution loads the bytecode into the VM
/// for each call.
pub struct LuaEngine {
    lua: Lua,
    /// Pre-compiled on_enqueue bytecode per queue_id.
    on_enqueue_cache: HashMap<String, CompiledScript>,
    /// Pre-compiled on_failure bytecode per queue_id.
    on_failure_cache: HashMap<String, CompiledScript>,
    /// Per-queue safety configuration and circuit breakers.
    safety: SafetyManager,
}

impl LuaEngine {
    /// Create a new LuaEngine with a sandboxed Lua VM and fila.get() bridge.
    pub fn new(storage: Arc<dyn Storage>, lua_config: &LuaConfig) -> Result<Self, LuaError> {
        let lua = sandbox::create_sandbox().map_err(LuaError::VmCreation)?;
        bridge::register_fila_api(&lua, storage).map_err(LuaError::BridgeRegistration)?;

        Ok(Self {
            lua,
            on_enqueue_cache: HashMap::new(),
            on_failure_cache: HashMap::new(),
            safety: SafetyManager::new(lua_config),
        })
    }

    /// Compile a Lua source string to bytecode.
    ///
    /// Validates that the source is syntactically correct and can be compiled.
    /// Returns the bytecode that can be stored in the cache.
    pub fn compile_script(&self, source: &str) -> Result<CompiledScript, LuaError> {
        let func = self
            .lua
            .load(source)
            .set_name("lua_script")
            .into_function()
            .map_err(LuaError::Compilation)?;

        Ok(func.dump(true))
    }

    /// Register per-queue safety config (circuit breaker, timeout, memory limit).
    ///
    /// Must be called once per queue before caching any scripts. Safety config
    /// is per-queue, not per-script — both on_enqueue and on_failure share the
    /// same limits.
    pub fn register_queue_safety(
        &mut self,
        queue_id: &str,
        timeout_ms: Option<u64>,
        memory_limit_bytes: Option<usize>,
    ) {
        self.safety
            .register_queue(queue_id, timeout_ms, memory_limit_bytes);
    }

    /// Cache a pre-compiled on_enqueue script for a queue.
    pub fn cache_on_enqueue(&mut self, queue_id: &str, bytecode: CompiledScript) {
        self.on_enqueue_cache.insert(queue_id.to_string(), bytecode);
    }

    /// Remove all cached scripts and safety state for a queue.
    pub fn remove_queue_scripts(&mut self, queue_id: &str) {
        self.on_enqueue_cache.remove(queue_id);
        self.on_failure_cache.remove(queue_id);
        self.safety.remove_queue(queue_id);
    }

    /// Get the cached on_enqueue bytecode for a queue, if any.
    pub fn get_on_enqueue(&self, queue_id: &str) -> Option<&CompiledScript> {
        self.on_enqueue_cache.get(queue_id)
    }

    /// Cache a pre-compiled on_failure script for a queue.
    pub fn cache_on_failure(&mut self, queue_id: &str, bytecode: CompiledScript) {
        self.on_failure_cache.insert(queue_id.to_string(), bytecode);
    }

    /// Execute the on_enqueue script for a queue, returning the scheduling metadata.
    ///
    /// If no script is cached for the queue, returns None (caller should use defaults).
    /// If the circuit breaker is tripped, returns defaults with a warning.
    /// If execution fails, returns safe defaults, logs a warning, and updates
    /// the circuit breaker.
    pub fn run_on_enqueue(
        &mut self,
        queue_id: &str,
        headers: &std::collections::HashMap<String, String>,
        payload_size: usize,
        queue_name: &str,
    ) -> Option<(OnEnqueueResult, LuaExecOutcome)> {
        if !self.on_enqueue_cache.contains_key(queue_id) {
            return None;
        }

        // Circuit breaker check
        if !self.safety.should_execute(queue_id) {
            tracing::warn!(queue = %queue_id, "circuit breaker active, bypassing Lua");
            return Some((
                OnEnqueueResult::default(),
                LuaExecOutcome::CircuitBreakerBypassed,
            ));
        }

        // Set safety hooks before execution (fail-closed: always apply limits)
        let (instruction_limit, memory_limit_bytes) = if let Some(config) =
            self.safety.get_config(queue_id)
        {
            (config.instruction_limit, config.memory_limit_bytes)
        } else {
            tracing::warn!(queue = %queue_id, "safety config missing, applying global defaults");
            self.safety.default_limits()
        };
        safety::set_instruction_hook(&self.lua, instruction_limit);
        safety::set_memory_limit(&self.lua, memory_limit_bytes);

        let bytecode = self.on_enqueue_cache.get(queue_id).unwrap();
        let result =
            on_enqueue::try_run_on_enqueue(&self.lua, bytecode, headers, payload_size, queue_name);

        // Remove safety hooks after execution
        safety::remove_instruction_hook(&self.lua);
        safety::reset_memory_limit(&self.lua);

        // Record success/failure for circuit breaker
        match result {
            Ok(on_enqueue_result) => {
                self.safety.record_success(queue_id);
                Some((on_enqueue_result, LuaExecOutcome::Success))
            }
            Err(e) => {
                tracing::warn!(queue = %queue_id, error = %e, "on_enqueue script failed, using defaults");
                let just_tripped = self.safety.record_failure(queue_id);
                if just_tripped {
                    tracing::warn!(queue = %queue_id, "circuit breaker tripped");
                }
                Some((
                    OnEnqueueResult::default(),
                    LuaExecOutcome::ScriptError {
                        circuit_breaker_tripped: just_tripped,
                    },
                ))
            }
        }
    }

    /// Execute the on_failure script for a queue, returning the failure action.
    ///
    /// If no script is cached for the queue, returns None (caller uses default retry).
    /// If the circuit breaker is tripped, returns default retry.
    /// If execution fails, returns default retry, logs a warning, and updates
    /// the circuit breaker.
    pub fn run_on_failure(
        &mut self,
        queue_id: &str,
        headers: &std::collections::HashMap<String, String>,
        msg_id: &str,
        attempt_count: u32,
        queue_name: &str,
        error: &str,
    ) -> Option<(OnFailureResult, LuaExecOutcome)> {
        if !self.on_failure_cache.contains_key(queue_id) {
            return None;
        }

        // Circuit breaker check (shared with on_enqueue for this queue)
        if !self.safety.should_execute(queue_id) {
            tracing::warn!(queue = %queue_id, "circuit breaker active, bypassing on_failure Lua");
            return Some((
                OnFailureResult::default(),
                LuaExecOutcome::CircuitBreakerBypassed,
            ));
        }

        // Set safety hooks before execution (fail-closed: always apply limits)
        let (instruction_limit, memory_limit_bytes) = if let Some(config) =
            self.safety.get_config(queue_id)
        {
            (config.instruction_limit, config.memory_limit_bytes)
        } else {
            tracing::warn!(queue = %queue_id, "safety config missing for on_failure, applying global defaults");
            self.safety.default_limits()
        };
        safety::set_instruction_hook(&self.lua, instruction_limit);
        safety::set_memory_limit(&self.lua, memory_limit_bytes);

        let bytecode = self.on_failure_cache.get(queue_id).unwrap();
        let result = on_failure::try_run_on_failure(
            &self.lua,
            bytecode,
            headers,
            msg_id,
            attempt_count,
            queue_name,
            error,
        );

        // Remove safety hooks after execution
        safety::remove_instruction_hook(&self.lua);
        safety::reset_memory_limit(&self.lua);

        // Record success/failure for circuit breaker
        match result {
            Ok(on_failure_result) => {
                if on_failure_result.delay_ms > 0 {
                    tracing::warn!(
                        queue = %queue_id,
                        delay_ms = on_failure_result.delay_ms,
                        "on_failure requested delay_ms but delayed retry is not yet supported, retrying immediately"
                    );
                }
                self.safety.record_success(queue_id);
                Some((on_failure_result, LuaExecOutcome::Success))
            }
            Err(e) => {
                tracing::warn!(queue = %queue_id, error = %e, "on_failure script failed, using default retry");
                let just_tripped = self.safety.record_failure(queue_id);
                if just_tripped {
                    tracing::warn!(queue = %queue_id, "circuit breaker tripped");
                }
                Some((
                    OnFailureResult::default(),
                    LuaExecOutcome::ScriptError {
                        circuit_breaker_tripped: just_tripped,
                    },
                ))
            }
        }
    }

    /// Verify a script compiles successfully and can produce a function.
    ///
    /// Used at queue creation time to reject invalid scripts early.
    /// Only validates syntax (compilation) — does NOT execute the script,
    /// since execution without safety hooks would be a DoS vector.
    pub fn validate_script(&self, source: &str) -> Result<(), LuaError> {
        self.compile_script(source)?;
        Ok(())
    }
}

/// Internal Lua errors. Used for logging and for propagation within the lua module.
/// These are NOT exposed to callers of the broker — Lua failures fall back to defaults
/// for on_enqueue, and compilation errors are surfaced via CreateQueueError::LuaCompilation.
#[derive(Debug, thiserror::Error)]
pub enum LuaError {
    #[error("failed to create Lua VM: {0}")]
    VmCreation(mlua::Error),

    #[error("failed to register fila API bridge: {0}")]
    BridgeRegistration(mlua::Error),

    #[error("lua compilation error: {0}")]
    Compilation(mlua::Error),
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::storage::RocksDbStorage;

    fn test_engine() -> (LuaEngine, tempfile::TempDir) {
        let dir = tempfile::tempdir().unwrap();
        let storage = Arc::new(RocksDbStorage::open(dir.path()).unwrap());
        let lua_config = crate::broker::config::LuaConfig::default();
        let engine = LuaEngine::new(storage, &lua_config).unwrap();
        (engine, dir)
    }

    #[test]
    fn compile_and_cache_script() {
        let (mut engine, _dir) = test_engine();

        let bytecode = engine
            .compile_script(
                r#"
                function on_enqueue(msg)
                    return { fairness_key = "test" }
                end
            "#,
            )
            .unwrap();

        assert!(!bytecode.is_empty());
        engine.register_queue_safety("q1", None, None);
        engine.cache_on_enqueue("q1", bytecode);
        assert!(engine.get_on_enqueue("q1").is_some());
        assert!(engine.get_on_enqueue("q2").is_none());
    }

    #[test]
    fn compile_invalid_script_returns_error() {
        let (engine, _dir) = test_engine();

        let result = engine.compile_script("this is not valid lua %%%");
        assert!(result.is_err());
        assert!(matches!(result.unwrap_err(), LuaError::Compilation(_)));
    }

    #[test]
    fn remove_cached_script() {
        let (mut engine, _dir) = test_engine();

        let bytecode = engine
            .compile_script("function on_enqueue(msg) return { fairness_key = 'x' } end")
            .unwrap();
        engine.register_queue_safety("q1", None, None);
        engine.cache_on_enqueue("q1", bytecode);
        assert!(engine.get_on_enqueue("q1").is_some());

        engine.remove_queue_scripts("q1");
        assert!(engine.get_on_enqueue("q1").is_none());
    }

    #[test]
    fn run_on_enqueue_returns_none_without_cached_script() {
        let (mut engine, _dir) = test_engine();
        let result = engine.run_on_enqueue("q1", &HashMap::new(), 100, "test-queue");
        assert!(result.is_none());
    }

    #[test]
    fn run_on_enqueue_executes_cached_script() {
        let (mut engine, _dir) = test_engine();

        let bytecode = engine
            .compile_script(
                r#"
                function on_enqueue(msg)
                    return {
                        fairness_key = msg.headers["tenant"] or "default",
                        weight = 2,
                    }
                end
            "#,
            )
            .unwrap();
        engine.register_queue_safety("q1", None, None);
        engine.cache_on_enqueue("q1", bytecode);

        let mut headers = HashMap::new();
        headers.insert("tenant".to_string(), "acme".to_string());

        let (result, outcome) = engine
            .run_on_enqueue("q1", &headers, 100, "test-queue")
            .unwrap();
        assert_eq!(result.fairness_key, "acme");
        assert_eq!(result.weight, 2);
        assert_eq!(outcome, LuaExecOutcome::Success);
    }

    #[test]
    fn run_on_enqueue_applies_safety_limits_without_per_queue_config() {
        let (mut engine, _dir) = test_engine();

        // An infinite loop script — should be killed by instruction limit
        let bytecode = engine
            .compile_script(
                r#"
                function on_enqueue(msg)
                    while true do end
                end
            "#,
            )
            .unwrap();

        // Insert bytecode directly into the cache WITHOUT registering safety config,
        // simulating the missing-config scenario. The engine should still apply
        // global default limits and abort the script (fail-closed).
        engine.on_enqueue_cache.insert("q1".to_string(), bytecode);

        let (result, outcome) = engine
            .run_on_enqueue("q1", &HashMap::new(), 100, "test-queue")
            .unwrap();
        // Should return defaults (script failed due to instruction limit), not hang
        assert_eq!(result, OnEnqueueResult::default());
        assert!(matches!(outcome, LuaExecOutcome::ScriptError { .. }));
    }

    #[test]
    fn validate_script_accepts_valid_source() {
        let (engine, _dir) = test_engine();
        engine
            .validate_script("function on_enqueue(msg) return { fairness_key = 'x' } end")
            .unwrap();
    }

    #[test]
    fn validate_script_rejects_invalid_source() {
        let (engine, _dir) = test_engine();
        let result = engine.validate_script("not valid lua %%%");
        assert!(result.is_err());
    }
}
