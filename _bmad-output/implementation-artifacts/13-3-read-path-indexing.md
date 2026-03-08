# Story 13.3: Read Path & Indexing

Status: review

## Story

As a developer,
I want in-memory indexes that enable O(1) lookups for all storage read operations,
so that the Fila storage engine can serve reads efficiently from WAL-replay state without scanning segment files.

## Critical Context

**Story 13.2 delivered** the WAL write path: `FilaStorage` appends all writes to segment files. Read methods are currently stubbed (`Ok(None)` / `Ok(vec![])`).

**This story implements** the read path by building in-memory indexes from WAL replay on startup and maintaining them incrementally on each write. After this story, `FilaStorage` will fully implement the `Storage` trait — all read and write methods functional.

**What is NOT in scope:**
- Background compaction / segment merging (Story 13.4)
- Integration with broker / cutover (Story 13.5)
- TTL expiry processing (Story 13.4) — but the index for querying expired leases IS in scope

## Acceptance Criteria

1. **Given** WAL segments on disk
   **When** `FilaStorage::open()` is called
   **Then** all segments are replayed in order
   **And** in-memory indexes are rebuilt from the replayed operations
   **And** deletes/tombstones correctly remove entries from indexes

2. **Given** a `put_message()` followed by `get_message()` on the same key
   **When** the message is retrieved
   **Then** the returned message matches exactly what was stored
   **And** the lookup is O(1) via the message index (no segment scan)

3. **Given** multiple messages in the same queue
   **When** `list_messages(prefix)` is called
   **Then** all messages matching the prefix are returned in lexicographic key order
   **And** deleted messages are NOT returned

4. **Given** lease operations (`put_lease`, `get_lease`, `delete_lease`)
   **When** leases are stored and retrieved
   **Then** the lease index supports O(1) lookup by key
   **And** `list_expired_leases(up_to_key)` returns lease expiry entries whose keys are <= the bound, sorted ascending

5. **Given** queue config operations (`put_queue`, `get_queue`, `delete_queue`, `list_queues`)
   **When** queue configs are stored and retrieved
   **Then** the queue index supports O(1) lookup by queue_id
   **And** `list_queues()` returns all stored queue configs

6. **Given** state operations (`put_state`, `get_state`, `delete_state`, `list_state_by_prefix`)
   **When** state key-values are stored and retrieved
   **Then** the state index supports O(1) lookup by key
   **And** `list_state_by_prefix(prefix, limit)` returns matching entries up to the limit

7. **Given** incremental index maintenance
   **When** a `write_batch()` is called after startup
   **Then** the in-memory indexes are updated immediately (before returning Ok)
   **And** subsequent reads reflect the write without re-replaying the WAL

## Tasks / Subtasks

