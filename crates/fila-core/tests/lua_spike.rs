//! Technical spike: evaluate mlua capabilities for the Fila Lua rules engine.
//!
//! Each test verifies one capability we need. Run with:
//!   cargo test -p fila-core --test lua_spike

use mlua::{ChunkMode, Error, HookTriggers, Lua, LuaOptions, StdLib, Table, VmState};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

// ---------------------------------------------------------------------------
// 1. Bytecode pre-compilation (FR25)
// ---------------------------------------------------------------------------
// Compile a Lua script to bytecode once, then execute the compiled bytecode
// multiple times without re-parsing.

#[test]
fn bytecode_precompilation() {
    let lua = Lua::new();

    // Compile source → Function → bytecode
    let func = lua
        .load(
            r#"
            local x = ...
            return x * 2
        "#,
        )
        .set_name("double")
        .into_function()
        .expect("compile to function");

    let bytecode = func.dump(true); // strip debug info
    assert!(!bytecode.is_empty(), "bytecode should not be empty");
    assert_eq!(
        bytecode[0], 0x1b,
        "bytecode should start with Lua binary signature"
    );

    // Execute the bytecode multiple times without re-parsing
    for i in 1..=5 {
        let result: i64 = lua
            .load(bytecode.as_slice())
            .set_mode(ChunkMode::Binary)
            .call(i)
            .unwrap_or_else(|e| panic!("execution {i} failed: {e}"));
        assert_eq!(result, i * 2, "iteration {i}");
    }

    // Verify Binary mode rejects source code
    let err = lua
        .load("return 1")
        .set_mode(ChunkMode::Binary)
        .exec()
        .unwrap_err();
    assert!(
        matches!(err, Error::SyntaxError { .. }),
        "expected SyntaxError for source in Binary mode, got: {err}"
    );

    // Security-critical: verify Text mode rejects bytecode (prevents bytecode
    // injection attacks that could escape sandboxes)
    let err = lua
        .load(bytecode.as_slice())
        .set_mode(ChunkMode::Text)
        .exec()
        .unwrap_err();
    assert!(
        matches!(err, Error::SyntaxError { .. }),
        "expected SyntaxError for bytecode in Text mode, got: {err}"
    );
}

// ---------------------------------------------------------------------------
// 2. Instruction count timeout (FR23)
// ---------------------------------------------------------------------------
// Set an instruction count hook that aborts execution if a script runs too
// long (e.g., infinite loop).

#[test]
fn instruction_count_timeout() {
    let lua = Lua::new();

    let limit: u64 = 10_000;
    let counter = Arc::new(AtomicU64::new(0));
    let counter_hook = counter.clone();
    let batch: u32 = 100;

    lua.set_hook(
        HookTriggers::new().every_nth_instruction(batch),
        move |_lua, _debug| {
            let count = counter_hook.fetch_add(batch as u64, Ordering::Relaxed);
            if count + batch as u64 >= limit {
                Err(Error::runtime("instruction limit exceeded"))
            } else {
                Ok(VmState::Continue)
            }
        },
    );

    // Infinite loop must be killed
    let err = lua.load("while true do end").exec().unwrap_err();
    let msg = err.to_string();
    assert!(
        msg.contains("instruction limit exceeded"),
        "expected 'instruction limit exceeded', got: {msg}"
    );

    // Counter should have accumulated roughly `limit` instructions
    let final_count = counter.load(Ordering::Relaxed);
    assert!(
        final_count >= limit,
        "counter should have reached the limit, got: {final_count}"
    );

    lua.remove_hook();

    // After removing the hook, normal execution should work
    let result: i64 = lua.load("return 1 + 1").eval().expect("post-hook eval");
    assert_eq!(result, 2);
}

// ---------------------------------------------------------------------------
// 3. Memory limit via allocator (FR23)
// ---------------------------------------------------------------------------
// Configure mlua to limit memory allocation so a script that allocates too
// much is terminated.

#[test]
fn memory_limit() {
    let lua = Lua::new();

    let baseline = lua.used_memory();
    // Allow 512 KB above baseline
    lua.set_memory_limit(baseline + 512 * 1024)
        .expect("set memory limit");

    // Script that tries to allocate a huge table
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
        matches!(result, Err(Error::MemoryError(_))),
        "expected MemoryError, got: {result:?}"
    );

    // Reset limit and verify VM is still usable
    lua.set_memory_limit(0).expect("reset memory limit");
    let val: i64 = lua.load("return 42").eval().expect("post-limit eval");
    assert_eq!(val, 42);
}

