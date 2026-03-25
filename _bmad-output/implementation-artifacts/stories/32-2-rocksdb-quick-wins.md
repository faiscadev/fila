# Story 32.2: RocksDB Quick Wins — Tuning & Queue Config Cache

Status: pending

## Story

As a developer,
I want to apply low-risk RocksDB configuration tuning and eliminate the per-message queue existence check,
So that enqueue throughput improves by ~50-80% with minimal code changes.

## Acceptance Criteria

1. **Given** `unordered_write` is disabled in the RocksDB configuration
   **When** it is enabled
   **Then** write throughput improves measurably (expected +34-42% based on RocksDB benchmarks)
   **And** existing tests pass (unordered_write is safe for Fila's use case: no iterators during writes on the hot path)

2. **Given** every enqueue message triggers a `storage.get_queue()` RocksDB read to check queue existence
   **When** an in-memory queue config cache is introduced
   **Then** queue existence checks read from the cache (O(1) HashMap lookup) instead of RocksDB
   **And** the cache is populated on startup by scanning existing queues
   **And** the cache is updated on queue create/delete/update operations
   **And** the per-message RocksDB read on CF_QUEUES is eliminated from the enqueue hot path

3. **Given** RocksDB write buffer defaults are used
   **When** write buffer sizes are tuned
   **Then** `write_buffer_size` is set to 128-256 MB (configurable)
   **And** `max_write_buffer_number` is set to 4
   **And** `max_background_compactions` matches available CPU core count

4. **Given** the optimizations are applied
   **When** `fila-bench` enqueue throughput is measured
   **Then** throughput improves over the 32.1 baseline
   **And** the profiling from 32.1 is re-run to measure per-operation cost changes

5. **Given** the tuning changes
   **When** memory usage is measured
   **Then** RSS increase from write buffer tuning is documented
   **And** RSS stays within acceptable bounds for the target deployment

## Tasks / Subtasks

- [ ] Task 1: Enable `unordered_write` in RocksDB config
  - [ ] 1.1 Set `options.set_unordered_write(true)` in `rocksdb.rs`
  - [ ] 1.2 Verify all tests pass
  - [ ] 1.3 Benchmark before/after

- [ ] Task 2: Implement queue config cache
  - [ ] 2.1 Add `HashMap<String, QueueConfig>` cache to scheduler state
  - [ ] 2.2 Populate on startup from storage scan
  - [ ] 2.3 Update cache on CreateQueue/DeleteQueue/UpdateQueue commands
  - [ ] 2.4 Replace `storage.get_queue()` calls in `prepare_enqueue()` with cache lookup
  - [ ] 2.5 Add tests for cache consistency (create → lookup → delete → lookup)

- [ ] Task 3: Tune write buffer parameters
  - [ ] 3.1 Set `write_buffer_size`, `max_write_buffer_number`, `max_background_compactions`
  - [ ] 3.2 Make values configurable via `BrokerConfig`
  - [ ] 3.3 Document memory implications

- [ ] Task 4: Benchmark and profile
  - [ ] 4.1 Run `fila-bench` enqueue throughput
  - [ ] 4.2 Re-run flamegraph + tracing spans from 32.1
  - [ ] 4.3 Document measured improvement

## Dev Notes

### RocksDB `unordered_write` Safety

`unordered_write = true` relaxes ordering guarantees: writes from different threads may appear in different orders in the WAL. This is safe for Fila because:
- The scheduler is single-writer per shard — no concurrent writes to the same keys
- Fila does not rely on WAL ordering for correctness (Raft provides ordering in cluster mode)

### Queue Config Cache Design

The cache replaces a per-message RocksDB point lookup (5-10μs) with a HashMap lookup (~50ns). Queue configs change rarely (admin operations only), so cache invalidation is straightforward — update on the same scheduler thread that handles admin commands.

### Key Files to Modify

| File | Change |
|------|--------|
| `crates/fila-core/src/storage/rocksdb.rs` | Enable `unordered_write`, tune write buffers |
| `crates/fila-core/src/broker/scheduler/handlers.rs` | Replace `storage.get_queue()` with cache lookup in `prepare_enqueue()` |
| `crates/fila-core/src/broker/scheduler/mod.rs` | Add queue config cache, populate on startup |
| `crates/fila-core/src/config.rs` | Add write buffer config fields |

### References

- [Source: RocksDB Unordered Write](https://rocksdb.org/blog/2019/08/15/unordered-write.html) — +34-42% throughput
- [Source: RocksDB Tuning Guide](https://github.com/facebook/rocksdb/wiki/RocksDB-Tuning-Guide) — Write buffer sizing
- [Research: technical-kafka-parity-profiling-strategy-2026-03-25.md, Pattern 7] — RocksDB tuning analysis
