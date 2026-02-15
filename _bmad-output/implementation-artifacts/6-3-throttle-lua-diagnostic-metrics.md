# Story 6.3: Throttle, Lua & Diagnostic Metrics

Status: done

## Story

As an operator,
I want per-throttle-key hit rates and Lua execution metrics,
so that I can diagnose throttling behavior and script performance.

## Acceptance Criteria

1. **Given** throttle rate limiting is active, **when** a throttle decision is made during DRR delivery, **then** `fila.throttle.decisions` counter is incremented with labels `throttle_key` and `result` (`pass` or `hit`)
2. **Given** throttle keys are configured, **when** metrics are exported, **then** `fila.throttle.tokens_remaining` gauge shows current token count per throttle key
3. **Given** a Lua on_enqueue or on_failure hook executes, **when** the script completes, **then** `fila.lua.execution_duration_us` histogram records execution time with labels `queue_id` and `hook` (`on_enqueue` or `on_failure`)
4. **Given** a Lua script fails during execution, **when** the error is caught, **then** `fila.lua.errors` counter is incremented (label: `queue_id`)
5. **Given** a Lua circuit breaker trips, **when** consecutive failures exceed the threshold, **then** `fila.lua.circuit_breaker.activations` counter is incremented (label: `queue_id`)
6. **Given** all metric names, **when** reviewed, **then** they follow the `fila.*` naming convention with `snake_case` labels
7. **Given** a test setup with throttle and Lua metrics, **when** metrics are flushed, **then** counter, gauge, and histogram values are correct and match the expected instrumentation points

## Tasks / Subtasks

