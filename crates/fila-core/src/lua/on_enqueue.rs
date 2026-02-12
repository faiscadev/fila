use std::collections::HashMap;

use mlua::{ChunkMode, Lua, Table};

/// Result of executing an on_enqueue Lua script.
#[derive(Debug, Clone, PartialEq)]
pub struct OnEnqueueResult {
    pub fairness_key: String,
    pub weight: u32,
    pub throttle_keys: Vec<String>,
}

impl Default for OnEnqueueResult {
    fn default() -> Self {
        Self {
            fairness_key: "default".to_string(),
            weight: 1,
            throttle_keys: Vec::new(),
        }
    }
}

/// Execute a pre-compiled on_enqueue script with the given message context.
///
/// The script receives a `msg` table with:
/// - `msg.headers`: read-only table of string key-value pairs
/// - `msg.payload_size`: number (byte count)
/// - `msg.queue`: string (queue name)
///
/// The script must return a table with:
/// - `fairness_key`: string (required, falls back to "default")
/// - `weight`: number (optional, defaults to 1)
/// - `throttle_keys`: array of strings (optional, defaults to empty)
pub fn run_on_enqueue(
    lua: &Lua,
    bytecode: &[u8],
    headers: &HashMap<String, String>,
    payload_size: usize,
    queue_name: &str,
) -> OnEnqueueResult {
    match try_run_on_enqueue(lua, bytecode, headers, payload_size, queue_name) {
        Ok(result) => result,
        Err(e) => {
            tracing::warn!(queue = %queue_name, error = %e, "on_enqueue script failed, using defaults");
            OnEnqueueResult::default()
        }
    }
}

