# Story 13.2: Write Path — WAL & Segment Log

Status: done

## Story

As a developer,
I want a write-ahead log and segment-based storage engine optimized for queue write patterns,
so that append-heavy workloads achieve higher throughput than RocksDB with predictable latency.

## Critical Context: Design Foundations

**Story 13.1 delivered** the `Storage` trait abstraction with partition-awareness. All broker/scheduler code uses `Storage` trait — no direct RocksDB calls exist outside `storage/rocksdb.rs`.

**This story builds** the write path for a new `FilaStorage` engine that implements the `Storage` trait. The engine uses an append-only WAL with segment rotation — optimized for queue workloads where writes are sequential appends (enqueue) and deletes (ack).

**What is NOT in scope:**
- Read path / indexes (Story 13.3)
- Background compaction (Story 13.4)
- Integration with broker / cutover (Story 13.5)
- The `FilaStorage` does NOT need to pass all `Storage` trait tests yet — only write-path methods are implemented here. Read methods can return empty/None until Story 13.3.

## Acceptance Criteria

1. **Given** a new `FilaStorage` struct implementing the `Storage` trait
   **When** a write operation (enqueue, ack, nack, lease, config) is performed
   **Then** the operation is serialized as a WAL entry and appended to the current segment file
   **And** the WAL entry format includes: entry length, CRC32 checksum, operation type, serialized payload

2. **Given** WAL fsync configuration
   **When** `sync_mode` is set to `EveryBatch` (default)
   **Then** every `write_batch()` call fsyncs the WAL before returning
   **When** `sync_mode` is set to `Interval(duration_ms)`
   **Then** fsync is deferred and performed by a background timer at the configured interval
   **And** `flush()` always forces an immediate fsync regardless of mode

3. **Given** segment size configuration (default: 64MB)
   **When** the current segment exceeds the configured size after a write
   **Then** the current segment is sealed (marked immutable)
   **And** a new segment is created with an incrementing sequence number
   **And** the sealed segment's file is not modified after sealing

4. **Given** a `write_batch()` call with multiple `WriteBatchOp` entries
   **When** the batch is written
   **Then** all operations in the batch are serialized into a single WAL entry (atomic batch)
   **And** a partial batch is never visible (all-or-nothing on replay)

5. **Given** a crash mid-write (simulated by truncating the last WAL entry)
   **When** the engine restarts and replays the WAL
   **Then** complete entries are replayed successfully
   **And** the truncated/corrupt last entry is detected via CRC mismatch and skipped
   **And** no data corruption propagates

6. **Given** individual write methods on the `Storage` trait (put_message, put_lease, etc.)
   **When** called on `FilaStorage`
   **Then** each is implemented as a single-op `write_batch()` call
   **And** delete methods (delete_message, delete_lease, etc.) also append tombstone WAL entries

7. **Given** read methods on the `Storage` trait (get_message, list_messages, etc.)
   **When** called on `FilaStorage` in this story
   **Then** they return `Ok(None)` / `Ok(vec![])` as stubs
   **And** a `todo!()` or log warning indicates they are not yet implemented (Story 13.3)

## Tasks / Subtasks

