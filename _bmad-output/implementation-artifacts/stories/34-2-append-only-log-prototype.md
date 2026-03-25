# Story 34.2: Append-Only Log Prototype — Custom Storage Engine

Status: pending

## Story

As a developer,
I want a prototype `AppendOnlyEngine` implementing the `StorageEngine` trait with CommitLog segments and per-(queue, fairness_key) secondary indexes,
So that the viability of custom append-only storage is validated before committing to a full migration.

## Acceptance Criteria

1. **Given** the `StorageEngine` trait (from Epic 13)
   **When** `AppendOnlyEngine` is implemented
   **Then** it supports all trait methods: message CRUD, lease management, queue config, state/config operations, expiry scanning
   **And** it passes the full existing test suite when swapped in for `RocksDbEngine`

2. **Given** the write path
   **When** a message is enqueued
   **Then** the message wire bytes are appended to a shared CommitLog file
   **And** the CommitLog uses 1GB segment files with sequential writes only
   **And** a secondary index entry `(offset, length, msg_id)` is written to a per-(queue, fairness_key) index

3. **Given** the read path (storage fallback for consumer lag)
   **When** a message is read by key
   **Then** the per-(queue, fairness_key) index is consulted to find the CommitLog offset
   **And** the message bytes are read by seeking into the CommitLog segment file

4. **Given** garbage collection
   **When** all messages in a segment are acked
   **Then** the segment file is deleted
   **And** partially-consumed segments are compacted or retained until fully consumed

5. **Given** crash recovery
   **When** the server restarts
   **Then** secondary indexes are rebuilt by replaying the CommitLog from the last checkpoint
   **And** the CommitLog is the single source of truth
   **And** recovery time is bounded and documented

6. **Given** the prototype
   **When** benchmarked against `RocksDbEngine`
   **Then** write throughput comparison is documented
   **And** write amplification is 1x (append-only, no compaction on write path)
   **And** the trade-offs (recovery time, disk space, GC complexity) are documented

## Tasks / Subtasks

- [ ] Task 1: Implement CommitLog
  - [ ] 1.1 Segment file management (create, rotate at 1GB, list segments)
  - [ ] 1.2 Append operation (returns offset + length)
  - [ ] 1.3 Read by offset operation
  - [ ] 1.4 CRC checksums per entry for corruption detection

- [ ] Task 2: Implement secondary indexes
  - [ ] 2.1 Per-(queue, fairness_key) index files with fixed-size entries
  - [ ] 2.2 Index entry: `(commitlog_offset: u64, msg_length: u32, msg_id: [u8; 16])`
  - [ ] 2.3 Append-only index writes (like RocketMQ's ConsumeQueue)

- [ ] Task 3: Implement StorageEngine trait
  - [ ] 3.1 `apply_mutations()` — append to CommitLog + update indexes
  - [ ] 3.2 `get_message()` — index lookup + CommitLog seek
  - [ ] 3.3 Lease and config operations (use RocksDB for metadata, not CommitLog)
  - [ ] 3.4 Expiry scanning via index traversal

- [ ] Task 4: Implement GC
  - [ ] 4.1 Track per-segment ack count
  - [ ] 4.2 Delete fully-consumed segments
  - [ ] 4.3 Handle partially-consumed segments

- [ ] Task 5: Implement crash recovery
  - [ ] 5.1 Checkpoint last-processed CommitLog offset
  - [ ] 5.2 On startup: replay CommitLog from checkpoint to rebuild indexes
  - [ ] 5.3 Measure and document recovery time

- [ ] Task 6: Benchmark and compare
  - [ ] 6.1 Run `fila-bench` enqueue with AppendOnlyEngine
  - [ ] 6.2 Compare write throughput vs RocksDB
  - [ ] 6.3 Measure write amplification (should be 1x)
  - [ ] 6.4 Document trade-offs

## Dev Notes

### Architecture (from RocketMQ)

```
CommitLog (shared, append-only):
  [msg1_bytes][msg2_bytes][msg3_bytes]...
  1GB segment files, sequential writes

Per-(queue, fairness_key) Index:
  Fixed 28-byte entries: (offset: u64, length: u32, msg_id: u128)
  Sequential files, append-only
```

### Hybrid Approach

RocksDB is retained for metadata column families (CF_QUEUES, CF_STATE) — they're small, read-heavy, and well-suited for LSM trees. Only CF_MESSAGES (the write-heavy, large-value column family) is replaced by the CommitLog.

### This is a Prototype

The goal is to validate the approach and measure throughput, not to ship production-ready storage. Story 34.3 (TeeEngine) provides the validation safety net for production cutover.

### Key Files to Create/Modify

| File | Change |
|------|--------|
| `crates/fila-core/src/storage/append_only.rs` | NEW: AppendOnlyEngine |
| `crates/fila-core/src/storage/commitlog.rs` | NEW: CommitLog segment management |
| `crates/fila-core/src/storage/secondary_index.rs` | NEW: Per-(queue, fairness_key) index |
| `crates/fila-core/src/storage/mod.rs` | Register new engine |

### References

- [Source: RocketMQ Storage Mechanism](https://www.alibabacloud.com/blog/an-in-depth-interpretation-of-the-rocketmq-storage-mechanism_599798) — CommitLog + ConsumeQueue
- [Source: BookKeeper Architecture](https://bookkeeper.apache.org/docs/4.5.1/getting-started/concepts/) — Journal + Ledger
- [Source: CockroachDB Pebble Migration](https://www.cockroachlabs.com/blog/pebble-rocksdb-kv-store/) — Storage engine migration playbook
- [Research: technical-kafka-parity-profiling-strategy-2026-03-25.md, Pattern 6] — Append-only log analysis
