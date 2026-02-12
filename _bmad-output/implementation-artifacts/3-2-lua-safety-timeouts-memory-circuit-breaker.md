# Story 3.2: Lua Safety: Timeouts, Memory Limits & Circuit Breaker

Status: complete

## Story

As an operator,
I want Lua scripts to be safely sandboxed with execution limits and automatic fallback,
so that a buggy script cannot crash or slow down the broker.

## Acceptance Criteria

1. **Given** a queue has a Lua script attached, **when** the script exceeds its execution timeout (default 10ms, configurable per queue), **then** the script is terminated via mlua instruction count hook and the circuit breaker failure counter is incremented
2. **Given** a queue has a Lua script attached, **when** the script exceeds its memory limit (default 1MB, configurable per queue), **then** the script is terminated via mlua memory limit and the circuit breaker failure counter is incremented
3. **Given** a Lua script has failed 3 consecutive times (configurable threshold), **when** the next message is enqueued, **then** the circuit breaker is active: Lua is bypassed entirely, safe defaults are applied (`fairness_key = "default"`, `weight = 1`, no throttle keys), a warning is logged, and an error counter is incremented
4. **Given** the circuit breaker is active, **when** the cooldown period (default 10 seconds, configurable) expires, **then** the next enqueue attempts Lua execution again
5. **Given** the circuit breaker is active and cooldown has expired, **when** a Lua execution succeeds, **then** the consecutive failure counter is reset to 0
6. **Given** the circuit breaker threshold is configured, **then** unit tests verify circuit breaker activation at exactly the threshold count (not before, not after)
7. **Given** the circuit breaker cooldown, **then** unit tests verify the cooldown period behavior (bypass during cooldown, retry after cooldown)
8. **Given** the Lua config defaults are `[lua] default_timeout_ms = 10, default_memory_limit_bytes = 1048576, circuit_breaker_threshold = 3, circuit_breaker_cooldown_ms = 10000`, **then** these are added to `BrokerConfig` and used as defaults when per-queue values are not set

## Tasks / Subtasks

