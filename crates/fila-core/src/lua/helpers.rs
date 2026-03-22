use mlua::Lua;

/// Register the `fila.helpers` module on the existing `fila` global table.
///
/// Provides convenience functions for common Lua script patterns:
/// - `fila.helpers.exponential_backoff(attempts, base_ms, max_ms)` — delay with jitter
/// - `fila.helpers.tenant_route(msg, header_name)` — extract fairness key from header
/// - `fila.helpers.rate_limit_keys(msg, patterns)` — generate throttle keys from patterns
/// - `fila.helpers.max_retries(attempts, max)` — retry-or-dlq decision
///
/// Must be called AFTER `register_fila_api` since it reads the existing `fila` table.
pub fn register_helpers(lua: &Lua) -> mlua::Result<()> {
    let fila_table: mlua::Table = lua.globals().get("fila")?;
    let helpers_table = lua.create_table()?;

    helpers_table.set("exponential_backoff", create_exponential_backoff(lua)?)?;
    helpers_table.set("tenant_route", create_tenant_route(lua)?)?;
    helpers_table.set("rate_limit_keys", create_rate_limit_keys(lua)?)?;
    helpers_table.set("max_retries", create_max_retries(lua)?)?;

    fila_table.set("helpers", helpers_table)?;
    Ok(())
}

/// `fila.helpers.exponential_backoff(attempts, base_ms, max_ms)` → number
///
/// Computes exponential backoff delay with ±25% jitter.
/// - `attempts <= 0` → returns `base_ms` with jitter
/// - Delay = `base_ms * 2^(attempts-1)`, capped at `max_ms`
/// - Jitter: multiply by random factor in `[0.75, 1.25]`
fn create_exponential_backoff(lua: &Lua) -> mlua::Result<mlua::Function> {
    lua.create_function(|lua, (attempts, base_ms, max_ms): (f64, f64, f64)| {
        let exponent = (attempts - 1.0).max(0.0).min(52.0); // cap to avoid f64 overflow
        let delay = (base_ms * 2.0_f64.powf(exponent)).min(max_ms);

        // Use Lua's math.random() for jitter (available in sandbox)
        let random: f64 = lua.load("math.random()").eval()?;
        let jitter_factor = 0.75 + random * 0.5; // [0.75, 1.25]
        let result = (delay * jitter_factor).floor();

        Ok(result)
    })
}

/// `fila.helpers.tenant_route(msg, header_name)` → table
///
/// Returns `{ fairness_key = msg.headers[header_name] }` or
/// `{ fairness_key = "default" }` if the header is missing.
fn create_tenant_route(lua: &Lua) -> mlua::Result<mlua::Function> {
    lua.create_function(|lua, (msg, header_name): (mlua::Table, String)| {
        let fairness_key = msg
            .get::<mlua::Table>("headers")
            .ok()
            .and_then(|headers| headers.get::<String>(header_name.as_str()).ok())
            .unwrap_or_else(|| "default".to_string());

        let result = lua.create_table()?;
        result.set("fairness_key", fairness_key)?;
        Ok(result)
    })
}

/// `fila.helpers.rate_limit_keys(msg, patterns)` → table (array of strings)
///
/// Each pattern is a string like `"provider:{provider_id}"`. Placeholders
/// in `{braces}` are replaced with the corresponding `msg.headers` value.
/// Patterns referencing missing headers are omitted from the result.
fn create_rate_limit_keys(lua: &Lua) -> mlua::Result<mlua::Function> {
    lua.create_function(|lua, (msg, patterns): (mlua::Table, mlua::Table)| {
        let result = lua.create_table()?;

        // If msg.headers is missing/nil, return empty array (no headers to substitute)
        let headers = match msg.get::<mlua::Table>("headers") {
            Ok(h) => h,
            Err(_) => return Ok(result),
        };

        let mut index = 1;

        for entry in patterns.sequence_values::<String>() {
            let pattern = entry?;
            let mut resolved = pattern.clone();
            let mut all_resolved = true;

            // Find all {placeholder} occurrences and replace with header values
            let mut search_from = 0;
            loop {
                let Some(start) = resolved[search_from..].find('{') else {
                    break;
                };
                let start = start + search_from;
                let Some(end) = resolved[start..].find('}') else {
                    break;
                };
                let end = end + start;

                let key = &resolved[start + 1..end];
                match headers.get::<String>(key) {
                    Ok(value) => {
                        let before = &resolved[..start];
                        let after = &resolved[end + 1..];
                        resolved = format!("{before}{value}{after}");
                        search_from = start + value.len();
                    }
                    Err(_) => {
                        all_resolved = false;
                        break;
                    }
                }
            }

            if all_resolved {
                result.set(index, resolved)?;
                index += 1;
            }
        }

        Ok(result)
    })
}