- [x] Task 1: Define WAL entry format and segment file layout (AC: #1, #3)
  - [x] Define `WalEntry` struct with `WalOp` (tag, key, optional value)
  - [x] Batch entry: `WalEntry` wraps Vec<WalOp> for atomic batches (AC #4)
  - [x] Segment file naming: `segment-{sequence_number:010}.wal` (zero-padded)
  - [x] Segment header: magic bytes "FILA", version u16, creation timestamp u64, 14 reserved bytes (32 total)

- [x] Task 2: Implement WAL writer (AC: #1, #2, #4)
  - [x] `WalWriter` struct with active segment file handle, sync mode, size tracking
  - [x] `append()`: serialize entry → compute CRC32 → write len+CRC+payload → conditional fsync
  - [x] `SyncMode` enum: `EveryBatch` | `Interval(u64)` (ms)
  - [x] Batch writes: all ops serialized as single WAL entry (op_count + individual ops)
  - [x] `fsync()`: force fsync of current segment

- [x] Task 3: Implement segment rotation (AC: #3)
  - [x] Track `current_size` in `WalWriter`
  - [x] After each write, check if segment exceeds max size → rotate
  - [x] `rotate()`: fsync current, create new segment with next sequence number
  - [x] Sealed segments are append-closed (no further writes)

- [x] Task 4: Implement WAL replay for crash recovery (AC: #5)
  - [x] `WalReader` iterates over all segment files in sequence order
  - [x] Read entries: parse length → read payload → verify CRC → yield entry
  - [x] Truncated last entry: detect short read → skip and log warning
  - [x] CRC mismatch: treat as end-of-log and log warning
  - [x] Returns Vec<WalEntry> of valid entries

- [x] Task 5: Implement `FilaStorage` struct with `Storage` trait (AC: #1, #6, #7)
  - [x] `FilaStorage` in `storage/fila/mod.rs` with `Mutex<WalWriter>`
  - [x] All write methods delegate to `write_batch()` with single-op batches
  - [x] `write_batch()`: convert WriteBatchOp → WalOp, append to WAL
  - [x] All read methods stubbed with `Ok(None)` / `Ok(vec![])` + tracing warn
  - [x] `flush()`: delegates to WAL writer fsync

- [x] Task 6: Configuration (AC: #2, #3)
  - [x] `FilaStorageConfig`: data_dir, segment_size_bytes (default 64MB), sync_mode (default EveryBatch)
  - [x] `FilaStorage::open(config)` constructor

- [x] Task 7: Tests (AC: #1-#6)
  - [x] `wal_append_and_replay_roundtrip`: write 10, close, replay, verify all 10
  - [x] `segment_rotation`: small segment size, verify multiple files, replay all 20
  - [x] `crash_recovery_truncated_last_entry`: write 5, truncate, replay 4
  - [x] `crc_integrity_corrupt_byte`: write 3, corrupt, replay 2
  - [x] `batch_atomicity`: 3-op batch, verify single WAL entry with all ops
  - [x] `fila_storage_implements_storage_trait`: write via trait, verify WAL
  - [x] `segment_ordering_across_multiple_segments`: 30 entries, 3+ segments, verify order
  - [x] `serialization_roundtrip_all_op_types`: all 10 OpTag variants
  - [x] `empty_directory_replay_returns_nothing`: empty dir
  - [x] `writer_reopens_existing_segment`: write 3, close, write 2 more, verify 5
  - [x] `fila_storage_write_batch_atomic`: 3-op batch via Storage trait
  - [x] `fila_storage_read_stubs_return_empty`: all read methods return empty
  - [x] `fila_storage_empty_write_batch_is_noop`: empty batch produces no WAL entries

## Dev Notes

### Architecture

The WAL is the foundation of the custom storage engine. All state changes flow through it:

```
write_batch(ops) → serialize → CRC → append to WAL segment → fsync → Ok
```

The read path (Story 13.3) will rebuild in-memory indexes from WAL replay on startup. Background compaction (Story 13.4) will merge and remove acknowledged messages from sealed segments.

### WAL Entry Binary Format

```
+--------+--------+--------+------------------+
| len(4) | crc(4) | tag(1) | payload(len - 1) |
+--------+--------+--------+------------------+
```

- `len`: u32 LE — length of tag + payload (excludes the length and CRC fields themselves)
- `crc`: u32 LE — CRC32 (IEEE) of tag + payload bytes
- `tag`: u8 — operation type discriminant
- `payload`: serialized operation data

### Segment File Layout

```
data_dir/
  segment-0000000001.wal    # first segment
  segment-0000000002.wal    # second segment (after rotation)
  ...
```

Each segment starts with a header:
- Magic bytes: `FILA` (4 bytes)
- Version: u16 LE (1)
- Created timestamp: u64 LE (nanos since epoch)
- Reserved: 14 bytes (zero-filled, for future use)
- Total header: 32 bytes

### Operation Serialization

Map `WriteBatchOp` variants to WAL operations. Use `serde` with bincode or a custom compact format. Bincode is recommended for speed and compactness.

For batch entries, use a batch header tag followed by count + individual ops.

### Sync Mode Design

```rust
pub enum SyncMode {
    EveryBatch,          // fsync after every write_batch() — safest, slower
    Interval(u64),       // fsync every N ms — faster, slight durability risk
}
```

Default is `EveryBatch` for safety. `Interval` mode would need a background thread or cooperative flushing — for this story, implement `EveryBatch` fully and define the `Interval` variant as a type but defer its background timer to Story 13.4 or later if needed.

### File Organization

```
crates/fila-core/src/storage/
  mod.rs           — existing, add fila module
  traits.rs        — existing Storage trait
  rocksdb.rs       — existing RocksDB adapter
  keys.rs          — existing key encoding
  fila/
    mod.rs         — FilaStorage struct, Storage trait impl
    wal.rs         — WalWriter, WalReader, WalEntry, segment management
    config.rs      — FilaStorageConfig, SyncMode
```

### Dependencies

- `crc32fast` crate for CRC32 computation (fast, widely used)
- `bincode` crate for compact binary serialization (already available or easy to add)
- `serde` already in use throughout the project

### Error Handling

WAL errors map to `StorageError::Backend(String)` — the backend-agnostic variant from Story 13.1. Specific failure modes:
- I/O errors during write → `StorageError::Backend`
- CRC mismatch during replay → log warning, skip entry (not an error returned to caller)
- Segment rotation failure → `StorageError::Backend`

### Testing Strategy

- All WAL tests use temp directories (`tempfile::tempdir()`)
- Crash simulation: write entries, drop writer (no flush), truncate last bytes of file, create new reader and verify replay
- No integration with broker in this story — purely storage-layer unit tests
- `FilaStorage` must satisfy `Storage: Send + Sync` — verify with compile-time assertion

### Project Structure Notes

- New module `storage/fila/` with its own submodules — follows the existing `storage/rocksdb.rs` pattern but with directory structure for multiple files
- `FilaStorage` will be exported from `storage/mod.rs` alongside `RocksDbStorage`
- No changes to `fila-server`, `fila-cli`, or any other crate in this story

### References

- [Source: crates/fila-core/src/storage/traits.rs] — Storage trait with PartitionId
- [Source: crates/fila-core/src/storage/rocksdb.rs] — RocksDB adapter pattern to follow
- [Source: crates/fila-core/src/storage/keys.rs] — key encoding (reused as-is)
- [Source: crates/fila-core/src/error.rs] — StorageError with Backend variant
- [Source: _bmad-output/planning-artifacts/epics.md#Epic 13] — epic plan
- [Source: _bmad-output/planning-artifacts/architecture.md#Data Architecture] — current storage design
- [Source: _bmad-output/implementation-artifacts/13-1-storage-trait-abstraction-rocksdb-adapter.md] — Story 13.1 completion notes

## Dev Agent Record

### Agent Model Used
Claude Opus 4.6

### Debug Log References
None — no debug issues encountered.

### Completion Notes List
- WAL entry format: len(u32 LE) + crc32(u32 LE) + payload. Payload = op_count(u32 LE) + ops. Each op = tag(u8) + key_len(u32 LE) + key + [value_len(u32 LE) + value]
- Segment header: 32 bytes (magic "FILA", version u16, timestamp u64, 14 reserved)
- `FilaStorage` wraps `Mutex<WalWriter>` for thread safety (Send + Sync)
- Read methods are stubbed with `warn!` logging — will be implemented in Story 13.3
- Added `crc32fast` crate to workspace dependencies
- 13 new tests (9 WAL-level, 4 FilaStorage-level), 307 total tests pass
- No bincode dependency needed — custom compact binary format for WAL entries

### File List
- `Cargo.toml` — added crc32fast workspace dependency
- `crates/fila-core/Cargo.toml` — added crc32fast dependency
- `crates/fila-core/src/storage/mod.rs` — added fila module, exports FilaStorage + FilaStorageConfig
- `crates/fila-core/src/storage/fila/mod.rs` — FilaStorage struct, Storage trait impl, tests
- `crates/fila-core/src/storage/fila/wal.rs` — WalWriter, WalReader, WalEntry, WalOp, OpTag, segment management, tests
- `crates/fila-core/src/storage/fila/config.rs` — FilaStorageConfig, SyncMode
