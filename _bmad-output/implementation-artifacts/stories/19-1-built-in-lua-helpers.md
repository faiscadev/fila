# Story 19.1: Built-in Lua Helpers

Status: ready-for-dev

## Story

As a script author,
I want built-in Lua helper functions for common patterns,
so that I can implement standard scheduling policies without writing boilerplate.

## Acceptance Criteria

1. **Given** the Lua sandbox provides `fila.get()` and standard libraries
   **When** built-in helpers are loaded
   **Then** the Lua environment includes a `fila.helpers` module available to all scripts

2. **Given** a script calls `fila.helpers.exponential_backoff(attempts, base_ms, max_ms)`
   **When** attempts, base_ms, and max_ms are valid numbers
   **Then** it returns delay in milliseconds with jitter (randomized ±25% of computed delay)
   **And** computed delay = `base_ms * 2^(attempts - 1)`, capped at `max_ms`
   **And** with zero attempts returns `base_ms`

3. **Given** a script calls `fila.helpers.tenant_route(msg, header_name)`
   **When** `msg.headers[header_name]` exists
   **Then** it returns a table `{ fairness_key = <header_value> }`
   **And** when the header is missing or nil, returns `{ fairness_key = "default" }`

4. **Given** a script calls `fila.helpers.rate_limit_keys(msg, patterns)`
   **When** patterns is a table of string patterns like `{"provider:{provider_id}", "region:{region}"}`
   **Then** it returns an array of throttle keys with `{placeholder}` replaced by the corresponding `msg.headers` value
   **And** patterns referencing missing headers are omitted from the result

5. **Given** a script calls `fila.helpers.max_retries(attempts, max)`
   **When** `attempts < max`
   **Then** it returns `{ action = "retry" }`
   **And** when `attempts >= max`, returns `{ action = "dlq" }`

6. **Given** helpers are documented
   **Then** `docs/lua-patterns.md` includes a "Built-in Helpers" section with API reference and usage examples for each helper

7. **Given** helpers are unit tested in Rust (via mlua)
   **Then** edge cases are covered: nil headers, missing keys, zero attempts, negative values, overflow values, empty patterns table, non-numeric arguments

8. **Given** existing user scripts that don't use helpers
   **Then** they continue to work unchanged — helpers are additive, not replacing any existing API

## Tasks / Subtasks

- [ ] Task 1: Create `helpers.rs` module in `crates/fila-core/src/lua/` (AC: 1)
  - [ ] 1.1: Create `crates/fila-core/src/lua/helpers.rs` with a `register_helpers(lua: &Lua)` function
  - [ ] 1.2: In `register_helpers`, create a `helpers` table and set it on the existing `fila` global: `fila.helpers = helpers_table`
  - [ ] 1.3: Add `pub mod helpers;` to `crates/fila-core/src/lua/mod.rs`
  - [ ] 1.4: Call `helpers::register_helpers(&lua)` in `LuaEngine::new()` after `register_fila_api`

- [ ] Task 2: Implement `fila.helpers.exponential_backoff` (AC: 2)
  - [ ] 2.1: Register Lua function `exponential_backoff(attempts, base_ms, max_ms)` on helpers table
  - [ ] 2.2: Compute delay: `base_ms * 2^(attempts-1)`, cap at `max_ms`
  - [ ] 2.3: Add jitter: multiply by random factor in [0.75, 1.25] using `math.random()`
  - [ ] 2.4: Handle edge cases: `attempts <= 0` → return `base_ms`, large exponents → cap before overflow

- [ ] Task 3: Implement `fila.helpers.tenant_route` (AC: 3)
  - [ ] 3.1: Register Lua function `tenant_route(msg, header_name)` on helpers table
  - [ ] 3.2: Extract `msg.headers[header_name]`, return `{ fairness_key = value }` or `{ fairness_key = "default" }` if nil

- [ ] Task 4: Implement `fila.helpers.rate_limit_keys` (AC: 4)
  - [ ] 4.1: Register Lua function `rate_limit_keys(msg, patterns)` on helpers table
  - [ ] 4.2: Iterate patterns, replace `{placeholder}` with `msg.headers[placeholder]`
  - [ ] 4.3: Omit keys where any referenced header is missing

- [ ] Task 5: Implement `fila.helpers.max_retries` (AC: 5)
  - [ ] 5.1: Register Lua function `max_retries(attempts, max)` on helpers table
  - [ ] 5.2: Return `{ action = "retry" }` if `attempts < max`, else `{ action = "dlq" }`

- [ ] Task 6: Unit tests (AC: 7)
  - [ ] 6.1: Test `exponential_backoff` — normal case, zero attempts, large exponents, jitter range
  - [ ] 6.2: Test `tenant_route` — present header, missing header, nil msg.headers
  - [ ] 6.3: Test `rate_limit_keys` — valid patterns, missing headers omitted, empty patterns
  - [ ] 6.4: Test `max_retries` — below max, at max, above max, zero max
  - [ ] 6.5: Test helpers are accessible alongside `fila.get()` (no interference)