// ---------------------------------------------------------------------------
// 4. Sandbox function stripping (architecture requirement)
// ---------------------------------------------------------------------------
// Create a Lua environment with NO access to io, os, loadfile, dofile, or
// any filesystem/network functions. Only string, math, table libs available.
//
// Two-layer approach:
//   1. `Lua::new_with` excludes io, os, package, debug libraries
//   2. Nil-strip base globals (loadfile, dofile, load) which are always present
//      regardless of StdLib flags (they're part of Lua's core runtime)

#[test]
fn sandbox_via_selective_libs() {
    let safe_libs = StdLib::MATH | StdLib::STRING | StdLib::TABLE | StdLib::UTF8;

    let lua =
        Lua::new_with(safe_libs, LuaOptions::default()).expect("create sandboxed Lua");

    // loadfile/dofile/load are Lua core globals — always present regardless of
    // StdLib flags. Must nil them explicitly.
    let globals = lua.globals();
    for name in &["loadfile", "dofile", "load"] {
        globals
            .set(*name, mlua::Value::Nil)
            .unwrap_or_else(|e| panic!("failed to nil {name}: {e}"));
    }

    // Verify safe libs work
    let math_result: f64 = lua
        .load("return math.sqrt(144)")
        .eval()
        .expect("math.sqrt");
    assert!((math_result - 12.0).abs() < f64::EPSILON);

    let string_result: String = lua
        .load(r#"return string.upper("hello")"#)
        .eval()
        .expect("string.upper");
    assert_eq!(string_result, "HELLO");

    let table_result: i64 = lua
        .load(
            r#"
        local t = {3, 1, 2}
        table.sort(t)
        return t[1]
    "#,
        )
        .eval()
        .expect("table.sort");
    assert_eq!(table_result, 1);

    // Verify dangerous functions are absent
    let dangerous_accesses = [
        ("io", "io.open('/etc/passwd', 'r')"),
        ("os", "os.execute('echo pwned')"),
        ("loadfile", "loadfile('init.lua')"),
        ("dofile", "dofile('init.lua')"),
        ("require", "require('os')"),
    ];

    for (name, code) in &dangerous_accesses {
        let err = lua.load(*code).exec();
        assert!(
            err.is_err(),
            "{name} should not be available in sandbox, but code succeeded"
        );
    }
}

// Alternative approach: load all safe libs (includes base) then strip globals.
// This gives you `print`, `type`, `tostring`, `pcall`, `error` etc. while
// removing filesystem-related functions.
#[test]
fn sandbox_via_global_stripping() {
    let lua = Lua::new_with(StdLib::ALL_SAFE, LuaOptions::default())
        .expect("create Lua with all safe libs");

    // Verify loadfile/dofile exist before stripping (they come from base lib)
    let has_loadfile: bool = lua
        .load("return loadfile ~= nil")
        .eval()
        .expect("check loadfile");
    assert!(has_loadfile, "loadfile should exist before stripping");

    // Strip dangerous globals
    let globals = lua.globals();
    for name in &["loadfile", "dofile", "load"] {
        globals.set(*name, mlua::Value::Nil).unwrap();
    }

    // Verify they're gone
    for (name, code) in &[
        ("loadfile", "loadfile('init.lua')"),
        ("dofile", "dofile('init.lua')"),
        ("load", "load('return 1')()"),
    ] {
        let err = lua.load(*code).exec();
        assert!(
            err.is_err(),
            "{name} should not be available after stripping"
        );
    }

    // Verify useful base functions still work (print, type, pcall, etc.)
    let type_result: String = lua
        .load(r#"return type("hello")"#)
        .eval()
        .expect("type()");
    assert_eq!(type_result, "string");

    let pcall_result: bool = lua
        .load("local ok = pcall(function() error('test') end); return not ok")
        .eval()
        .expect("pcall()");
    assert!(pcall_result);
}

// ---------------------------------------------------------------------------
// 5. Rust-to-Lua bridge function (FR22, FR27)
// ---------------------------------------------------------------------------
// Register a Rust function callable from Lua as fila.get(key) that returns a
// string value.

#[test]
fn rust_to_lua_bridge() {
    let lua = Lua::new();

    // Simulate a config store
    let config: Vec<(String, String)> = vec![
        ("max_retries".into(), "3".into()),
        ("timeout_ms".into(), "5000".into()),
        ("queue_name".into(), "orders".into()),
    ];

    // Create the `fila` namespace table
    let fila_table = lua.create_table().expect("create fila table");

    // Register fila.get(key) -> string | nil
    let get_fn = lua
        .create_function(move |_, key: String| {
            let value = config
                .iter()
                .find(|(k, _)| k == &key)
                .map(|(_, v)| v.clone());
            Ok(value)
        })
        .expect("create get function");

    fila_table.set("get", get_fn).expect("set fila.get");
    lua.globals()
        .set("fila", fila_table)
        .expect("set fila global");

    // Call from Lua — existing key
    let result: String = lua
        .load(r#"return fila.get("queue_name")"#)
        .eval()
        .expect("fila.get existing key");
    assert_eq!(result, "orders");

    // Call from Lua — missing key returns nil
    let result: mlua::Value = lua
        .load(r#"return fila.get("nonexistent")"#)
        .eval()
        .expect("fila.get missing key");
    assert!(matches!(result, mlua::Value::Nil));

    // Use it in a Lua expression
    let retries: i64 = lua
        .load(r#"return tonumber(fila.get("max_retries")) + 1"#)
        .eval()
        .expect("fila.get in expression");
    assert_eq!(retries, 4);
}

// ---------------------------------------------------------------------------
// 6. Script input/output pattern — on_enqueue hook signature
// ---------------------------------------------------------------------------
// Call a Lua function that receives a table (msg with headers subtable and
// payload_size number) and returns a table with fairness_key (string),
// weight (number), and throttle_keys (array of strings).

#[test]
fn script_input_output_pattern() {
    let lua = Lua::new();

    // Load the Lua on_enqueue function
    lua.load(
        r#"
        function on_enqueue(msg)
            local tenant = msg.headers["x-tenant-id"] or "default"
            local region = msg.headers["x-region"] or "us"

            return {
                fairness_key = tenant,
                weight = msg.payload_size > 1000 and 2 or 1,
                throttle_keys = {
                    "tenant:" .. tenant,
                    "region:" .. region,
                    "tenant:" .. tenant .. ":region:" .. region,
                },
            }
        end
    "#,
    )
    .exec()
    .expect("load on_enqueue function");

    // Build the input table from Rust (simulating a message)
    let headers = lua.create_table().expect("create headers");
    headers
        .set("x-tenant-id", "acme")
        .expect("set x-tenant-id");
    headers.set("x-region", "eu").expect("set x-region");
    headers
        .set("content-type", "application/json")
        .expect("set content-type");

    let msg = lua.create_table().expect("create msg");
    msg.set("headers", headers).expect("set headers");
    msg.set("payload_size", 2048_i64)
        .expect("set payload_size");

    // Call the function
    let on_enqueue: mlua::Function = lua.globals().get("on_enqueue").expect("get on_enqueue");
    let result: Table = on_enqueue.call(msg).expect("call on_enqueue");

    // Read the results
    let fairness_key: String = result.get("fairness_key").expect("get fairness_key");
    assert_eq!(fairness_key, "acme");

    let weight: i64 = result.get("weight").expect("get weight");
    assert_eq!(weight, 2, "payload_size > 1000 should give weight 2");

    let throttle_keys: Table = result.get("throttle_keys").expect("get throttle_keys");
    let keys: Vec<String> = throttle_keys
        .sequence_values::<String>()
        .collect::<mlua::Result<_>>()
        .expect("collect throttle_keys");

    assert_eq!(
        keys,
        vec![
            "tenant:acme".to_string(),
            "region:eu".to_string(),
            "tenant:acme:region:eu".to_string(),
        ]
    );

    // Also test with a small message (weight should be 1) and missing headers
    let headers2 = lua.create_table().expect("create headers2");
    let msg2 = lua.create_table().expect("create msg2");
    msg2.set("headers", headers2).expect("set headers2");
    msg2.set("payload_size", 500_i64)
        .expect("set payload_size2");

    let result2: Table = on_enqueue.call(msg2).expect("call on_enqueue small msg");

    let fairness_key2: String = result2.get("fairness_key").expect("get fairness_key2");
    assert_eq!(fairness_key2, "default");

    let weight2: i64 = result2.get("weight").expect("get weight2");
    assert_eq!(weight2, 1, "payload_size <= 1000 should give weight 1");

    let throttle_keys2: Table = result2.get("throttle_keys").expect("get throttle_keys2");
    let keys2: Vec<String> = throttle_keys2
        .sequence_values::<String>()
        .collect::<mlua::Result<_>>()
        .expect("collect throttle_keys2");

    assert_eq!(
        keys2,
        vec![
            "tenant:default".to_string(),
            "region:us".to_string(),
            "tenant:default:region:us".to_string(),
        ]
    );
}

// ---------------------------------------------------------------------------
// 7. Global state persists across calls (important for production design)
// ---------------------------------------------------------------------------
// Demonstrates that a single Lua instance shares global state between calls.
// Production code must decide: reuse one VM (fast, must manage state) or
// create fresh VMs (slower, automatic isolation).

#[test]
fn global_state_persists_across_calls() {
    let lua = Lua::new();

    lua.load(
        r#"
        call_count = 0
        function on_enqueue(msg)
            call_count = call_count + 1
            return {
                fairness_key = "tenant",
                weight = 1,
                throttle_keys = {},
            }
        end
    "#,
    )
    .exec()
    .expect("load script");

    let on_enqueue: mlua::Function = lua.globals().get("on_enqueue").unwrap();

    // Call multiple times
    for _ in 0..5 {
        let msg = lua.create_table().unwrap();
        let headers = lua.create_table().unwrap();
        msg.set("headers", headers).unwrap();
        msg.set("payload_size", 100_i64).unwrap();
        let _result: Table = on_enqueue.call(msg).unwrap();
    }

    // Global state accumulated across calls
    let count: i64 = lua.globals().get("call_count").unwrap();
    assert_eq!(count, 5, "global state should persist across calls");

    // To isolate state, you'd reset globals or use separate Lua instances.
    // For the rules engine, this is acceptable since scripts should be
    // pure functions — but we must document it.
}

// ---------------------------------------------------------------------------
// Bonus: Combined sandbox + hooks + bridge (integration scenario)
// ---------------------------------------------------------------------------
// Verify all mechanisms work together as they would in production.

#[test]
fn combined_sandbox_with_hooks_and_bridge() {
    let safe_libs = StdLib::MATH | StdLib::STRING | StdLib::TABLE | StdLib::UTF8;
    let lua =
        Lua::new_with(safe_libs, LuaOptions::default()).expect("create sandboxed Lua");

    // Strip core globals that bypass sandbox
    let globals = lua.globals();
    for name in &["loadfile", "dofile", "load"] {
        globals.set(*name, mlua::Value::Nil).unwrap();
    }

    // Set instruction limit
    let counter = Arc::new(AtomicU64::new(0));
    let counter_hook = counter.clone();
    lua.set_hook(
        HookTriggers::new().every_nth_instruction(100),
        move |_lua, _debug| {
            let count = counter_hook.fetch_add(100, Ordering::Relaxed);
            if count + 100 >= 1_000_000 {
                Err(Error::runtime("instruction limit exceeded"))
            } else {
                Ok(VmState::Continue)
            }
        },
    );

    // Set memory limit (baseline + 1MB)
    let baseline = lua.used_memory();
    lua.set_memory_limit(baseline + 1_048_576)
        .expect("set memory limit");

    // Register fila.get bridge
    let fila = lua.create_table().expect("create fila");
    let get_fn = lua
        .create_function(|_, key: String| {
            Ok(match key.as_str() {
                "priority_boost" => Some("true".to_string()),
                _ => None,
            })
        })
        .expect("create get");
    fila.set("get", get_fn).expect("set get");
    globals.set("fila", fila).expect("set fila");

    // Run a realistic on_enqueue script
    lua.load(
        r#"
        function on_enqueue(msg)
            local boost = fila.get("priority_boost") == "true"
            local base_weight = msg.payload_size > 1000 and 2 or 1
            local weight = boost and base_weight * 2 or base_weight

            return {
                fairness_key = msg.headers["x-tenant-id"] or "default",
                weight = weight,
                throttle_keys = { "global" },
            }
        end
    "#,
    )
    .exec()
    .expect("load script");

    let headers = lua.create_table().unwrap();
    headers.set("x-tenant-id", "acme").unwrap();
    let msg = lua.create_table().unwrap();
    msg.set("headers", headers).unwrap();
    msg.set("payload_size", 2048_i64).unwrap();

    let on_enqueue: mlua::Function = globals.get("on_enqueue").unwrap();
    let result: Table = on_enqueue.call(msg).unwrap();

    assert_eq!(result.get::<String>("fairness_key").unwrap(), "acme");
    assert_eq!(result.get::<i64>("weight").unwrap(), 4); // 2 * 2 (boost)
    let keys: Vec<String> = result
        .get::<Table>("throttle_keys")
        .unwrap()
        .sequence_values()
        .collect::<mlua::Result<_>>()
        .unwrap();
    assert_eq!(keys, vec!["global"]);

    // Instruction counter should be low for a simple script
    let instructions = counter.load(Ordering::Relaxed);
    assert!(
        instructions < 10_000,
        "simple script should use few instructions, used: {instructions}"
    );
}
