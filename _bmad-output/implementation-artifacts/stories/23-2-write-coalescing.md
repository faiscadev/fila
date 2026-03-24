# Story 23.2: Server-Side Write Coalescing

Status: in-progress

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

- [ ] Task 1: Add `write_coalesce_max_batch` to `SchedulerConfig` with default 100
- [ ] Task 2: Refactor `handle_enqueue` to return prepared mutations + metadata instead of writing directly
- [ ] Task 3: Add `coalesce_enqueues` method that batches mutations from multiple enqueue commands
- [ ] Task 4: Update scheduler loop to drain channel and separate enqueue vs non-enqueue commands
- [ ] Task 5: Add tests for coalescing (single-message fast path, multi-message batch, error isolation)
- [ ] Task 6: Verify all existing tests pass

## Dev Notes

- The scheduler is single-threaded. No async/await, locks, or multi-threading introduced.
- Coalescing is purely about draining multiple commands from the crossbeam channel before committing.
- `prepare_enqueue` does everything `handle_enqueue` did (queue check, Lua, routing, key generation, metrics, DRR, pending) except the actual `put_message` storage call.
- After all prepares, mutations are committed in one `apply_mutations` call.
- On storage failure, all callers in the batch receive the error (atomic batch).
