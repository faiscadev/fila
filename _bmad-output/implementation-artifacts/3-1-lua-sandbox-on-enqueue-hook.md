# Story 3.1: Lua Sandbox & on_enqueue Hook

Status: dev-complete

## Story

As a platform engineer,
I want to write Lua scripts that assign fairness keys, weights, and throttle keys from message headers at enqueue time,
so that I can define custom scheduling policy without modifying the broker.

## Acceptance Criteria

1. **Given** a queue is created with an `on_enqueue` Lua script, **when** a message is enqueued to that queue, **then** the Lua script receives a `msg` table with: `msg.headers` (read-only table), `msg.payload_size` (number), `msg.queue` (string)
2. **Given** the on_enqueue script executes, **when** it returns a table, **then** the broker uses the returned `fairness_key` (string), `weight` (number, optional, default 1), and `throttle_keys` (array of strings, optional) to assign scheduling metadata to the message
3. **Given** the Lua sandbox, **then** it provides standard Lua string, math, and table libraries but does NOT provide IO, OS, filesystem, or network access
4. **Given** the Lua sandbox, **then** `fila.get(key)` reads runtime config from the `state` CF via the storage layer
5. **Given** a queue is created with an on_enqueue script, **then** the script is pre-compiled to bytecode at queue creation time and cached (no parse overhead per enqueue)
6. **Given** a queue has no on_enqueue script, **when** a message is enqueued, **then** the existing default behavior applies (`fairness_key = "default"`, `weight = 1`, no throttle keys)
7. **Given** the on_enqueue script returns invalid output (missing fairness_key, wrong types), **then** safe defaults are applied and a warning is logged
8. An integration test creates a queue with an on_enqueue script that reads `msg.headers["tenant_id"]` and assigns it as the fairness key, then verifies messages are assigned the correct keys

## Tasks / Subtasks

