# Story 23.2: Server-Side Write Coalescing

Status: review

## Story

As a Fila operator,
I want the scheduler to coalesce concurrent enqueue commands into batched storage writes,
So that high-throughput workloads achieve higher write throughput by amortizing RocksDB WriteBatch overhead across multiple messages.

## Acceptance Criteria

1. **Given** multiple enqueue commands arrive concurrently on the scheduler channel
   **When** the scheduler loop drains them
   **Then** their storage mutations are collected into a single `apply_mutations` WriteBatch call

2. **And** non-enqueue commands (ack, nack, admin) are processed inline without waiting for the coalescing batch

3. **And** a new `write_coalesce_max_batch` config (default 100) limits how many commands are drained per batch

4. **And** when only one message is in the channel (low load), it is processed immediately with no artificial delay

5. **And** each enqueue caller receives its individual response (success/error) after the coalesced write commits

6. **And** all existing tests pass unchanged (single-command behavior is identical)

## Tasks / Subtasks

- [x] Task 1: Add `write_coalesce_max_batch` to `SchedulerConfig` with default 100
- [x] Task 2: Refactor `handle_enqueue` into `prepare_enqueue` + `finalize_enqueue`
- [x] Task 3: Add `flush_coalesced_enqueues` method that batches mutations from multiple enqueue commands
- [x] Task 4: Add `drain_and_coalesce` to scheduler loop, separating enqueue vs non-enqueue commands
- [x] Task 5: Add 6 coalescing tests (single fast path, multi-message, error isolation, interleaved, cross-queue, max batch)
- [x] Task 6: Verify all existing tests pass (385 tests, zero failures)

## Dev Notes

- The scheduler is single-threaded. No async/await, locks, or multi-threading introduced.
- Coalescing is purely about draining multiple commands from the crossbeam channel before committing.
- `prepare_enqueue` does everything `handle_enqueue` did (queue check, Lua, routing, key generation, serialization) except the actual storage write, metrics, DRR, and pending index.
- `finalize_enqueue` updates in-memory state (metrics, DRR, pending) after the batch commits.
- After all prepares, mutations are committed in one `apply_mutations` call.
- On storage failure, all callers in the batch receive the error (atomic batch).
- Non-enqueue commands interleaved in the channel trigger a flush of pending enqueues before processing.

## Files Changed

- `crates/fila-core/src/broker/config.rs` -- `write_coalesce_max_batch` field + tests
- `crates/fila-core/src/broker/scheduler/mod.rs` -- `drain_and_coalesce`, `flush_coalesced_enqueues`, field storage
- `crates/fila-core/src/broker/scheduler/handlers.rs` -- `PreparedEnqueue`, `prepare_enqueue`, `finalize_enqueue`, `handle_enqueue` refactor
- `crates/fila-core/src/broker/scheduler/tests/coalescing.rs` -- 6 new tests
- `crates/fila-core/src/broker/scheduler/tests/mod.rs` -- register coalescing module
- `crates/fila-core/src/broker/scheduler/tests/common.rs` -- updated SchedulerConfig structs
- `crates/fila-core/src/broker/scheduler/tests/ack_nack.rs` -- updated SchedulerConfig structs
- `crates/fila-core/src/broker/scheduler/tests/fairness.rs` -- updated SchedulerConfig structs
- `crates/fila-core/src/broker/mod.rs` -- updated SchedulerConfig structs in broker tests
