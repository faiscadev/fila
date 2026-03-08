# Story 13.1: Storage Trait Abstraction & RocksDB Adapter

Status: ready-for-dev

## Story

As a developer,
I want a clean storage engine trait that abstracts away the storage backend with partition-namespace awareness,
so that the storage implementation can be swapped without changing broker logic and the design is ready for future multi-partition clustering.

## Critical Context: Existing Abstraction

**The `Storage` trait already exists** at `crates/fila-core/src/storage/traits.rs`. RocksDB is already isolated in `storage/rocksdb.rs`. All broker/scheduler code already uses the trait — no direct RocksDB calls exist outside `storage/rocksdb.rs`.

**What this story actually delivers:**
1. **Partition-namespace awareness** — all trait operations accept a partition identifier (initially a single default partition)
2. **Trait completeness audit** — verify every storage operation the scheduler uses is properly abstracted
3. **Clean interface verification** — ensure no RocksDB-specific types leak through the trait
4. **RocksDB adapter hardening** — the existing `RocksDbStorage` becomes an explicit adapter implementing the enhanced trait

This is **NOT** a greenfield trait design — it's an enhancement of the existing abstraction.

## Acceptance Criteria

1. **Given** the existing `Storage` trait at `crates/fila-core/src/storage/traits.rs`
   **When** partition-namespace awareness is added
   **Then** all trait methods accept a `partition: &PartitionId` parameter (or equivalent namespace parameter)
   **And** a `PartitionId` type is defined with a `DEFAULT` constant for single-partition mode
   **And** the default partition preserves current key encoding behavior exactly

2. **Given** the existing `RocksDbStorage` at `crates/fila-core/src/storage/rocksdb.rs`
   **When** it implements the partition-aware `Storage` trait
   **Then** the default partition maps to the current column family key encoding unchanged
   **And** partition-aware key encoding namespaces keys by partition ID when non-default partitions are used
   **And** the existing `WriteBatchOp` enum is extended to carry partition context

3. **Given** all broker/scheduler code already uses `Storage` trait
   **When** the trait signature changes to include partition parameters
   **Then** all call sites in `scheduler/mod.rs`, `handlers.rs`, `admin_handlers.rs`, `recovery.rs`, `delivery.rs` are updated to pass `PartitionId::DEFAULT`
   **And** all existing unit and integration tests pass without modification to test assertions
   **And** the e2e test suite (11 tests) passes

4. **Given** the trait interface
   **When** inspected for RocksDB leakage
   **Then** no RocksDB-specific types appear in the `Storage` trait or `WriteBatchOp` enum
   **And** `StorageError` variants are backend-agnostic (rename `RocksDb` variant to a generic name like `Backend`)

5. **Given** the `storage/keys.rs` key encoding module
   **When** partition-namespacing is implemented
   **Then** key encoding functions accept a partition parameter
   **And** the default partition produces identical keys to current encoding (zero behavioral change)
   **And** non-default partitions prepend a partition namespace to keys

## Tasks / Subtasks