- [x] Task 1: Add Lua safety config to BrokerConfig (AC: #8)
- [x] Task 2: Add instruction count timeout to LuaEngine (AC: #1)
- [x] Task 3: Add memory limit to LuaEngine (AC: #2)
- [x] Task 4: Implement circuit breaker (AC: #3, #4, #5, #6, #7)
- [x] Task 5: Pass config through to LuaEngine and Scheduler (AC: #1, #2, #8)
- [x] Task 6: Integration tests (AC: #1, #2, #3, #4, #5)

## Dev Notes

### Architecture — Safety Layer

The safety mechanisms wrap the existing `run_on_enqueue` execution. The flow becomes:

```
handle_enqueue()
  → circuit_breaker.should_execute(queue_id)?
    → NO: apply defaults, log warning, return
    → YES:
      → set instruction hook (timeout)
      → set memory limit
      → run_on_enqueue_inner()
      → remove hook, reset memory limit
      → if OK: circuit_breaker.record_success(queue_id)
      → if Err: circuit_breaker.record_failure(queue_id)
        → if tripped: log warning "circuit breaker tripped for queue X"
```

### Spike Reference

All safety capabilities were validated in `crates/fila-core/tests/lua_spike.rs`:

- **Instruction count timeout**: `lua.set_hook(HookTriggers::new().every_nth_instruction(batch), ...)` with `Err(Error::runtime("..."))` to abort. After aborting, call `lua.remove_hook()` and the VM is reusable. (test 2, lines 80-120)
- **Memory limit**: `lua.set_memory_limit(baseline + limit)` returns `Ok`. Script that exceeds limit gets `Error::MemoryError`. Reset with `lua.set_memory_limit(0)` and VM is reusable. (test 3, lines 128-158)

### Key Implementation Decisions

1. **Instruction count as proxy for time**: Real wall-clock measurement is impractical in a synchronous hook. Instead, use instruction count as a proxy. The default of 10ms maps to approximately 100,000 instructions (rough calibration). This is configurable so operators can tune it.

2. **Per-queue circuit breaker state**: Each queue has its own circuit breaker. A failing script on queue A doesn't affect queue B. State is in-memory (lost on restart, which is acceptable — a fresh start means a clean slate).

3. **Circuit breaker with cooldown**: After tripping, the breaker stays active for `cooldown_ms`. After cooldown expires, the next enqueue tries Lua again. If it succeeds, the counter resets. If it fails again, the counter starts fresh (1 failure after cooldown retry).

4. **Config layering**: Global defaults in `[lua]` section of TOML config. Per-queue overrides in `QueueConfig` (optional fields). Effective value = queue override or global default.

5. **Hook lifecycle**: The instruction count hook must be set before each call and removed after. The memory limit must be set before and reset after. Both are applied to the single `Lua` VM instance owned by `LuaEngine`.

### File Structure

```
crates/fila-core/src/
├── broker/
│   ├── config.rs    # Add LuaConfig struct
│   └── scheduler.rs # Pass LuaConfig to LuaEngine
├── lua/
│   ├── mod.rs       # Update LuaEngine to accept config, manage circuit breakers
│   ├── safety.rs    # NEW: CircuitBreaker struct, instruction hook setup, memory limit management
│   └── on_enqueue.rs # No changes needed (safety wraps the call)
└── queue.rs         # Add per-queue lua_timeout_ms, lua_memory_limit_bytes
```

### Dependencies to Touch

- No new crate dependencies needed (mlua already provides all APIs)
- `std::time::Instant` for circuit breaker cooldown tracking

### Existing Code Patterns to Follow

- **Config pattern**: `BrokerConfig` → `SchedulerConfig` with `#[serde(default)]` and `impl Default`. Add `LuaConfig` following the same pattern.
- **Per-command error types**: Timeout and memory errors during on_enqueue don't need new error types — they fall back to defaults (same as existing Lua error handling).
- **Circuit breaker is internal state**: Not exposed via errors to gRPC callers. Only surfaced through logging and metrics.

### Testing Notes

- Instruction count tests need scripts that provably exceed the limit (infinite loops, large iterations)
- Memory limit tests need scripts that allocate large tables/strings
- Circuit breaker tests should use a low threshold (e.g., 2) and short cooldown (e.g., 50ms) for fast testing
- Use `std::time::Instant` for cooldown timing, not mocked clocks (tests use real short durations)

### Risks & Mitigations

- **Risk**: Instruction count is not perfectly correlated with wall-clock time. **Mitigation**: Document that the timeout is approximate. Operators can tune the instruction count per ms calibration via config.
- **Risk**: Memory limit might be slightly exceeded (Lua allocates in chunks). **Mitigation**: mlua's built-in memory limiting handles this at the allocator level, which is precise enough.
- **Risk**: Circuit breaker could stay tripped forever if the script permanently fails. **Mitigation**: Cooldown ensures periodic retries. Operators can fix the script and the next retry after cooldown will succeed and reset the breaker.

### References

- [Source: _bmad-output/planning-artifacts/architecture.md#Lua Integration — Safety Model]
- [Source: _bmad-output/planning-artifacts/architecture.md#Configuration Management — lua section]
- [Source: _bmad-output/planning-artifacts/epics.md#Story 3.2]
- [Source: crates/fila-core/tests/lua_spike.rs — tests 2, 3]
- [Source: crates/fila-core/src/lua/mod.rs — LuaEngine struct]
- [Source: crates/fila-core/src/lua/on_enqueue.rs — run_on_enqueue with fallback pattern]

## Dev Agent Record

### Agent Model Used
claude-opus-4-6

### Completion Notes List
- All 6 tasks completed: config, instruction hook, memory limit, circuit breaker, config wiring, integration tests
- 137 tests passing (125 unit + 9 spike + 3 server), 1 ignored (slow 10k DRR)
- New safety tests: 11 unit tests in safety.rs + 4 integration tests in scheduler.rs
- `SafetyManager` centralizes per-queue breakers and config with global defaults
- Instruction count hook uses `INSTRUCTIONS_PER_MS = 10_000` calibration constant
- Circuit breaker is per-queue, in-memory (lost on restart, by design)

### Change Log
- `crates/fila-core/src/broker/config.rs` — Added `LuaConfig` struct and `lua` field to `BrokerConfig`
- `crates/fila-core/src/queue.rs` — Added `lua_timeout_ms`, `lua_memory_limit_bytes` per-queue overrides
- `crates/fila-core/src/lua/safety.rs` — NEW: CircuitBreaker, SafetyManager, instruction hook, memory limit functions
- `crates/fila-core/src/lua/mod.rs` — Updated LuaEngine with SafetyManager, safety hooks in run_on_enqueue
- `crates/fila-core/src/lua/on_enqueue.rs` — Exposed `try_run_on_enqueue` (returns Result)
- `crates/fila-core/src/broker/scheduler.rs` — Updated Scheduler::new to accept LuaConfig, 4 new integration tests
- `crates/fila-core/src/broker/mod.rs` — Updated Broker::new to pass lua config to scheduler
- `crates/fila-server/src/admin_service.rs` — Added new QueueConfig fields

### File List
- crates/fila-core/src/broker/config.rs
- crates/fila-core/src/broker/mod.rs
- crates/fila-core/src/broker/scheduler.rs
- crates/fila-core/src/lua/mod.rs
- crates/fila-core/src/lua/on_enqueue.rs
- crates/fila-core/src/lua/safety.rs (NEW)
- crates/fila-core/src/queue.rs
- crates/fila-server/src/admin_service.rs
