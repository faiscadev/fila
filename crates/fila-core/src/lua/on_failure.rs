use std::collections::HashMap;

use mlua::{ChunkMode, Lua, Table};

/// Action to take when a message fails.
#[derive(Debug, Clone, PartialEq)]
pub enum FailureAction {
    /// Retry the message immediately (or with delay, when supported).
    Retry,
    /// Move the message to the dead-letter queue.
    DeadLetter,
}

/// Result of executing an on_failure Lua script.
#[derive(Debug, Clone, PartialEq)]
pub struct OnFailureResult {
    pub action: FailureAction,
    /// Requested delay in milliseconds before retry. Logged as warning if > 0
    /// (delayed retry not yet supported).
    pub delay_ms: u64,
}

impl Default for OnFailureResult {
    fn default() -> Self {
        Self {
            action: FailureAction::Retry,
            delay_ms: 0,
        }
    }
}

/// Execute a pre-compiled on_failure script, returning a Result.
///
/// The script receives a `msg` table with:
/// - `msg.headers`: read-only table of string key-value pairs
/// - `msg.id`: string (message UUID)
/// - `msg.attempts`: number (current attempt count, already incremented)
/// - `msg.queue`: string (queue name)
/// - `msg.error`: string (error description from nack)
///
/// The script must return a table with:
/// - `action`: string ("retry" or "dlq", defaults to "retry")
/// - `delay_ms`: number (optional, defaults to 0)
pub fn try_run_on_failure(
    lua: &Lua,
    bytecode: &[u8],
    headers: &HashMap<String, String>,
    msg_id: &str,
    attempt_count: u32,
    queue_name: &str,
    error: &str,
) -> mlua::Result<OnFailureResult> {
    // Load the pre-compiled bytecode as a chunk that defines on_failure.
    // The bytecode sets a global `on_failure` function. We nil it after use
    // to prevent stale globals from leaking between queue invocations.
    lua.load(bytecode).set_mode(ChunkMode::Binary).exec()?;

    // Get the on_failure function from globals and nil the global to prevent leakage.
    // Nil first so the global is cleaned even if the type cast to Function fails.
    let on_failure_val = lua.globals().get::<mlua::Value>("on_failure")?;
    lua.globals().set("on_failure", mlua::Value::Nil)?;
    let on_failure: mlua::Function = mlua::FromLua::from_lua(on_failure_val, lua)?;

    // Build the msg input table
    let msg = lua.create_table()?;

    let headers_table = lua.create_table()?;
    for (k, v) in headers {
        headers_table.set(k.as_str(), v.as_str())?;
    }
    msg.set("headers", headers_table)?;
    msg.set("id", msg_id)?;
    msg.set("attempts", attempt_count)?;
    msg.set("queue", queue_name)?;
    msg.set("error", error)?;

    // Call the script
    let result: Table = on_failure.call(msg)?;

    // Parse the output table with safe fallbacks
    let action_str: String = result
        .get::<String>("action")
        .unwrap_or_else(|_| "retry".to_string());

    let action = match action_str.as_str() {
        "retry" => FailureAction::Retry,
        "dlq" => FailureAction::DeadLetter,
        other => {
            tracing::warn!(action = %other, "unknown on_failure action, defaulting to retry");
            FailureAction::Retry
        }
    };

    let delay_ms: u64 = result
        .get::<i64>("delay_ms")
        .ok()
        .and_then(|d| u64::try_from(d).ok())
        .unwrap_or(0);

    Ok(OnFailureResult { action, delay_ms })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::lua::sandbox::create_sandbox;

    fn compile_script(lua: &Lua, source: &str) -> Vec<u8> {
        let func = lua
            .load(source)
            .set_name("on_failure_script")
            .into_function()
            .expect("compile script");
        func.dump(true)
    }

    #[test]
    fn on_failure_returns_retry_by_default() {
        let lua = create_sandbox().unwrap();
        let bytecode = compile_script(
            &lua,
            r#"
            function on_failure(msg)
                return { action = "retry" }
            end
        "#,
        );

        let result = try_run_on_failure(
            &lua,
            &bytecode,
            &HashMap::new(),
            "msg-123",
            1,
            "test-queue",
            "processing failed",
        )
        .unwrap();
        assert_eq!(result.action, FailureAction::Retry);
        assert_eq!(result.delay_ms, 0);
    }

    #[test]
    fn on_failure_returns_dlq_action() {
        let lua = create_sandbox().unwrap();
        let bytecode = compile_script(
            &lua,
            r#"
            function on_failure(msg)
                if msg.attempts >= 3 then
                    return { action = "dlq" }
                end
                return { action = "retry" }
            end
        "#,
        );

        // With 2 attempts — should retry
        let result = try_run_on_failure(
            &lua,
            &bytecode,
            &HashMap::new(),
            "msg-123",
            2,
            "test-queue",
            "error",
        )
        .unwrap();
        assert_eq!(result.action, FailureAction::Retry);

        // With 3 attempts — should DLQ
        let result = try_run_on_failure(
            &lua,
            &bytecode,
            &HashMap::new(),
            "msg-123",
            3,
            "test-queue",
            "error",
        )
        .unwrap();
        assert_eq!(result.action, FailureAction::DeadLetter);
    }

    #[test]
    fn on_failure_returns_delay_ms() {
        let lua = create_sandbox().unwrap();
        let bytecode = compile_script(
            &lua,
            r#"
            function on_failure(msg)
                return { action = "retry", delay_ms = 5000 }
            end
        "#,
        );

        let result = try_run_on_failure(
            &lua,
            &bytecode,
            &HashMap::new(),
            "msg-123",
            1,
            "test-queue",
            "error",
        )
        .unwrap();
        assert_eq!(result.action, FailureAction::Retry);
        assert_eq!(result.delay_ms, 5000);
    }

    #[test]
    fn on_failure_reads_error_and_headers() {
        let lua = create_sandbox().unwrap();
        let bytecode = compile_script(
            &lua,
            r#"
            function on_failure(msg)
                if msg.error == "permanent" then
                    return { action = "dlq" }
                end
                if msg.headers["retry_policy"] == "never" then
                    return { action = "dlq" }
                end
                return { action = "retry" }
            end
        "#,
        );

        // Error triggers DLQ
        let result = try_run_on_failure(
            &lua,
            &bytecode,
            &HashMap::new(),
            "msg-123",
            1,
            "test-queue",
            "permanent",
        )
        .unwrap();
        assert_eq!(result.action, FailureAction::DeadLetter);

        // Header triggers DLQ
        let mut headers = HashMap::new();
        headers.insert("retry_policy".to_string(), "never".to_string());
        let result = try_run_on_failure(
            &lua,
            &bytecode,
            &headers,
            "msg-123",
            1,
            "test-queue",
            "transient",
        )
        .unwrap();
        assert_eq!(result.action, FailureAction::DeadLetter);

        // Normal error with no special header — retry
        let result = try_run_on_failure(
            &lua,
            &bytecode,
            &HashMap::new(),
            "msg-123",
            1,
            "test-queue",
            "transient",
        )
        .unwrap();
        assert_eq!(result.action, FailureAction::Retry);
    }

    #[test]
    fn on_failure_defaults_on_missing_action() {
        let lua = create_sandbox().unwrap();
        let bytecode = compile_script(
            &lua,
            r#"
            function on_failure(msg)
                return { delay_ms = 100 }
            end
        "#,
        );

        let result = try_run_on_failure(
            &lua,
            &bytecode,
            &HashMap::new(),
            "msg-123",
            1,
            "test-queue",
            "error",
        )
        .unwrap();
        // Missing action defaults to retry
        assert_eq!(result.action, FailureAction::Retry);
        assert_eq!(result.delay_ms, 100);
    }

    #[test]
    fn on_failure_falls_back_on_script_error() {
        let lua = create_sandbox().unwrap();
        let bytecode = compile_script(
            &lua,
            r#"
            function on_failure(msg)
                error("intentional failure")
            end
        "#,
        );

        let result = try_run_on_failure(
            &lua,
            &bytecode,
            &HashMap::new(),
            "msg-123",
            1,
            "test-queue",
            "error",
        );
        assert!(result.is_err());
    }

    #[test]
    fn on_failure_reads_msg_id_and_queue() {
        let lua = create_sandbox().unwrap();
        let bytecode = compile_script(
            &lua,
            r#"
            function on_failure(msg)
                if msg.id == "special-id" and msg.queue == "important" then
                    return { action = "dlq" }
                end
                return { action = "retry" }
            end
        "#,
        );

        let result = try_run_on_failure(
            &lua,
            &bytecode,
            &HashMap::new(),
            "special-id",
            1,
            "important",
            "error",
        )
        .unwrap();
        assert_eq!(result.action, FailureAction::DeadLetter);

        let result = try_run_on_failure(
            &lua,
            &bytecode,
            &HashMap::new(),
            "other-id",
            1,
            "important",
            "error",
        )
        .unwrap();
        assert_eq!(result.action, FailureAction::Retry);
    }
}
