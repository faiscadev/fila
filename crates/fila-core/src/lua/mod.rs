pub mod bridge;
pub mod on_enqueue;
pub mod sandbox;

use std::collections::HashMap;
use std::sync::Arc;

use mlua::{ChunkMode, Lua};

use crate::storage::Storage;

pub use on_enqueue::OnEnqueueResult;

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
}

impl LuaEngine {
    /// Create a new LuaEngine with a sandboxed Lua VM and fila.get() bridge.
    pub fn new(storage: Arc<dyn Storage>) -> Result<Self, LuaError> {
        let lua = sandbox::create_sandbox().map_err(LuaError::VmCreation)?;
        bridge::register_fila_api(&lua, storage).map_err(LuaError::BridgeRegistration)?;

        Ok(Self {
            lua,
            on_enqueue_cache: HashMap::new(),
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
            .set_name("on_enqueue")
            .into_function()
            .map_err(LuaError::Compilation)?;

        Ok(func.dump(true))
    }

    /// Cache a pre-compiled on_enqueue script for a queue.
    pub fn cache_on_enqueue(&mut self, queue_id: &str, bytecode: CompiledScript) {
        self.on_enqueue_cache.insert(queue_id.to_string(), bytecode);
    }

    /// Remove a cached on_enqueue script for a queue.
    pub fn remove_on_enqueue(&mut self, queue_id: &str) {
        self.on_enqueue_cache.remove(queue_id);
    }

    /// Get the cached on_enqueue bytecode for a queue, if any.
    pub fn get_on_enqueue(&self, queue_id: &str) -> Option<&CompiledScript> {
        self.on_enqueue_cache.get(queue_id)
    }

    /// Execute the on_enqueue script for a queue, returning the scheduling metadata.
    ///
    /// If no script is cached for the queue, returns None (caller should use defaults).
    /// If execution fails, returns safe defaults and logs a warning.
    pub fn run_on_enqueue(
        &self,
        queue_id: &str,
        headers: &std::collections::HashMap<String, String>,
        payload_size: usize,
        queue_name: &str,
    ) -> Option<OnEnqueueResult> {
        let bytecode = self.on_enqueue_cache.get(queue_id)?;
        Some(on_enqueue::run_on_enqueue(
            &self.lua,
            bytecode,
            headers,
            payload_size,
            queue_name,
        ))
    }

    /// Verify a script compiles successfully and can produce a function.
    ///
    /// Used at queue creation time to reject invalid scripts early.
    pub fn validate_script(&self, source: &str) -> Result<(), LuaError> {
        // Compile to bytecode, then verify it can be loaded back
        let bytecode = self.compile_script(source)?;
        self.lua
            .load(bytecode.as_slice())
            .set_mode(ChunkMode::Binary)
            .exec()
            .map_err(LuaError::Compilation)?;
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
        let engine = LuaEngine::new(storage).unwrap();
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
        engine.cache_on_enqueue("q1", bytecode);
        assert!(engine.get_on_enqueue("q1").is_some());

        engine.remove_on_enqueue("q1");
        assert!(engine.get_on_enqueue("q1").is_none());
    }

    #[test]
    fn run_on_enqueue_returns_none_without_cached_script() {
        let (engine, _dir) = test_engine();
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
        engine.cache_on_enqueue("q1", bytecode);

        let mut headers = HashMap::new();
        headers.insert("tenant".to_string(), "acme".to_string());

        let result = engine
            .run_on_enqueue("q1", &headers, 100, "test-queue")
            .unwrap();
        assert_eq!(result.fairness_key, "acme");
        assert_eq!(result.weight, 2);
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