- [ ] Task 1: Add `PartitionId` type (AC: #1)
  - [ ] Define `PartitionId` as a newtype in `storage/mod.rs` or a new `storage/partition.rs`
  - [ ] Implement `PartitionId::DEFAULT` constant for single-partition mode
  - [ ] Implement `Display`, `Clone`, `Debug`, `PartialEq`, `Eq`, `Hash`

- [ ] Task 2: Evolve `Storage` trait with partition awareness (AC: #1, #4)
  - [ ] Add `partition: &PartitionId` parameter to all trait methods
  - [ ] Rename `StorageError::RocksDb` → `StorageError::Backend` for backend-agnosticism
  - [ ] Verify `WriteBatchOp` carries partition context (add `partition: PartitionId` field to each variant, or use a wrapper)
  - [ ] Verify no RocksDB-specific types in trait or WriteBatchOp

- [ ] Task 3: Update key encoding for partition awareness (AC: #5)
  - [ ] Update `storage/keys.rs` functions to accept partition parameter
  - [ ] Default partition produces identical keys to current encoding
  - [ ] Non-default partitions add namespace prefix

- [ ] Task 4: Update `RocksDbStorage` implementation (AC: #2)
  - [ ] Implement updated `Storage` trait on `RocksDbStorage`
  - [ ] Map partition parameter into key encoding
  - [ ] Ensure atomic write batches handle partition-aware keys

- [ ] Task 5: Update all scheduler call sites (AC: #3)
  - [ ] Update `scheduler/mod.rs` — all `self.storage.*()` calls pass `PartitionId::DEFAULT`
  - [ ] Update `scheduler/handlers.rs` — enqueue, ack, nack operations
  - [ ] Update `scheduler/admin_handlers.rs` — config, stats, redrive, list operations
  - [ ] Update `scheduler/recovery.rs` — crash recovery storage calls
  - [ ] Update `scheduler/delivery.rs` — delivery storage calls
  - [ ] Update broker `mod.rs` if it calls storage directly

- [ ] Task 6: Update tests (AC: #3)
  - [ ] Update unit tests in `scheduler/tests/` to pass `PartitionId::DEFAULT`
  - [ ] Update any storage-level tests
  - [ ] Run full test suite: `cargo test --workspace`
  - [ ] Run e2e tests: `cargo test -p fila-e2e`
  - [ ] Run clippy: `cargo clippy --workspace --all-targets`

## Dev Notes

### Existing Code Map

| Component | File | Relevance |
|-----------|------|-----------|
| Storage trait | `crates/fila-core/src/storage/traits.rs` | **Primary target** — enhance with partition awareness |
| RocksDB impl | `crates/fila-core/src/storage/rocksdb.rs` | **Primary target** — update to implement new trait |
| Key encoding | `crates/fila-core/src/storage/keys.rs` | **Update** — add partition namespace support |
| Storage mod | `crates/fila-core/src/storage/mod.rs` | Re-exports, add PartitionId |
| Error types | `crates/fila-core/src/error.rs` | Rename `StorageError::RocksDb` → `Backend` |
| Scheduler core | `crates/fila-core/src/broker/scheduler/mod.rs` | Update storage calls |
| Handlers | `crates/fila-core/src/broker/scheduler/handlers.rs` | Update storage calls |
| Admin handlers | `crates/fila-core/src/broker/scheduler/admin_handlers.rs` | Update storage calls |
| Recovery | `crates/fila-core/src/broker/scheduler/recovery.rs` | Update storage calls |
| Delivery | `crates/fila-core/src/broker/scheduler/delivery.rs` | Update storage calls |
| Metrics recording | `crates/fila-core/src/broker/scheduler/metrics_recording.rs` | Check if it calls storage |

### Existing Storage Trait Methods (current signatures)

```rust
pub trait Storage: Send + Sync {
    fn put_message(&self, key: &[u8], message: &Message) -> StorageResult<()>;
    fn get_message(&self, key: &[u8]) -> StorageResult<Option<Message>>;
    fn delete_message(&self, key: &[u8]) -> StorageResult<()>;
    fn list_messages(&self, prefix: &[u8]) -> StorageResult<Vec<(Vec<u8>, Message)>>;
    fn put_lease(&self, key: &[u8], value: &[u8]) -> StorageResult<()>;
    fn get_lease(&self, key: &[u8]) -> StorageResult<Option<Vec<u8>>>;
    fn delete_lease(&self, key: &[u8]) -> StorageResult<()>;
    fn list_expired_leases(&self, up_to_key: &[u8]) -> StorageResult<Vec<Vec<u8>>>;
    fn put_queue(&self, queue_id: &str, config: &QueueConfig) -> StorageResult<()>;
    fn get_queue(&self, queue_id: &str) -> StorageResult<Option<QueueConfig>>;
    fn delete_queue(&self, queue_id: &str) -> StorageResult<()>;
    fn list_queues(&self) -> StorageResult<Vec<QueueConfig>>;
    fn put_state(&self, key: &str, value: &[u8]) -> StorageResult<()>;
    fn get_state(&self, key: &str) -> StorageResult<Option<Vec<u8>>>;
    fn delete_state(&self, key: &str) -> StorageResult<()>;
    fn list_state_by_prefix(&self, prefix: &str, limit: usize) -> StorageResult<Vec<(String, Vec<u8>)>>;
    fn write_batch(&self, ops: Vec<WriteBatchOp>) -> StorageResult<()>;
    fn flush(&self) -> StorageResult<()>;
}
```

### Design Decisions

1. **PartitionId approach**: Use a lightweight newtype (`PartitionId(String)` or `PartitionId(u32)`) — keep it simple. A `u32` is sufficient for partition IDs and avoids string allocation on every call. Use `PartitionId::DEFAULT` (value 0) for single-partition mode.

2. **Key namespacing**: For default partition, keys remain exactly as they are today. For non-default partitions, prepend a partition-length-prefixed namespace. This ensures backward compatibility.

3. **WriteBatchOp evolution**: Two options:
   - Add `partition: PartitionId` to each variant — verbose but explicit
   - Wrap: `struct PartitionedOp { partition: PartitionId, op: WriteBatchOp }` — cleaner
   Choose the cleaner approach (wrapper struct) to avoid touching every variant.

4. **StorageError::RocksDb rename**: This variant holds a `String` error message. Rename to `Backend(String)` — the error message already contains the details. Update all match arms in `rocksdb.rs` that construct this variant.

### Error Pattern (per CLAUDE.md)

```rust
// Explicit error mapping — match on all variants, no catch-all
.map_err(|err| match err {
    StorageError::Backend(msg) => ...,
    StorageError::Serialization(msg) => ...,
    StorageError::ColumnFamilyNotFound(name) => ...,
    StorageError::CorruptData(msg) => ...,
})
```

### Column Families (5 CFs in RocksDB)

1. `messages` — key: `{queue_id}:{fairness_key}:{ts}:{msg_id}`
2. `leases` — key: `{queue_id}:{msg_id}`
3. `lease_expiry` — key: `{expiry_ts}:{queue_id}:{msg_id}`
4. `queues` — key: `{queue_id}`
5. `state` — key: arbitrary strings (e.g., `throttle.{key}`)

### Testing Strategy

- **Zero assertion changes**: All existing tests must pass with only call-site updates (adding `PartitionId::DEFAULT`)
- **New tests**: Add at least one test verifying non-default partition key encoding produces namespaced keys
- **E2E tests**: Must pass — they test through the full stack
- **CI**: Existing CI pipeline covers this crate, no new CI setup needed

### Project Structure Notes

- All changes are within `crates/fila-core/src/` — no new crates
- `storage/` module gains the `PartitionId` type (in `mod.rs` or new `partition.rs`)
- Public API of fila-core changes (trait signature) — update `lib.rs` exports if needed
- No changes to `fila-server`, `fila-cli`, `fila-sdk`, `fila-bench`, or `fila-e2e` source code (they use the broker, not storage directly) — but they must compile and pass tests

### References

- [Source: crates/fila-core/src/storage/traits.rs] — existing Storage trait
- [Source: crates/fila-core/src/storage/rocksdb.rs] — RocksDB implementation
- [Source: crates/fila-core/src/storage/keys.rs] — key encoding
- [Source: crates/fila-core/src/error.rs] — StorageError definition
- [Source: _bmad-output/planning-artifacts/epics.md#Epic 13] — epic plan and ACs
- [Source: _bmad-output/planning-artifacts/architecture.md] — architecture decisions

## Dev Agent Record

### Agent Model Used

### Debug Log References

### Completion Notes List

### File List