/// Execute a pre-compiled on_enqueue script, returning a Result.
///
/// Unlike `run_on_enqueue`, this function does not catch errors — the caller
/// is responsible for handling failures (e.g., circuit breaker tracking).
pub fn try_run_on_enqueue(
    lua: &Lua,
    bytecode: &[u8],
    headers: &HashMap<String, String>,
    payload_size: usize,
    queue_name: &str,
) -> mlua::Result<OnEnqueueResult> {
    // Load the pre-compiled bytecode as a chunk that defines on_enqueue.
    // The bytecode sets a global `on_enqueue` function. We nil it after use
    // to prevent stale globals from leaking between queue invocations.
    lua.load(bytecode).set_mode(ChunkMode::Binary).exec()?;

    // Get the on_enqueue function from globals and immediately nil the global
    let on_enqueue: mlua::Function = lua.globals().get("on_enqueue")?;
    lua.globals().set("on_enqueue", mlua::Value::Nil)?;

    // Build the msg input table
    let msg = lua.create_table()?;

    let headers_table = lua.create_table()?;
    for (k, v) in headers {
        headers_table.set(k.as_str(), v.as_str())?;
    }
    msg.set("headers", headers_table)?;
    msg.set("payload_size", payload_size as i64)?;
    msg.set("queue", queue_name)?;

    // Call the script
    let result: Table = on_enqueue.call(msg)?;

    // Parse the output table with safe fallbacks
    let fairness_key: String = result
        .get::<String>("fairness_key")
        .unwrap_or_else(|_| {
            tracing::warn!(queue = %queue_name, "on_enqueue: missing or invalid fairness_key, using default");
            "default".to_string()
        });

    let weight: u32 = result
        .get::<i64>("weight")
        .ok()
        .and_then(|w| u32::try_from(w).ok())
        .unwrap_or(1);

    let throttle_keys: Vec<String> = result
        .get::<Table>("throttle_keys")
        .ok()
        .map(|t| {
            t.sequence_values::<String>()
                .filter_map(|r| r.ok())
                .collect()
        })
        .unwrap_or_default();

    Ok(OnEnqueueResult {
        fairness_key,
        weight,
        throttle_keys,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::lua::sandbox::create_sandbox;

    fn compile_script(lua: &Lua, source: &str) -> Vec<u8> {
        // Load the source to define the function, then dump
        let func = lua
            .load(source)
            .set_name("on_enqueue_script")
            .into_function()
            .expect("compile script");
        func.dump(true)
    }

    #[test]
    fn on_enqueue_assigns_fairness_key_from_header() {
        let lua = create_sandbox().unwrap();
        let bytecode = compile_script(
            &lua,
            r#"
            function on_enqueue(msg)
                return {
                    fairness_key = msg.headers["tenant_id"] or "default",
                }
            end
        "#,
        );

        let mut headers = HashMap::new();
        headers.insert("tenant_id".to_string(), "acme".to_string());

        let result = run_on_enqueue(&lua, &bytecode, &headers, 100, "test-queue");
        assert_eq!(result.fairness_key, "acme");
        assert_eq!(result.weight, 1);
        assert!(result.throttle_keys.is_empty());
    }

    #[test]
    fn on_enqueue_returns_weight_and_throttle_keys() {
        let lua = create_sandbox().unwrap();
        let bytecode = compile_script(
            &lua,
            r#"
            function on_enqueue(msg)
                local tenant = msg.headers["tenant_id"] or "default"
                return {
                    fairness_key = tenant,
                    weight = 3,
                    throttle_keys = { "tenant:" .. tenant, "global" },
                }
            end
        "#,
        );

        let mut headers = HashMap::new();
        headers.insert("tenant_id".to_string(), "acme".to_string());

        let result = run_on_enqueue(&lua, &bytecode, &headers, 100, "test-queue");
        assert_eq!(result.fairness_key, "acme");
        assert_eq!(result.weight, 3);
        assert_eq!(result.throttle_keys, vec!["tenant:acme", "global"]);
    }

    #[test]
    fn on_enqueue_uses_defaults_for_missing_fields() {
        let lua = create_sandbox().unwrap();
        let bytecode = compile_script(
            &lua,
            r#"
            function on_enqueue(msg)
                return { fairness_key = "tenant-a" }
            end
        "#,
        );

        let result = run_on_enqueue(&lua, &bytecode, &HashMap::new(), 100, "test-queue");
        assert_eq!(result.fairness_key, "tenant-a");
        assert_eq!(result.weight, 1);
        assert!(result.throttle_keys.is_empty());
    }

    #[test]
    fn on_enqueue_falls_back_on_script_error() {
        let lua = create_sandbox().unwrap();
        let bytecode = compile_script(
            &lua,
            r#"
            function on_enqueue(msg)
                error("intentional failure")
            end
        "#,
        );

        let result = run_on_enqueue(&lua, &bytecode, &HashMap::new(), 100, "test-queue");
        assert_eq!(result, OnEnqueueResult::default());
    }

    #[test]
    fn on_enqueue_reads_payload_size_and_queue() {
        let lua = create_sandbox().unwrap();
        let bytecode = compile_script(
            &lua,
            r#"
            function on_enqueue(msg)
                local key = msg.queue
                if msg.payload_size > 1000 then
                    key = key .. ":large"
                end
                return { fairness_key = key, weight = 1 }
            end
        "#,
        );

        let result = run_on_enqueue(&lua, &bytecode, &HashMap::new(), 2048, "orders");
        assert_eq!(result.fairness_key, "orders:large");

        let result_small = run_on_enqueue(&lua, &bytecode, &HashMap::new(), 500, "orders");
        assert_eq!(result_small.fairness_key, "orders");
    }

    #[test]
    fn on_enqueue_handles_invalid_return_types() {
        let lua = create_sandbox().unwrap();
        // Script returns wrong types — weight as string, throttle_keys as number
        let bytecode = compile_script(
            &lua,
            r#"
            function on_enqueue(msg)
                return {
                    fairness_key = "ok",
                    weight = "not_a_number",
                    throttle_keys = 42,
                }
            end
        "#,
        );

        let result = run_on_enqueue(&lua, &bytecode, &HashMap::new(), 100, "test-queue");
        assert_eq!(result.fairness_key, "ok");
        assert_eq!(result.weight, 1); // fallback
        assert!(result.throttle_keys.is_empty()); // fallback
    }
}
