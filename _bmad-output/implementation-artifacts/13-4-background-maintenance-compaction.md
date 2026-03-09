# Story 13.4: Background Maintenance & Compaction

Status: review

## Story

As an operator,
I want background storage maintenance that doesn't cause latency spikes,
so that the broker maintains predictable performance under sustained load.

## Critical Context

**Story 13.3 delivered** the read path: `FilaStorage` has in-memory indexes rebuilt from WAL replay on startup. All `Storage` trait methods are fully functional.

**This story implements** background compaction of sealed WAL segments. Acknowledged/deleted messages accumulate as dead entries in WAL segments. Compaction rewrites sealed segments, keeping only live entries, and reclaims disk space. It also adds TTL-based message expiry during compaction.

**What is NOT in scope:**
- Integration with broker / cutover (Story 13.5)
- Changes to the read or write path logic
- Online segment defragmentation (compaction only runs on sealed segments)

## Acceptance Criteria

1. **Given** sealed WAL segments containing dead entries (deleted messages, leases, etc.)
   **When** background compaction runs
   **Then** dead entries are removed from the compacted segment
   **And** live entries are preserved in correct order
   **And** the foreground write and read paths are not blocked

2. **Given** active write and read operations
   **When** background compaction is running concurrently
   **Then** foreground p99 latency does not spike above 10ms
   **And** compaction I/O is rate-limited to prevent foreground starvation

3. **Given** a message whose age exceeds the configured `message_ttl_ms`
   **When** compaction processes the segment containing that message
   **Then** the message is treated as dead (not written to compacted output)
   **And** the message is removed from the in-memory index

4. **Given** a sustained workload of enqueue + ack cycles
   **When** compaction has run
   **Then** storage footprint is below 1.5x the raw size of live message data

5. **Given** `FilaStorageConfig` with compaction settings
   **When** `FilaStorage::open()` is called
   **Then** a background compaction thread is spawned
   **And** compaction triggers are configurable: interval (default 60s) or segment-count threshold

6. **Given** compaction activity
   **When** segments are compacted
   **Then** OTel metrics are recorded:
   - `fila.storage.compaction.segments_compacted` (counter)
   - `fila.storage.compaction.bytes_reclaimed` (counter)
   - `fila.storage.compaction.duration_seconds` (histogram)

7. **Given** unit tests for compaction
   **When** tests run
   **Then** compaction removes dead entries, preserves live entries
   **And** WAL replay after compaction produces the same in-memory state
   **And** concurrent read/write during compaction works correctly

## Tasks / Subtasks