- [x] Task 1: Define in-memory index structures (AC: #2-#6)
  - [x] Message index: `BTreeMap<Vec<u8>, Message>` — sorted by key for prefix scans
  - [x] Lease index: `HashMap<Vec<u8>, Vec<u8>>` — key → value for O(1) lookup
  - [x] Lease expiry index: `BTreeMap<Vec<u8>, ()>` — sorted for range queries (list_expired_leases)
  - [x] Queue index: `HashMap<String, QueueConfig>` — queue_id → config
  - [x] State index: `BTreeMap<String, Vec<u8>>` — sorted for prefix scans

- [x] Task 2: Implement WAL replay into indexes (AC: #1)
  - [x] On `FilaStorage::open()`, after opening WAL writer, replay all entries
  - [x] For each WAL op, apply to the appropriate index (put adds, delete removes)
  - [x] Batch entries apply atomically (all ops in batch applied together)

- [x] Task 3: Implement incremental index updates (AC: #7)
  - [x] After each `write_batch()` WAL append, apply the same ops to indexes
  - [x] Ensure index updates happen within the same lock scope as WAL append

- [x] Task 4: Implement read methods (AC: #2-#6)
  - [x] `get_message()`: lookup in message BTreeMap
  - [x] `list_messages(prefix)`: range scan on message BTreeMap using prefix bounds
  - [x] `get_lease()`: lookup in lease HashMap
  - [x] `list_expired_leases(up_to_key)`: range scan on lease expiry BTreeMap
  - [x] `get_queue()`: lookup in queue HashMap
  - [x] `list_queues()`: collect all values from queue HashMap
  - [x] `get_state()`: lookup in state BTreeMap
  - [x] `list_state_by_prefix()`: range scan on state BTreeMap using prefix bounds with limit
  - [x] `delete_message()`, `delete_lease()`, `delete_queue()`, `delete_state()`: remove from index + WAL append

- [x] Task 5: Tests (AC: #1-#7)
  - [x] Unit test: put + get roundtrip for messages, leases, queues, state
  - [x] Unit test: list_messages with prefix filtering
  - [x] Unit test: list_expired_leases range query
  - [x] Unit test: list_state_by_prefix with limit
  - [x] Unit test: delete removes from index (get returns None after delete)
  - [x] Unit test: WAL replay rebuilds indexes (write, close, reopen, verify reads)
  - [x] Unit test: write_batch with mixed ops updates all indexes
  - [x] Unit test: overwrite (put same key twice, get returns latest)

## Dev Notes

### Index Architecture

```
FilaStorage {
    writer: Mutex<WalWriter>,
    indexes: RwLock<Indexes>,
}

struct Indexes {
    messages: BTreeMap<Vec<u8>, Message>,
    leases: HashMap<Vec<u8>, Vec<u8>>,
    lease_expiries: BTreeMap<Vec<u8>, ()>,
    queues: HashMap<String, QueueConfig>,
    state: BTreeMap<String, Vec<u8>>,
}
```

Using `RwLock<Indexes>` allows concurrent reads while writes hold exclusive lock. The writer `Mutex` serializes WAL appends, and the index `RwLock` allows reads to proceed independently of each other.

### Locking Strategy

`write_batch()` flow:
1. Acquire writer Mutex → append to WAL → acquire indexes write lock → apply ops to indexes
2. Release both locks

Read methods:
1. Acquire indexes read lock → lookup/scan → release

The writer Mutex is held across both WAL append and index update to prevent concurrent write_batch calls from reordering index updates relative to WAL order. Without this, in-memory state could diverge from what WAL replay produces on restart. (Fixed per Cubic review finding on PR #51.)

### Prefix Scan with BTreeMap

BTreeMap's `range()` method supports efficient prefix scanning:
```rust
use std::ops::Bound;
let start = prefix.to_vec();
let mut end = prefix.to_vec();
// Increment the last byte to get the exclusive upper bound
if let Some(last) = end.last_mut() {
    *last = last.wrapping_add(1);
    // If it wrapped to 0, we need to handle the carry
}
btree.range(start..end)
```

For `list_expired_leases(up_to_key)`, use `..=up_to_key` range.

### WAL Op to Index Update Mapping

| WAL Op | Index | Action |
|--------|-------|--------|
| PutMessage(key, value) | messages | Deserialize Message, insert at key |
| DeleteMessage(key) | messages | Remove key |
| PutLease(key, value) | leases | Insert key → value |
| DeleteLease(key) | leases | Remove key |
| PutLeaseExpiry(key) | lease_expiries | Insert key |
| DeleteLeaseExpiry(key) | lease_expiries | Remove key |
| PutQueue(key, value) | queues | Deserialize QueueConfig, insert queue_id → config |
| DeleteQueue(key) | queues | Remove queue_id |
| PutState(key, value) | state | Insert key (as String) → value |
| DeleteState(key) | state | Remove key (as String) |

### Deserialization

Message and QueueConfig values in the WAL are JSON-serialized (same as RocksDB adapter). Use `serde_json::from_slice()` during replay and index updates.

### File Organization

All changes in `crates/fila-core/src/storage/fila/mod.rs` — the indexes are part of `FilaStorage`. No new files needed.

### Testing Strategy

- All tests use `FilaStorage` through the `Storage` trait interface
- WAL replay test: create storage, write data, drop, reopen from same dir, verify reads
- All existing WAL tests continue to pass (write-path unchanged)
- Use `PartitionId::DEFAULT` for all tests (same as scheduler tests)

### References

- [Source: crates/fila-core/src/storage/fila/mod.rs] — current FilaStorage with stubbed reads
- [Source: crates/fila-core/src/storage/fila/wal.rs] — WAL reader/writer
- [Source: crates/fila-core/src/storage/traits.rs] — Storage trait interface
- [Source: crates/fila-core/src/storage/rocksdb.rs] — reference implementation for behavior
- [Source: _bmad-output/planning-artifacts/epics.md#Epic 13] — epic plan
- [Source: _bmad-output/implementation-artifacts/13-2-write-path-wal-segment-log.md] — Story 13.2 completion notes

## Dev Agent Record

### Agent Model Used
Claude Opus 4.6

### Debug Log References
None.

### Completion Notes List
- `Indexes` struct with 5 collections: BTreeMap<Vec<u8>, Message>, HashMap<Vec<u8>, Vec<u8>>, BTreeMap<Vec<u8>, ()>, HashMap<String, QueueConfig>, BTreeMap<String, Vec<u8>>
- WAL replay on `open()` rebuilds all indexes, including delete/tombstone processing
- `RwLock<Indexes>` for concurrent reads, `Mutex<WalWriter>` for serialized writes
- `write_batch()` appends to WAL then immediately updates indexes
- `prefix_upper_bound()` helper for BTreeMap range scans (handles 0xFF edge case)
- 14 new tests, 321 total tests pass, clippy clean
- Removed old `fila_storage_read_stubs_return_empty` test (reads now return real data)

### File List
- `crates/fila-core/src/storage/fila/mod.rs` — added Indexes struct, WAL replay, all read method implementations, 14 new tests
