# Story 32.3: String Interning — lasso for queue_id and fairness_key

Status: pending

## Story

As a developer,
I want repeated `queue_id` and `fairness_key` strings interned as 4-byte tokens throughout the scheduler,
So that per-message string cloning and HashMap key allocation overhead is eliminated.

## Acceptance Criteria

1. **Given** `queue_id` and `fairness_key` are `String` values cloned multiple times per message in the hot path
   **When** `lasso::ThreadedRodeo` string interning is introduced
   **Then** each unique string is interned once, producing a `Spur` (4 bytes, `Copy`)
   **And** all subsequent operations use `Spur` instead of `String` for these fields

2. **Given** the scheduler uses `String` keys in DRR state, pending index, and finalize_enqueue
   **When** these are migrated to `Spur`
   **Then** DRR `add_key()`, `next_key()`, and pending queue operations use `Spur` keys
   **And** HashMap entries shrink from 24+ bytes (String) to 4 bytes (Spur) per key
   **And** `String::clone()` calls for queue_id/fairness_key in the hot path are replaced with `Spur` copy (4-byte memcpy)

3. **Given** the interner
   **When** strings need to be resolved back to `&str` (for storage keys, gRPC responses, logging)
   **Then** `interner.resolve(&spur)` provides the original string
   **And** resolution is used only at system boundaries (storage write, gRPC encode), not in the hot scheduling loop

4. **Given** `lasso` v0.7.0 has a known deadlock issue (#39)
   **When** the dependency is added
   **Then** the pinned version is 0.6.x (known-stable)
   **And** a code comment references the issue for future upgrade consideration

5. **Given** the interning changes
   **When** all existing tests are run
   **Then** all tests pass with identical behavior
   **And** `fila-bench` enqueue throughput improves over the 32.2 baseline

## Tasks / Subtasks

- [ ] Task 1: Add `lasso` dependency
  - [ ] 1.1 Add `lasso = "0.6"` to `fila-core/Cargo.toml`
  - [ ] 1.2 Create interner instance in scheduler (per-shard ownership)

- [ ] Task 2: Intern queue_id throughout scheduler
  - [ ] 2.1 Intern at enqueue entry point (after extracting from proto)
  - [ ] 2.2 Migrate DRR state to use `Spur` for queue keys
  - [ ] 2.3 Migrate pending index to use `Spur` for queue keys
  - [ ] 2.4 Resolve back to `&str` at storage write and gRPC response boundaries

- [ ] Task 3: Intern fairness_key throughout scheduler
  - [ ] 3.1 Intern at enqueue entry point
  - [ ] 3.2 Migrate DRR fairness key state to `Spur`
  - [ ] 3.3 Migrate pending entry fairness_key to `Spur`
  - [ ] 3.4 Resolve at storage and delivery boundaries

- [ ] Task 4: Benchmark and profile
  - [ ] 4.1 Run `fila-bench` enqueue throughput
  - [ ] 4.2 Measure allocation reduction (fewer heap allocs per message)
  - [ ] 4.3 Re-run flamegraph to verify string clone overhead reduced

## Dev Notes

### lasso Architecture

- `ThreadedRodeo`: concurrent interner for the enqueue path (multiple gRPC threads intern simultaneously)
- `Spur`: 4-byte opaque token, `Copy + Eq + Hash` — ideal for HashMap keys and struct fields
- Memory model: arena-backed, strings never freed until Rodeo is dropped. Safe for Fila because queue_id × fairness_key cardinality is bounded.

### Migration Strategy

The migration is internal to the scheduler. The `StorageEngine` trait and gRPC proto layer continue to use strings — interning happens at the boundary between gRPC handler and scheduler, and un-interning happens at the boundary between scheduler and storage/response.

```
gRPC handler (String) → intern → Scheduler (Spur) → resolve → Storage (String bytes)
```

### Key Files to Modify

| File | Change |
|------|--------|
| `crates/fila-core/src/broker/scheduler/mod.rs` | Add interner, intern on enqueue entry |
| `crates/fila-core/src/broker/scheduler/handlers.rs` | Use Spur in prepare/finalize_enqueue |
| `crates/fila-core/src/broker/drr.rs` | Migrate key types from String to Spur |
| `crates/fila-core/src/broker/scheduler/delivery.rs` | Resolve Spur at delivery boundaries |
| `crates/fila-core/Cargo.toml` | Add lasso dependency |

### References

- [Source: lasso GitHub](https://github.com/Kixiron/lasso) — ThreadedRodeo API
- [Source: lasso Issue #39](https://github.com/Kixiron/lasso/issues/39) — v0.7.0 deadlock
- [Research: technical-kafka-parity-profiling-strategy-2026-03-25.md, Pattern 4] — String interning analysis