- [x] Task 1: Add compaction configuration (AC: #5)
  - [x] Add to `FilaStorageConfig`: `compaction_interval_secs` (default 60), `message_ttl_ms` (default None = no TTL), `compaction_io_rate_bytes_per_sec` (default 10MB/s)
  - [x] Add `compaction_enabled` bool (default false, explicitly enabled in production)

- [x] Task 2: Implement compaction logic (AC: #1, #2, #4)
  - [x] `compact_segment()` in `storage/fila/compaction.rs`
  - [x] `compact_segment()`: read sealed segment → filter live entries → write new segment → delete old
  - [x] Determine liveness: `LivenessSnapshot` checks key existence in index snapshot
  - [x] Rate-limit writes: sleep between chunks to cap I/O at configured rate
  - [x] Atomic swap: write compacted segment with `.tmp` suffix, rename to original name on completion

- [x] Task 3: Implement TTL expiry during compaction (AC: #3)
  - [x] During compaction, for `PutMessage` entries: deserialize, check if `enqueued_at + ttl_ms < now`
  - [x] If expired: skip entry AND remove from in-memory index (acquire write lock briefly)
  - [x] If no TTL configured: skip TTL check entirely

- [x] Task 4: Background thread lifecycle (AC: #5)
  - [x] Spawn compaction thread on `FilaStorage::open()` if `compaction_enabled`
  - [x] Thread sleeps for `compaction_interval_secs`, then runs compaction pass
  - [x] Graceful shutdown: drop signals thread to stop via `AtomicBool`, `CompactionHandle::Drop` joins thread
  - [x] Thread has read access to `Arc<RwLock<Indexes>>` and `Arc<Mutex<WalWriter>>` for sealed segment paths

- [x] Task 5: OTel metrics (AC: #6)
  - [x] Register compaction metrics using existing OTel patterns from `crates/fila-core/src/broker/metrics.rs`
  - [x] Record after each compaction pass: segments count, bytes reclaimed, duration

- [x] Task 6: Segment management coordination (AC: #1)
  - [x] Compaction must NOT touch the active (current) segment — only sealed segments
  - [x] `WalWriter::sealed_segment_paths()` returns all segment files except the active one
  - [x] After compaction deletes/replaces a segment, WAL replay still works correctly (segments are read in order)

- [x] Task 7: Tests (AC: #1-#7)
  - [x] Compaction removes dead entries: put messages, delete some, compact, verify segment shrinks
  - [x] Compaction preserves live entries: put messages, compact, WAL replay produces same state
  - [x] TTL expiry: put message with old timestamp, compact with TTL, verify message removed
  - [x] Empty segment after compaction is deleted entirely
  - [x] Concurrent read/write during compaction: spawn compaction, continue writing, verify correctness
  - [x] Storage footprint test: enqueue/ack cycle, compact, measure ratio
  - [x] Compaction of already-compacted segment is a no-op (idempotent)
  - [x] Background thread starts and stops cleanly

## Dev Notes

### Compaction Architecture

```
Background Thread                      Foreground Path
─────────────────                      ───────────────
loop {                                 write_batch() {
  sleep(interval)                        writer.lock() → append WAL
  for seg in sealed_segments() {         indexes.write() → update
    compact_segment(seg)               }
  }                                    get_message() {
}                                        indexes.read() → lookup
                                       }
```

Compaction reads sealed segments (immutable files), checks liveness against the in-memory index, and writes compacted output. It never touches the active segment.

### Liveness Check Strategy

A WAL entry is "live" if the key it references still exists in the in-memory index. For put operations:
- `PutMessage(key, _)`: live if `indexes.messages.contains_key(key)`
- `PutLease(key, _)`: live if `indexes.leases.contains_key(key)`
- `PutLeaseExpiry(key)`: live if `indexes.lease_expiries.contains_key(key)`
- `PutQueue(key, _)`: live if `indexes.queues.contains_key(queue_id)` (parse queue_id from key)
- `PutState(key, _)`: live if `indexes.state.contains_key(key_str)`

Delete operations are always dead after compaction (the original entry they delete is also gone or superseded).

**Important**: The liveness check uses a read lock on indexes — it does NOT block foreground writes.

### Rate Limiting

Use a simple token-bucket or sleep-based approach:
- After writing N bytes of compacted output, sleep for `N / rate_bytes_per_sec` seconds
- Default rate: 10 MB/s — enough to compact a 64MB segment in ~6 seconds without starving foreground I/O
- The rate limit only applies to compaction writes, not reads (reads are sequential and cached by OS)

### Segment Replacement

```
1. Read sealed segment → filter → write to segment-NNNN.wal.tmp
2. fsync the .tmp file
3. rename segment-NNNN.wal.tmp → segment-NNNN.wal (atomic on POSIX)
4. If compacted segment is empty (all dead), delete the file entirely
```

Rename is atomic on POSIX, so a crash during compaction either sees the old segment (pre-compaction) or the new one (post-compaction). The `.tmp` file is cleaned up on restart if it exists.

### Thread Lifecycle

```rust
struct CompactionHandle {
    stop: Arc<AtomicBool>,
    thread: Option<JoinHandle<()>>,
}
```

On `FilaStorage` drop or `flush()`, set `stop = true` and join the thread. The compaction loop checks `stop` between segments and between sleep intervals.

### WalWriter Changes

Need to expose `sealed_segment_paths()` — returns all segment files in data_dir except the active one. The active segment's sequence number is already tracked in `WalWriter`.

```rust
impl WalWriter {
    pub fn sealed_segment_paths(&self) -> io::Result<Vec<PathBuf>> {
        // List segment-*.wal files, exclude current_seq
    }
}
```

### OTel Integration

Follow the pattern from `crates/fila-core/src/telemetry/metrics.rs`:
- Use `opentelemetry::global::meter("fila")`
- Counter for segments_compacted and bytes_reclaimed
- Histogram for duration_seconds

### File Organization

```
crates/fila-core/src/storage/fila/
  mod.rs         — FilaStorage (add CompactionHandle, spawn thread)
  wal.rs         — WalWriter (add sealed_segment_paths)
  config.rs      — FilaStorageConfig (add compaction fields)
  compaction.rs  — NEW: Compactor struct, compact_segment, compaction loop
```

### Testing Strategy

- Disable compaction in most existing tests by setting `compaction_enabled: false` in config
- Compaction-specific tests use small segments (e.g., 1KB) for fast rotation
- TTL tests use `enqueued_at` far in the past (e.g., 1 hour ago) with a short TTL (e.g., 1 second)
- Concurrent test: spawn a compaction pass in a thread, write from main thread, verify no corruption
- All tests use `tempfile::tempdir()`

### References

- [Source: crates/fila-core/src/storage/fila/mod.rs] — FilaStorage with indexes and write_batch
- [Source: crates/fila-core/src/storage/fila/wal.rs] — WalWriter, WalReader, segment management
- [Source: crates/fila-core/src/storage/fila/config.rs] — FilaStorageConfig, SyncMode
- [Source: crates/fila-core/src/storage/traits.rs] — Storage trait interface
- [Source: crates/fila-core/src/telemetry/metrics.rs] — OTel metric registration pattern
- [Source: crates/fila-bench/src/benchmarks/compaction.rs] — existing compaction benchmark (reference for AC #2)
- [Source: _bmad-output/planning-artifacts/epics.md#Epic 13] — epic plan
- [Source: _bmad-output/implementation-artifacts/13-3-read-path-indexing.md] — Story 13.3 completion notes

## Dev Agent Record

### Agent Model Used
Claude Opus 4.6

### Debug Log References
None.

### Completion Notes List
- `compaction.rs` module: `LivenessSnapshot` for dead entry detection, `compact_segment()` for per-segment compaction, `spawn_compaction_thread()` for background lifecycle
- `CompactionMetrics`: 3 OTel instruments (segments_compacted counter, bytes_reclaimed counter, duration_seconds histogram)
- TTL expiry removes messages from in-memory index during compaction (brief write lock)
- Atomic segment replacement: write `.tmp` → fsync → rename (POSIX atomic)
- Empty segments (all dead entries) are deleted entirely
- Rate limiting: sleep proportionally to bytes written, capped at 10ms per sleep
- `FilaStorage` now uses `Arc<Mutex<WalWriter>>` and `Arc<RwLock<Indexes>>` for shared ownership with compaction thread
- WAL helper functions made `pub(super)`: `serialize_entry`, `write_segment_header`, `validate_segment_header`, `read_one_entry`, `EntryError`, `SEGMENT_HEADER_SIZE`
- `WalWriter::sealed_segment_paths()` and `data_dir()` added (non-test-only)
- `WalWriter::current_seq()` changed from `#[cfg(test)]` to public
- Default `compaction_enabled = false` — must be explicitly enabled
- 8 new tests, 329 total tests pass, clippy clean

### File List
- `crates/fila-core/src/storage/fila/compaction.rs` — NEW: compaction logic, background thread, OTel metrics, test helper
- `crates/fila-core/src/storage/fila/config.rs` — added compaction config fields
- `crates/fila-core/src/storage/fila/wal.rs` — exposed helper functions as `pub(super)`, added `sealed_segment_paths()`
- `crates/fila-core/src/storage/fila/mod.rs` — `FilaStorage` uses Arc for shared ownership, spawns compaction thread, 8 new tests