- [ ] Task 7: Update documentation (AC: 6)
  - [ ] 7.1: Add "Built-in Helpers" section to `docs/lua-patterns.md` with API reference
  - [ ] 7.2: Add usage examples showing helpers in `on_enqueue` and `on_failure` scripts

- [ ] Task 8: Update sprint-status.yaml
  - [ ] 8.1: Mark story 19-1 as in-progress, epic-19 as in-progress

## Dev Notes

### Architecture Context

The Lua module lives at `crates/fila-core/src/lua/` with this structure:
- `mod.rs` — `LuaEngine` struct, manages VM lifecycle and script caching
- `bridge.rs` — Registers `fila` global table with `fila.get(key)` function
- `sandbox.rs` — Creates sandboxed Lua 5.4 VM with safe stdlib (math, string, table, utf8)
- `on_enqueue.rs` — Executes `on_enqueue` hook, parses result table
- `on_failure.rs` — Executes `on_failure` hook, parses result table
- `safety.rs` — Circuit breaker, instruction limit, memory limit

### Key Design Decision: Helpers as Pure Lua

The helpers should be implemented as **pure Lua functions registered from Rust**, not as Rust closures. This keeps them simple, testable, and within the existing sandbox model. The Lua `math.random()` function is available since `StdLib::MATH` is enabled in the sandbox.

Registration pattern (follow `bridge.rs`):
```rust
pub fn register_helpers(lua: &Lua) -> mlua::Result<()> {
    // Get existing fila table (already registered by bridge.rs)
    let fila_table: mlua::Table = lua.globals().get("fila")?;
    let helpers_table = lua.create_table()?;

    // Register each helper as a Lua function
    let exponential_backoff = lua.create_function(|_, (attempts, base_ms, max_ms): (f64, f64, f64)| {
        // Implementation...
        Ok(delay_with_jitter)
    })?;
    helpers_table.set("exponential_backoff", exponential_backoff)?;

    // ... more helpers ...

    fila_table.set("helpers", helpers_table)?;
    Ok(())
}
```

### Key Design Decision: Registration Order

`register_helpers` must be called AFTER `register_fila_api` because it reads the existing `fila` global table to add the `helpers` subtable. In `LuaEngine::new()`:
```rust
bridge::register_fila_api(&lua, storage).map_err(LuaError::BridgeRegistration)?;
helpers::register_helpers(&lua).map_err(LuaError::BridgeRegistration)?;  // After bridge
```

### Jitter Implementation

Use `math.random()` via Lua's stdlib (already available in sandbox). For the Rust-side implementation, use `mlua`'s built-in random or compute jitter in the closure:
- Compute base delay: `base_ms * 2.0_f64.powf((attempts - 1.0).max(0.0))`
- Cap at `max_ms`
- Apply jitter: `delay * (0.75 + rand::random::<f64>() * 0.5)` (Rust rand crate) OR use `lua.load("math.random()").eval::<f64>()` to stay within the sandbox

Using Rust `rand` is cleaner than calling back into Lua. The `rand` crate is already a transitive dependency.

### Existing Patterns to Follow

- **`bridge.rs`** — Pattern for registering Lua functions on the `fila` table via closures
- **`on_enqueue.rs`** — Pattern for creating Lua tables as return values (`lua.create_table()`)
- **Tests in `mod.rs`** — `test_engine()` helper creates a `LuaEngine` with temp storage; reuse for helper tests

### Testing Standards

- Unit tests in `crates/fila-core/src/lua/helpers.rs` (inline `#[cfg(test)]` module)
- Use `sandbox::create_sandbox()` + `register_fila_api()` + `register_helpers()` for each test
- Test via `lua.load(script).eval()` patterns (see `bridge.rs` tests for examples)
- Edge cases per AC 7: nil, zero, negative, overflow, empty tables, non-numeric args

### Backward Compatibility

Helpers are purely additive. The `fila` global table already exists with `fila.get()`. Adding `fila.helpers` does not affect any existing scripts. Scripts that don't use helpers see no change.

### References

- [Source: crates/fila-core/src/lua/bridge.rs] — `register_fila_api()`, existing `fila` table pattern
- [Source: crates/fila-core/src/lua/mod.rs] — `LuaEngine::new()`, where to call `register_helpers`
- [Source: crates/fila-core/src/lua/sandbox.rs] — `create_sandbox()`, enabled stdlib
- [Source: docs/lua-patterns.md] — Existing patterns to update with helper examples
- [Source: _bmad-output/planning-artifacts/epics.md#Epic-19] — Story ACs

## Dev Agent Record

### Agent Model Used

Claude Opus 4.6 (1M context)

### Debug Log References

### Completion Notes List

### File List
