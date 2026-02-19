# Story 8.2: Scheduler Decomposition

Status: review

## Story

As a developer,
I want `scheduler.rs` decomposed into focused submodules,
so that each module is under 500 lines and has a clear single responsibility.

## Acceptance Criteria

1. **Given** the blackbox e2e test suite from Story 8.1, **when** the scheduler is decomposed, **then** `scheduler.rs` is split into submodules under `broker/scheduler/`:
   - `mod.rs` — Scheduler struct, event loop, command dispatch
   - `delivery.rs` — DRR delivery logic, `drr_deliver_queue`, consumer management
   - `handlers.rs` — Command handlers: enqueue, ack, nack, config, stats, redrive, list_queues, list_config
   - `recovery.rs` — Startup recovery, lease expiry scanning, state rebuild
   - `leasing.rs` — Lease creation, expiry tracking, visibility timeout
   - `metrics_recording.rs` — `record_gauges`, fairness delivery tracking, all metric recording logic
2. **Given** the decomposed modules, **when** line counts are checked, **then** no submodule exceeds ~500 lines (architecture guideline)
3. **Given** the decomposed scheduler, **when** all test suites run, **then** all existing tests (unit, integration, metric, e2e) pass with zero changes to test assertions
4. **Given** the decomposition, **when** the public API is inspected, **then** the public API of `Scheduler` remains unchanged (internal restructure only)
5. **Given** the decomposed code, **when** `cargo clippy` and `cargo fmt` are run, **then** both pass cleanly

## Tasks / Subtasks