- [x] Task 1: Extend Metrics struct with throttle and Lua instruments (AC: #1, #2, #3, #4, #5)
  - [x] Subtask 1.1: Add `throttle_decisions: Counter<u64>` for `fila.throttle.decisions`
  - [x] Subtask 1.2: Add `throttle_tokens_remaining: Gauge<f64>` for `fila.throttle.tokens_remaining`
  - [x] Subtask 1.3: Add `lua_execution_duration: Histogram<f64>` for `fila.lua.execution_duration_us`
  - [x] Subtask 1.4: Add `lua_errors: Counter<u64>` for `fila.lua.errors`
  - [x] Subtask 1.5: Add `lua_circuit_breaker_activations: Counter<u64>` for `fila.lua.circuit_breaker.activations`
  - [x] Subtask 1.6: Add recording methods for each new metric
- [x] Task 2: Add LuaExecOutcome to surface execution result to scheduler (AC: #3, #4, #5)
  - [x] Subtask 2.1: Define `LuaExecOutcome` enum in `lua/mod.rs`: `Success`, `ScriptError { circuit_breaker_tripped: bool }`, `CircuitBreakerBypassed`
  - [x] Subtask 2.2: Change `run_on_enqueue` return type from `Option<OnEnqueueResult>` to `Option<(OnEnqueueResult, LuaExecOutcome)>`
  - [x] Subtask 2.3: Change `run_on_failure` return type from `Option<OnFailureResult>` to `Option<(OnFailureResult, LuaExecOutcome)>`
  - [x] Subtask 2.4: Update all call sites in scheduler to destructure the new return type
- [x] Task 3: Instrument throttle decisions in DRR delivery path (AC: #1)
  - [x] Subtask 3.1: After `peek_keys` returns false (throttled), record `hit` for each throttle key
  - [x] Subtask 3.2: After `consume_keys` on successful delivery, record `pass` for each throttle key
- [x] Task 4: Instrument throttle gauge in `record_gauges` (AC: #2)
  - [x] Subtask 4.1: Iterate `throttle.key_stats()` and record `tokens_remaining` per throttle key
- [x] Task 5: Instrument Lua execution in scheduler (AC: #3, #4, #5)
  - [x] Subtask 5.1: Time `run_on_enqueue` with `Instant::now()`, record duration histogram (only when script actually ran, not on CB bypass)
  - [x] Subtask 5.2: Time `run_on_failure` with `Instant::now()`, record duration histogram
  - [x] Subtask 5.3: Record `lua_errors` counter on `ScriptError` outcome
  - [x] Subtask 5.4: Record `lua_circuit_breaker_activations` on `circuit_breaker_tripped = true`
- [x] Task 6: Extend test harness for histogram assertions (AC: #7)
  - [x] Subtask 6.1: Add `assert_histogram` helper that checks count and optionally sum
  - [x] Subtask 6.2: Add `assert_gauge_f64_single` helper for single-label f64 gauge (throttle tokens)
- [x] Task 7: Integration tests (AC: #7)
  - [x] Subtask 7.1: Test throttle_decisions counter records pass/hit per key
  - [x] Subtask 7.2: Test throttle_tokens_remaining gauge records current tokens
  - [x] Subtask 7.3: Test lua_execution_duration histogram records entries
  - [x] Subtask 7.4: Test lua_errors counter increments on script failure
  - [x] Subtask 7.5: Test lua_circuit_breaker_activations counter increments on trip

## Dev Notes

### Architecture Compliance

- **Metric naming**: `fila.category.metric_type` with `snake_case` labels per architecture spec
- **Labels**: `throttle_key` + `result` on throttle decisions; `queue_id` + `hook` on Lua duration; `queue_id` on Lua errors and CB activations
- **Performance**: All recording is O(1). Throttle per-key iteration in DRR delivery is bounded by message throttle_keys count (typically 1-3)
- **Histogram**: Uses OTel `Histogram<f64>` — reports distribution (count, sum, buckets) per export interval

### LuaExecOutcome Design

The current `run_on_enqueue`/`run_on_failure` API returns `Option<T>` which hides whether execution succeeded, failed with fallback, or was circuit-breaker-bypassed. To instrument correctly:

- Define `LuaExecOutcome` enum surfacing the execution path
- Return `Option<(T, LuaExecOutcome)>` so the scheduler can:
  - Record duration histogram only when script actually ran (Success or ScriptError, not CircuitBreakerBypassed)
  - Record error counter on ScriptError
  - Record CB activation counter when `circuit_breaker_tripped = true`

### Throttle Decision Instrumentation

In `drr_deliver_queue()`:
- After `peek_keys(&throttle_keys)` returns `false` → record `hit` for each key
- After `consume_keys(&throttle_keys)` on delivery success → record `pass` for each key
- When `throttle_keys` is empty (message not throttled) → no recording needed (no throttle check)

### Throttle Tokens Gauge

In `record_gauges()`:
- Call `self.throttle.key_stats()` which returns `Vec<(key, tokens, rate, burst)>`
- Record `fila.throttle.tokens_remaining` gauge for each key with label `throttle_key`

### Histogram Test Strategy

OTel histograms export as `AggregatedMetrics::F64(MetricData::Histogram(hist))`. Data points have:
- `count()` — number of recordings
- `sum()` — sum of all values

For tests: assert `count >= expected_count` (exact count may vary with timing) and verify the metric exists.

### Key Code Locations

| Location | What Happens | Metric to Record |
|----------|--------------|------------------|
| `scheduler.rs` — `drr_deliver_queue()`, after `peek_keys` returns false | Throttle blocked | `fila.throttle.decisions` (result=hit) |
| `scheduler.rs` — `drr_deliver_queue()`, after `consume_keys` | Throttle passed | `fila.throttle.decisions` (result=pass) |
| `scheduler.rs` — `record_gauges()` | Periodic gauge update | `fila.throttle.tokens_remaining` |
| `scheduler.rs` — around `run_on_enqueue()` | Lua hook execution | `fila.lua.execution_duration_us` |
| `scheduler.rs` — around `run_on_failure()` | Lua hook execution | `fila.lua.execution_duration_us` |
| `scheduler.rs` — on LuaExecOutcome::ScriptError | Lua script failed | `fila.lua.errors` |
| `scheduler.rs` — on circuit_breaker_tripped | CB tripped | `fila.lua.circuit_breaker.activations` |

### Key Files to Modify

| File | Change |
|------|--------|
| `crates/fila-core/src/broker/metrics.rs` | Add 5 new instruments, recording methods, extend test harness with histogram support |
| `crates/fila-core/src/broker/scheduler.rs` | Instrument throttle path, wrap Lua calls with timing, record from LuaExecOutcome, extend record_gauges |
| `crates/fila-core/src/lua/mod.rs` | Add LuaExecOutcome enum, update run_on_enqueue/run_on_failure return types |

### References

- [Source: _bmad-output/planning-artifacts/architecture.md#Observability] Metric naming conventions
- [Source: _bmad-output/planning-artifacts/epics.md] Story 6.3 AC definition
- [Source: crates/fila-core/src/broker/throttle.rs] ThrottleManager with peek_keys, consume_keys, key_stats
- [Source: crates/fila-core/src/lua/mod.rs] LuaEngine run_on_enqueue/run_on_failure API
- [Source: crates/fila-core/src/broker/metrics.rs] Story 6.1 Metrics struct and test harness
- [Source: crates/fila-core/src/broker/scheduler.rs:1150-1160] Throttle check in DRR delivery
- [Source: crates/fila-core/src/broker/scheduler.rs:275-287] on_enqueue Lua call site
- [Source: crates/fila-core/src/broker/scheduler.rs:896-906] on_failure Lua call site

## Dev Agent Record

### Agent Model Used
claude-opus-4-6

### Completion Notes List
- Added 5 new OTel instruments: throttle_decisions counter, throttle_tokens_remaining f64 gauge, lua_execution_duration f64 histogram, lua_errors counter, lua_cb_activations counter
- Defined LuaExecOutcome enum (Success, ScriptError, CircuitBreakerBypassed) and updated both run_on_enqueue and run_on_failure return types to surface execution metadata
- Instrumented scheduler: Lua timing via Instant::now(), error/CB recording per LuaExecOutcome variant, throttle hit/pass per key in DRR delivery, throttle tokens gauge in record_gauges
- Extended test harness with assert_counter_with_attrs, assert_histogram, assert_gauge_f64 helpers plus 3 new extraction functions
- Added 5 integration tests covering all new metrics
- Updated 2 existing Lua tests for new tuple return type
- 258 tests pass, clippy clean, fmt clean
- No code review findings

### File List
- crates/fila-core/src/broker/metrics.rs
- crates/fila-core/src/broker/scheduler.rs
- crates/fila-core/src/lua/mod.rs
