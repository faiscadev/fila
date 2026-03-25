# Story 34.3: Tee Engine Validation — Dual-Write Correctness Verification

Status: pending

## Story

As a developer,
I want a `TeeEngine` that sends all writes to both RocksDB and AppendOnlyEngine and compares reads,
So that the custom storage engine's correctness is validated against the proven RocksDB implementation before cutover.

## Acceptance Criteria

1. **Given** `RocksDbEngine` and `AppendOnlyEngine` both implement `StorageEngine`
   **When** `TeeEngine` is configured
   **Then** all write operations (`apply_mutations`) are sent to BOTH engines
   **And** all read operations query BOTH engines and compare results
   **And** any divergence is logged as an error with full details (key, expected value, actual value)

2. **Given** the TeeEngine is running
   **When** the full test suite (unit + integration + e2e) is executed
   **Then** zero divergences are detected
   **And** all tests pass

3. **Given** the TeeEngine is running under load
   **When** `fila-bench` is executed
   **Then** zero divergences are detected after the benchmark run
   **And** the performance overhead of dual-writing is documented

4. **Given** the TeeEngine validation passes
   **When** a cutover decision is made
   **Then** switching from `RocksDbEngine` to `AppendOnlyEngine` is a configuration change
   **And** rollback to `RocksDbEngine` is equally simple

## Tasks / Subtasks

- [ ] Task 1: Implement TeeEngine
  - [ ] 1.1 Implement `StorageEngine` trait: forward all writes to both engines
  - [ ] 1.2 Implement read comparison: query both, compare results
  - [ ] 1.3 Log divergences with full context

- [ ] Task 2: Run test suite validation
  - [ ] 2.1 Run unit tests with TeeEngine
  - [ ] 2.2 Run integration tests with TeeEngine
  - [ ] 2.3 Run e2e tests with TeeEngine

- [ ] Task 3: Run load validation
  - [ ] 3.1 Run fila-bench with TeeEngine
  - [ ] 3.2 Document divergence count (should be 0)
  - [ ] 3.3 Document performance overhead

## Dev Notes

### CockroachDB Precedent

CockroachDB used this exact pattern (tee engine) when migrating from RocksDB to Pebble. It caught divergences that would have caused data corruption in production. The tee engine ran for months in testing before the cutover.

### Key Files to Create/Modify

| File | Change |
|------|--------|
| `crates/fila-core/src/storage/tee.rs` | NEW: TeeEngine implementation |
| `crates/fila-core/src/storage/mod.rs` | Register TeeEngine |
| `crates/fila-core/src/config.rs` | Engine selection config (rocksdb / append_only / tee) |

### References

- [Source: CockroachDB Pebble Migration](https://www.cockroachlabs.com/blog/pebble-rocksdb-kv-store/) — Tee engine validation
- [Research: technical-kafka-parity-profiling-strategy-2026-03-25.md, Architecture Decision 2] — Storage migration strategy