- [x] Task 1: Create `lua` module in fila-core (AC: #1, #2, #3, #4, #5)
  - [x] 1.1 Create `crates/fila-core/src/lua/mod.rs` with `LuaEngine` struct
  - [x] 1.2 Create `crates/fila-core/src/lua/sandbox.rs` — Lua VM creation with safe stdlib subset
  - [x] 1.3 Create `crates/fila-core/src/lua/bridge.rs` — `fila.get()` registration using storage get_state
  - [x] 1.4 Create `crates/fila-core/src/lua/on_enqueue.rs` — `OnEnqueueResult` struct, script execution, output parsing
  - [x] 1.5 `LuaEngine::compile_script(source) -> Result<CompiledScript>` — pre-compile to bytecode via `Lua::load().into_function()` then `function.dump()`
  - [x] 1.6 `LuaEngine::run_on_enqueue(compiled, msg, state_reader) -> OnEnqueueResult` — execute pre-compiled script with msg table input
  - [x] 1.7 Add `LuaError` variants to lua/mod.rs (internal errors, not exposed to callers)
  - [x] 1.8 Unit tests: compile, execute, sandbox restrictions, fila.get() bridge, invalid output → defaults (16 unit tests)

- [x] Task 2: Move mlua from dev-dependencies to regular dependency in fila-core (AC: #5)
  - [x] 2.1 Workspace `Cargo.toml` already has mlua in `[workspace.dependencies]`
  - [x] 2.2 Update `crates/fila-core/Cargo.toml` — moved mlua from `[dev-dependencies]` to `[dependencies]`

- [x] Task 3: Integrate on_enqueue into scheduler enqueue path (AC: #1, #2, #5, #6)
  - [x] 3.1 Add `lua_engine: Option<LuaEngine>` field to `Scheduler` struct
  - [x] 3.2 In `handle_enqueue()`: check for cached on_enqueue script via lua_engine
  - [x] 3.3 If script exists: call `lua_engine.run_on_enqueue()`, apply returned fairness_key/weight/throttle_keys to message
  - [x] 3.4 If no script: keep existing defaults (backward compatible)
  - [x] 3.5 Handle Lua errors: log warning, apply safe defaults (AC: #7)

- [x] Task 4: Pre-compile and cache scripts at queue creation (AC: #5)
  - [x] 4.1 In `handle_create_queue()`: if on_enqueue_script is Some, compile to bytecode via LuaEngine
  - [x] 4.2 Store compiled bytecode in `on_enqueue_cache: HashMap<String, CompiledScript>` on LuaEngine (keyed by queue_id)
  - [x] 4.3 On `handle_delete_queue()`: remove cached script
  - [x] 4.4 On startup recovery: re-compile scripts from queue configs in queues CF
  - [x] 4.5 Return error from CreateQueue if script fails to compile (`CreateQueueError::LuaCompilation(String)`)

- [x] Task 5: Integration tests (AC: #8)
  - [x] 5.1 `on_enqueue_assigns_fairness_key_from_header` — create queue with Lua script that reads `msg.headers["tenant_id"]`, enqueue message, verify fairness_key="acme" in storage
  - [x] 5.2 `on_enqueue_assigns_weight_and_throttle_keys` — script returns weight=5 and throttle_keys, verify message metadata in storage
  - [x] 5.3 `queue_without_script_uses_defaults` — enqueue without script, verify fairness_key="default", weight=1
  - [x] 5.4 `on_enqueue_reads_config_via_fila_get` — set config via state CF, script reads via fila.get("default_tenant"), verify fairness_key="megacorp"
  - [x] 5.5 `create_queue_with_invalid_script_returns_error` — attempt to create queue with syntax-error script, verify CreateQueueError::LuaCompilation returned

## Dev Notes

### Architecture — Where Lua Fits

The Lua engine executes **synchronously on the scheduler thread**. This is intentional: the single-threaded scheduler owns all mutable state, and Lua runs as a pure function transforming message metadata during `handle_enqueue()`. No async, no additional threads.

```
Enqueue flow with Lua:
  gRPC handler → SchedulerCommand::Enqueue → scheduler thread
    → handle_enqueue()
      → lookup queue config (already in memory or storage)
      → IF on_enqueue_script exists:
          → lua_engine.run_on_enqueue(compiled_bytecode, msg_headers, payload_size, queue_name, state_reader)
          → apply returned {fairness_key, weight, throttle_keys} to message
      → ELSE: use defaults (fairness_key="default", weight=1, throttle_keys=[])
      → build message key with (possibly Lua-assigned) fairness_key
      → storage.put_message()
      → drr.add_key() with (possibly Lua-assigned) weight
```

### Lua Spike Reference

All capabilities were validated in `crates/fila-core/tests/lua_spike.rs` (commit `ad502cc`):

- **Bytecode precompilation**: `Lua::load(source).into_function()` then `function.dump(true)` for binary bytecode. Load with `lua.load(&bytecode).set_mode(ChunkMode::Binary)` — rejects source injection (test 1, lines 16-71)
- **Safe stdlib**: `Lua::new_with(StdLib::MATH | StdLib::STRING | StdLib::TABLE | StdLib::UTF8, LuaOptions::default())` then nil out `loadfile`, `dofile`, `load` from globals (test 4, lines 171-228)
- **fila.get() bridge**: Register Rust closure as `fila.get` in globals table. Closure captures a reference/Arc to state reader (test 6, lines 284-334)
- **Script I/O pattern**: Input is `msg` table with `headers` and `payload_size`. Output is table with `fairness_key`, `weight`, `throttle_keys` (test 7, lines 343-439)

### Key Implementation Decisions

1. **One Lua VM per scheduler** (not per queue). Create a single `mlua::Lua` instance owned by `LuaEngine`. Pre-compiled bytecode is loaded into this VM for each execution. The spike confirmed global state persists across calls (test 8), so scripts must be pure functions — no mutable globals.

2. **CompiledScript = `Vec<u8>`** (bytecode). Stored in `HashMap<String, Vec<u8>>` on the scheduler, keyed by queue_id. Loaded into the Lua VM via `lua.load(&bytecode).set_mode(ChunkMode::Binary).into_function()` on each call.

3. **fila.get() bridge** must read from storage's state CF. The scheduler already has `&dyn Storage` access. Register a Rust function into Lua globals that calls `storage.get_state(key)`. Since Lua runs on the scheduler thread which owns storage access, this is safe without any synchronization.

4. **Output parsing**: Extract fields from returned Lua table. If `fairness_key` is missing or not a string → use "default". If `weight` is missing or not a number → use 1. If `throttle_keys` is missing or not a table → use empty vec. Log warnings for unexpected types but don't fail the enqueue.

5. **Error types** — add to `crates/fila-core/src/error.rs`:
   - `EnqueueError::LuaExecution(String)` is NOT needed because Lua errors don't fail enqueue — they fall back to defaults
   - `CreateQueueError::LuaCompilation { source: String }` — script failed to compile (syntax error, etc.)
   - `LuaError` enum in `lua/mod.rs` for internal Lua errors (used for logging, not for propagation to callers)

### File Structure

```
crates/fila-core/src/
├── lua/
│   ├── mod.rs           # LuaEngine struct, CompiledScript type, re-exports
│   ├── sandbox.rs       # create_sandbox() → mlua::Lua with safe libs, nil'd dangerous globals
│   ├── bridge.rs        # register_fila_api(lua, state_reader) — registers fila.get()
│   └── on_enqueue.rs    # OnEnqueueResult, run_on_enqueue(), parse output table
├── lib.rs               # Add `pub mod lua;`
└── error.rs             # Add CreateQueueError::LuaCompilation
```

### Dependencies to Touch

- `crates/fila-core/Cargo.toml` — move mlua from `[dev-dependencies]` to `[dependencies]`
- `Cargo.toml` (workspace root) — ensure mlua is in `[workspace.dependencies]` (already is)

### Existing Code Patterns to Follow

- **Per-command error types**: See `error.rs` — each command returns only errors it can produce. `CreateQueueError` gets a new `LuaCompilation` variant. `EnqueueError` does NOT get a Lua variant (Lua failures fall back to defaults, never fail enqueue).
- **Storage trait**: All state access goes through `&dyn Storage`. Use `storage.get_state(key)` for fila.get() bridge.
- **WriteBatch atomicity**: Message persistence already uses WriteBatch. Lua doesn't change this — it only modifies the message's metadata fields before the WriteBatch is assembled.
- **Module organization**: One module per domain concept. `lua/` is the new module with focused sub-files.

### Testing Notes

- Integration tests use the existing test helper pattern: start broker, create gRPC client, exercise flow
- For script compilation failure test: use a script with syntax errors and verify CreateQueue returns an error (gRPC INVALID_ARGUMENT)
- For fila.get() test: pre-populate state CF values, create queue with script that calls fila.get(), enqueue and verify script used config value
- The scheduler already has direct storage access so unit tests of LuaEngine can use the existing test storage setup

### Risks & Mitigations

- **Risk**: Lua execution adds latency to every enqueue. **Mitigation**: Pre-compiled bytecode eliminates parse overhead. Story 3.2 adds execution timeouts. Benchmark in Story 3.2.
- **Risk**: Global state leaks between script executions. **Mitigation**: Scripts must be pure functions. The spike (test 8) showed globals persist — so don't use mutable globals. Document this constraint.
- **Risk**: fila.get() could be slow if state CF has many entries. **Mitigation**: Single-key point lookup in RocksDB is O(1). Not a concern.

### References

- [Source: _bmad-output/planning-artifacts/architecture.md#Lua Integration] — Hook model, safety model, Lua API
- [Source: _bmad-output/planning-artifacts/architecture.md#Data Architecture — RocksDB] — state CF for fila.get()
- [Source: _bmad-output/planning-artifacts/epics.md#Story 3.1] — Full acceptance criteria
- [Source: crates/fila-core/tests/lua_spike.rs] — All capabilities pre-validated
- [Source: _bmad-output/implementation-artifacts/epic-2-retro-2026-02-12.md] — Pattern establishment guidance, test infrastructure needs

## Dev Agent Record

### Agent Model Used

claude-opus-4-6

### Completion Notes List

- `mlua::Error` is not `Send+Sync` (contains `Arc<dyn StdError>`), so `CreateQueueError::LuaCompilation` stores `String` instead of wrapping `LuaError`. Error is converted to string at call site.
- `Scheduler` is now `!Send` because it contains `Option<LuaEngine>` (Lua VM uses `Rc` internally). Five existing threaded tests were refactored to create `Scheduler` inside `std::thread::spawn` instead of moving it across the thread boundary.
- `lua_engine` is `Option<LuaEngine>` rather than `LuaEngine` to gracefully handle Lua VM creation failures — if VM fails to create, scripts are disabled but broker continues to function.
- Total: 110 tests passing (+ 1 ignored benchmark), 0 warnings.

### Change Log

- Created `crates/fila-core/src/lua/mod.rs` — `LuaEngine` struct with compile/cache/execute API, `LuaError` enum, 7 unit tests
- Created `crates/fila-core/src/lua/sandbox.rs` — `create_sandbox()` with safe stdlib, 2 unit tests
- Created `crates/fila-core/src/lua/bridge.rs` — `register_fila_api()` for `fila.get()`, 1 unit test
- Created `crates/fila-core/src/lua/on_enqueue.rs` — `OnEnqueueResult`, `run_on_enqueue()`, output parsing with safe fallbacks, 6 unit tests
- Modified `crates/fila-core/src/lib.rs` — added `pub mod lua;`
- Modified `crates/fila-core/Cargo.toml` — moved mlua from dev-deps to deps
- Modified `crates/fila-core/src/error.rs` — added `CreateQueueError::LuaCompilation(String)`
- Modified `crates/fila-core/src/broker/scheduler.rs` — integrated LuaEngine into Scheduler, handle_enqueue, handle_create_queue, handle_delete_queue, recover; refactored 5 threaded tests; added 5 integration tests
- Modified `crates/fila-server/src/error.rs` — added `LuaCompilation` → `INVALID_ARGUMENT` mapping

### File List

New files:
- `crates/fila-core/src/lua/mod.rs`
- `crates/fila-core/src/lua/sandbox.rs`
- `crates/fila-core/src/lua/bridge.rs`
- `crates/fila-core/src/lua/on_enqueue.rs`

Modified files:
- `crates/fila-core/Cargo.toml`
- `crates/fila-core/src/lib.rs`
- `crates/fila-core/src/error.rs`
- `crates/fila-core/src/broker/scheduler.rs`
- `crates/fila-server/src/error.rs`