/// `fila.helpers.max_retries(attempts, max)` → table
///
/// Returns `{ action = "retry" }` if `attempts < max`,
/// otherwise `{ action = "dlq" }`.
fn create_max_retries(lua: &Lua) -> mlua::Result<mlua::Function> {
    lua.create_function(|lua, (attempts, max): (i64, i64)| {
        let action = if attempts < max { "retry" } else { "dlq" };
        let result = lua.create_table()?;
        result.set("action", action)?;
        Ok(result)
    })
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use crate::storage::RocksDbEngine;

    use super::*;

    /// Create a sandboxed Lua VM with fila API and helpers registered.
    fn test_lua() -> (Lua, tempfile::TempDir) {
        let dir = tempfile::tempdir().unwrap();
        let storage = Arc::new(RocksDbEngine::open(dir.path()).unwrap());
        let lua = crate::lua::sandbox::create_sandbox().unwrap();
        crate::lua::bridge::register_fila_api(&lua, storage).unwrap();
        register_helpers(&lua).unwrap();
        (lua, dir)
    }

    // -- exponential_backoff tests --

    #[test]
    fn exponential_backoff_basic() {
        let (lua, _dir) = test_lua();
        let delay: f64 = lua
            .load("return fila.helpers.exponential_backoff(1, 1000, 60000)")
            .eval()
            .unwrap();
        // attempts=1 → base_ms * 2^0 = 1000, with jitter [750, 1250]
        assert!(delay >= 750.0 && delay <= 1250.0, "delay={delay}");
    }

    #[test]
    fn exponential_backoff_grows() {
        let (lua, _dir) = test_lua();
        // Run multiple times to ensure the computed base is correct despite jitter
        // attempts=3 → 1000 * 2^2 = 4000, jitter [3000, 5000]
        let delay: f64 = lua
            .load("return fila.helpers.exponential_backoff(3, 1000, 60000)")
            .eval()
            .unwrap();
        assert!(delay >= 3000.0 && delay <= 5000.0, "delay={delay}");
    }

    #[test]
    fn exponential_backoff_caps_at_max() {
        let (lua, _dir) = test_lua();
        // attempts=10 → 1000 * 2^9 = 512000, but max=60000 → capped at 60000, jitter [45000, 75000]
        let delay: f64 = lua
            .load("return fila.helpers.exponential_backoff(10, 1000, 60000)")
            .eval()
            .unwrap();
        assert!(delay >= 45000.0 && delay <= 75000.0, "delay={delay}");
    }

    #[test]
    fn exponential_backoff_zero_attempts() {
        let (lua, _dir) = test_lua();
        // attempts=0 → exponent = max(-1, 0) = 0 → base_ms * 1 = 1000, jitter [750, 1250]
        let delay: f64 = lua
            .load("return fila.helpers.exponential_backoff(0, 1000, 60000)")
            .eval()
            .unwrap();
        assert!(delay >= 750.0 && delay <= 1250.0, "delay={delay}");
    }

    #[test]
    fn exponential_backoff_large_exponent_no_overflow() {
        let (lua, _dir) = test_lua();
        // attempts=100 → exponent capped at 52, delay capped at max_ms
        let delay: f64 = lua
            .load("return fila.helpers.exponential_backoff(100, 1000, 60000)")
            .eval()
            .unwrap();
        assert!(delay >= 45000.0 && delay <= 75000.0, "delay={delay}");
    }

    // -- tenant_route tests --

    #[test]
    fn tenant_route_present_header() {
        let (lua, _dir) = test_lua();
        let key: String = lua
            .load(
                r#"
                local msg = { headers = { tenant_id = "acme" } }
                local result = fila.helpers.tenant_route(msg, "tenant_id")
                return result.fairness_key
                "#,
            )
            .eval()
            .unwrap();
        assert_eq!(key, "acme");
    }

    #[test]
    fn tenant_route_missing_header() {
        let (lua, _dir) = test_lua();
        let key: String = lua
            .load(
                r#"
                local msg = { headers = {} }
                local result = fila.helpers.tenant_route(msg, "tenant_id")
                return result.fairness_key
                "#,
            )
            .eval()
            .unwrap();
        assert_eq!(key, "default");
    }

    #[test]
    fn tenant_route_nil_headers() {
        let (lua, _dir) = test_lua();
        let key: String = lua
            .load(
                r#"
                local msg = {}
                local result = fila.helpers.tenant_route(msg, "tenant_id")
                return result.fairness_key
                "#,
            )
            .eval()
            .unwrap();
        assert_eq!(key, "default");
    }

    // -- rate_limit_keys tests --

    #[test]
    fn rate_limit_keys_valid_patterns() {
        let (lua, _dir) = test_lua();
        let result: Vec<String> = lua
            .load(
                r#"
                local msg = { headers = { provider = "aws", region = "us-east-1" } }
                local keys = fila.helpers.rate_limit_keys(msg, {"provider:{provider}", "region:{region}"})
                return keys
                "#,
            )
            .eval()
            .unwrap();
        assert_eq!(result, vec!["provider:aws", "region:us-east-1"]);
    }

    #[test]
    fn rate_limit_keys_missing_header_omitted() {
        let (lua, _dir) = test_lua();
        let result: Vec<String> = lua
            .load(
                r#"
                local msg = { headers = { provider = "aws" } }
                local keys = fila.helpers.rate_limit_keys(msg, {"provider:{provider}", "region:{region}"})
                return keys
                "#,
            )
            .eval()
            .unwrap();
        assert_eq!(result, vec!["provider:aws"]);
    }

    #[test]
    fn rate_limit_keys_empty_patterns() {
        let (lua, _dir) = test_lua();
        let result: Vec<String> = lua
            .load(
                r#"
                local msg = { headers = { provider = "aws" } }
                local keys = fila.helpers.rate_limit_keys(msg, {})
                return keys
                "#,
            )
            .eval()
            .unwrap();
        assert!(result.is_empty());
    }

    #[test]
    fn rate_limit_keys_nil_headers() {
        let (lua, _dir) = test_lua();
        let result: Vec<String> = lua
            .load(
                r#"
                local msg = {}
                local keys = fila.helpers.rate_limit_keys(msg, {"provider:{provider}"})
                return keys
                "#,
            )
            .eval()
            .unwrap();
        assert!(result.is_empty());
    }

    #[test]
    fn rate_limit_keys_no_placeholders() {
        let (lua, _dir) = test_lua();
        let result: Vec<String> = lua
            .load(
                r#"
                local msg = { headers = {} }
                local keys = fila.helpers.rate_limit_keys(msg, {"static-key"})
                return keys
                "#,
            )
            .eval()
            .unwrap();
        assert_eq!(result, vec!["static-key"]);
    }

    // -- max_retries tests --

    #[test]
    fn max_retries_below_max() {
        let (lua, _dir) = test_lua();
        let action: String = lua
            .load(
                r#"
                local result = fila.helpers.max_retries(2, 5)
                return result.action
                "#,
            )
            .eval()
            .unwrap();
        assert_eq!(action, "retry");
    }

    #[test]
    fn max_retries_at_max() {
        let (lua, _dir) = test_lua();
        let action: String = lua
            .load(
                r#"
                local result = fila.helpers.max_retries(5, 5)
                return result.action
                "#,
            )
            .eval()
            .unwrap();
        assert_eq!(action, "dlq");
    }

    #[test]
    fn max_retries_above_max() {
        let (lua, _dir) = test_lua();
        let action: String = lua
            .load(
                r#"
                local result = fila.helpers.max_retries(10, 5)
                return result.action
                "#,
            )
            .eval()
            .unwrap();
        assert_eq!(action, "dlq");
    }

    #[test]
    fn max_retries_zero_max() {
        let (lua, _dir) = test_lua();
        let action: String = lua
            .load(
                r#"
                local result = fila.helpers.max_retries(0, 0)
                return result.action
                "#,
            )
            .eval()
            .unwrap();
        assert_eq!(action, "dlq");
    }

    // -- Integration test: helpers coexist with fila.get() --

    #[test]
    fn helpers_alongside_fila_get() {
        let (lua, _dir) = test_lua();
        // Verify both fila.get() and fila.helpers work together
        let result: mlua::Value = lua
            .load(
                r#"
                local val = fila.get("nonexistent")
                local retry = fila.helpers.max_retries(1, 3)
                return retry.action
                "#,
            )
            .eval()
            .unwrap();
        assert_eq!(
            result,
            mlua::Value::String(lua.create_string("retry").unwrap())
        );
    }
}