- [ ] Task 1: Create `broker/scheduler/` directory structure (AC: #1)
  - [ ] Subtask 1.1: Rename `scheduler.rs` → `scheduler/mod.rs` (or create new `mod.rs` and delete old file)
  - [ ] Subtask 1.2: Update `broker/mod.rs` to reference `mod scheduler` (module path changes from file to directory)
  - [ ] Subtask 1.3: Verify compilation with the scheduler as a directory module (no logic changes yet)

- [ ] Task 2: Extract `handlers.rs` — command handlers (AC: #1, #2)
  - [ ] Subtask 2.1: Move all `handle_*` functions to `handlers.rs`: `handle_enqueue`, `handle_create_queue`, `handle_delete_queue`, `handle_set_config`, `handle_list_config`, `handle_get_config`, `handle_get_stats`, `handle_redrive`, `handle_list_queues`, `handle_ack`, `handle_nack`, `parse_throttle_value`
  - [ ] Subtask 2.2: Make moved functions `pub(super)` or keep as `impl Scheduler` methods via `impl` block in the new file
  - [ ] Subtask 2.3: Verify compilation and all tests pass

- [ ] Task 3: Extract `delivery.rs` — DRR delivery logic (AC: #1, #2)
  - [ ] Subtask 3.1: Move `drr_deliver`, `drr_deliver_queue`, `try_deliver_to_consumer`, `pending_push`, `remove_pending_for_queue`, `find_message_key` to `delivery.rs`
  - [ ] Subtask 3.2: Verify compilation and all tests pass

- [ ] Task 4: Extract `recovery.rs` — startup recovery and lease expiry (AC: #1, #2)
  - [ ] Subtask 4.1: Move `recover` and `reclaim_expired_leases` to `recovery.rs`
  - [ ] Subtask 4.2: Verify compilation and all tests pass

- [ ] Task 5: Extract `leasing.rs` — lease creation and expiry tracking (AC: #1, #2)
  - [ ] Subtask 5.1: Assess whether there is enough leasing-specific logic to warrant a separate module, or if leasing is already covered by `delivery.rs` and `recovery.rs`
  - [ ] Subtask 5.2: If warranted, move lease tracking (leased_msg_keys management, visibility timeout logic) to `leasing.rs`
  - [ ] Subtask 5.3: Verify compilation and all tests pass

- [ ] Task 6: Extract `metrics_recording.rs` — OTel gauge recording (AC: #1, #2)
  - [ ] Subtask 6.1: Move `record_gauges` and fairness_deliveries management to `metrics_recording.rs`
  - [ ] Subtask 6.2: Verify compilation and all tests pass

- [ ] Task 7: Organize test modules (AC: #2, #3)
  - [ ] Subtask 7.1: Split the ~5,280-line test module into logical test submodules under `scheduler/tests/` or inline test modules per source file
  - [ ] Subtask 7.2: Move test helpers (`test_setup`, `test_message`, etc.) to a shared test helper module
  - [ ] Subtask 7.3: Verify all 278 tests pass with zero assertion changes

- [ ] Task 8: Final validation (AC: #2, #3, #4, #5)
  - [ ] Subtask 8.1: Verify no module exceeds ~500 lines (production code; test modules can be slightly larger)
  - [ ] Subtask 8.2: Run `cargo fmt --all` and `cargo clippy --workspace`
  - [ ] Subtask 8.3: Run `cargo nextest run --workspace` — all 278 tests pass
  - [ ] Subtask 8.4: Run `cargo nextest run -p fila-e2e` — all 11 e2e tests pass (the safety net)
  - [ ] Subtask 8.5: Verify `Scheduler` public API is unchanged (only `pub fn new`, `pub fn run`, `pub fn storage` for tests)

## Dev Notes

### Architecture

This is a pure **move-only refactoring** — no logic changes, no new features, no API changes. The goal is to split the monolithic `scheduler.rs` (6,992 lines) into focused submodules.

### Current File Structure

```
crates/fila-core/src/broker/
├── mod.rs          — Broker struct, references `mod scheduler`
├── scheduler.rs    — 6,992 lines (1,712 production + 5,280 tests)
├── command.rs      — SchedulerCommand enum, ReadyMessage
├── config.rs       — BrokerConfig, SchedulerConfig, LuaConfig
├── drr.rs          — DrrScheduler (already extracted)
├── metrics.rs      — OTel metric instruments
├── stats.rs        — QueueStats struct
└── throttle.rs     — ThrottleManager (already extracted)
```

### Target File Structure

```
crates/fila-core/src/broker/
├── mod.rs              — Broker struct (unchanged)
├── scheduler/
│   ├── mod.rs          — Scheduler struct definition, `new()`, `run()`, command dispatch, re-exports
│   ├── handlers.rs     — All `handle_*` command handlers
│   ├── delivery.rs     — DRR delivery: `drr_deliver`, `drr_deliver_queue`, `try_deliver_to_consumer`, pending index
│   ├── recovery.rs     — `recover()` startup, `reclaim_expired_leases()`
│   ├── leasing.rs      — Lease tracking helpers (if warranted; may merge into delivery/recovery)
│   ├── metrics_recording.rs — `record_gauges()`, fairness delivery tracking
│   └── tests/          — Test submodules (or inline #[cfg(test)] per file)
├── command.rs          — (unchanged)
├── config.rs           — (unchanged)
├── drr.rs              — (unchanged)
├── metrics.rs          — (unchanged)
├── stats.rs            — (unchanged)
└── throttle.rs         — (unchanged)
```

### Key Constraints

1. **No test assertion changes** — If any test needs modification, the decomposition is wrong. Tests should be moved exactly as-is, only import paths change.
2. **No public API changes** — `Scheduler::new()`, `Scheduler::run()` are the only public interface. The `pub fn storage()` test accessor must remain available for tests.
3. **`impl Scheduler` blocks across files** — Rust allows `impl` blocks in multiple files within the same module. Each subfile can have `impl Scheduler { ... }` for the functions it owns.
4. **Visibility** — Internal functions become `pub(crate)` or remain private (accessible within the `scheduler` module). No functions should become more public than they currently are.
5. **Struct fields** — The `Scheduler` struct stays in `mod.rs`. Other files access fields through `self` since they're in the same module.

### Production Code Breakdown (Lines to Move)

| Target Module | Functions | Approx Lines |
|---|---|---|
| `mod.rs` | Scheduler struct, `new()`, `run()`, `handle_command()` dispatcher | ~270 |
| `handlers.rs` | All `handle_*` functions, `parse_throttle_value` | ~720 |
| `delivery.rs` | `drr_deliver`, `drr_deliver_queue`, `try_deliver_to_consumer`, pending index helpers | ~290 |
| `recovery.rs` | `recover`, `reclaim_expired_leases` | ~270 |
| `metrics_recording.rs` | `record_gauges`, fairness_deliveries tracking | ~80 |
| `leasing.rs` | Lease tracking (assess if separate module warranted) | ~50-80 |

### Test Code Breakdown (~5,280 lines)

The test module at line 1713 contains 78 test functions plus helpers. Tests should be split into test submodules that mirror the production modules:

| Test Module | Source Section | Approx Lines |
|---|---|---|
| `tests/common.rs` | Test setup helpers, message factories | ~150 |
| `tests/command_tests.rs` | Basic commands, queue operations | ~200 |
| `tests/enqueue_tests.rs` | Enqueue and basic delivery | ~350 |
| `tests/ack_nack_tests.rs` | Ack, nack, lease expiry | ~500 |
| `tests/recovery_tests.rs` | Recovery after restart | ~400 |
| `tests/fairness_tests.rs` | DRR fairness, weighted delivery | ~380 |
| `tests/lua_tests.rs` | on_enqueue, on_failure, circuit breaker | ~670 |
| `tests/dlq_tests.rs` | Dead-letter queue operations | ~260 |
| `tests/throttle_tests.rs` | Throttle rate limiting, config | ~470 |
| `tests/config_tests.rs` | ListConfig, Lua integration | ~160 |
| `tests/stats_tests.rs` | GetStats output verification | ~360 |
| `tests/redrive_tests.rs` | Redrive operations | ~360 |
| `tests/list_queues_tests.rs` | ListQueues operations | ~95 |

### Migration Strategy

1. **Start with `scheduler.rs` → `scheduler/mod.rs`** (renaming only, verify build)
2. **Extract one module at a time**, starting with the smallest (`metrics_recording.rs`) to validate the pattern
3. **Move production code first**, then move corresponding tests
4. **Run full test suite after each extraction** — if anything breaks, the move was incorrect
5. **The e2e tests (Story 8.1) are the ultimate safety net** — they verify behavior through the public API

### What NOT To Do

- Do NOT change any logic, algorithms, or behavior
- Do NOT rename functions (except visibility adjustments like `pub(super)`)
- Do NOT add new dependencies
- Do NOT modify test assertions
- Do NOT change the `Scheduler` public API
- Do NOT restructure `broker/mod.rs` beyond updating the `mod scheduler` declaration

### References

- [Source: _bmad-output/planning-artifacts/epics.md#Story 8.2]
- [Source: crates/fila-core/src/broker/scheduler.rs — current monolith, 6,992 lines]
- [Source: crates/fila-core/src/broker/mod.rs — Broker struct, `mod scheduler` declaration]
- [Source: _bmad-output/implementation-artifacts/8-1-blackbox-e2e-test-suite.md — safety net tests]

## Dev Agent Record

### Agent Model Used
claude-opus-4-6

### Debug Log References

### Completion Notes List
- Decomposed monolithic scheduler.rs (6,992 lines) into 6 production modules + 15 test files
- Production modules: mod.rs (282), handlers.rs (432), admin_handlers.rs (380), delivery.rs (293), recovery.rs (276), metrics_recording.rs (77)
- All production modules under 500 lines (target met)
- Test module split into 13 test submodules + common helpers + tests/mod.rs
- leasing.rs assessed and deemed unnecessary — lease logic is tightly coupled with delivery/recovery/handlers
- handlers.rs initially 809 lines, split into handlers.rs (432) + admin_handlers.rs (380)
- Pure move-only refactoring — zero logic changes, zero test assertion changes
- 278/278 tests pass, 11/11 e2e safety net tests pass
- Public API unchanged: new(), run(), storage() (test-only)

### File List
- crates/fila-core/src/broker/scheduler/mod.rs (modified — struct, new, run, handle_command, 282 lines)
- crates/fila-core/src/broker/scheduler/handlers.rs (new — enqueue, create/delete queue, ack, nack)
- crates/fila-core/src/broker/scheduler/admin_handlers.rs (new — config, stats, redrive, list_queues, parse_throttle)
- crates/fila-core/src/broker/scheduler/delivery.rs (new — DRR delivery, pending index, consumer delivery)
- crates/fila-core/src/broker/scheduler/recovery.rs (new — recover, reclaim_expired_leases)
- crates/fila-core/src/broker/scheduler/metrics_recording.rs (new — record_gauges)
- crates/fila-core/src/broker/scheduler/tests/mod.rs (new — test module entry point)
- crates/fila-core/src/broker/scheduler/tests/common.rs (new — shared test helpers)
- crates/fila-core/src/broker/scheduler/tests/command.rs (new — 9 tests)
- crates/fila-core/src/broker/scheduler/tests/enqueue.rs (new — 3 tests)
- crates/fila-core/src/broker/scheduler/tests/delivery.rs (new — 10 tests)
- crates/fila-core/src/broker/scheduler/tests/ack_nack.rs (new — 12 tests)
- crates/fila-core/src/broker/scheduler/tests/recovery.rs (new — 7 tests)
- crates/fila-core/src/broker/scheduler/tests/fairness.rs (new — 8 tests)
- crates/fila-core/src/broker/scheduler/tests/lua.rs (new — 15 tests)
- crates/fila-core/src/broker/scheduler/tests/dlq.rs (new — 6 tests)
- crates/fila-core/src/broker/scheduler/tests/throttle.rs (new — 5 tests)
- crates/fila-core/src/broker/scheduler/tests/config.rs (new — 16 tests)
- crates/fila-core/src/broker/scheduler/tests/stats.rs (new — 8 tests)
- crates/fila-core/src/broker/scheduler/tests/redrive.rs (new — 8 tests)
- crates/fila-core/src/broker/scheduler/tests/list_queues.rs (new — 2 tests)
- crates/fila-core/src/broker/scheduler.rs (deleted — replaced by scheduler/ directory)
