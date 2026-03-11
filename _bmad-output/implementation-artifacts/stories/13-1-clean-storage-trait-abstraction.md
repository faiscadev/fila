# Story 13.1: Clean Storage Trait Abstraction

Status: ready-for-dev

## Story

As a developer,
I want a clean storage engine trait using Fila-domain terms,
so that the storage implementation can be swapped without changing broker logic, and the interface is ready for Raft state machine application.

## Acceptance Criteria

1. A `StorageEngine` trait is defined with methods covering all current storage operations: message CRUD, lease management, queue config, state/config operations, expiry scanning.
2. The trait uses Fila-domain terms: message store, lease store, config store — not RocksDB concepts (column families, raw iterators, write batches).
3. The trait does NOT use PartitionId — queues are the unit of distribution in the Raft-per-queue model.
4. The trait supports atomic batch mutations (`apply_mutations(batch)`) suitable for Raft state machine application (applying committed log entries).
5. A `RocksDbEngine` struct implements the `StorageEngine` trait, wrapping all existing RocksDB logic.
6. All broker and scheduler code is migrated from direct `Storage` calls to `StorageEngine` trait methods.
7. An `InMemoryEngine` implementation is provided for unit tests (enables faster, deterministic scheduler testing).
8. All existing unit and integration tests pass without modification to test assertions (only internal wiring changes).
9. The e2e test suite (11 tests) passes with the RocksDB adapter.
10. The trait is defined in fila-core with no RocksDB-specific types in the trait interface (RocksDB is an implementation detail).

## Tasks / Subtasks

- [ ] Task 1: Rename `Storage` to `StorageEngine` and `RocksDbStorage` to `RocksDbEngine` (AC: 1, 5, 6)
  - [ ] 1.1 Rename the trait in `crates/fila-core/src/storage/traits.rs` from `Storage` to `StorageEngine`
  - [ ] 1.2 Rename the struct in `crates/fila-core/src/storage/rocksdb.rs` from `RocksDbStorage` to `RocksDbEngine`
  - [ ] 1.3 Update `crates/fila-core/src/storage/mod.rs` re-exports
  - [ ] 1.4 Update `crates/fila-core/src/lib.rs` public exports
  - [ ] 1.5 Update all references in broker/scheduler code (`handlers.rs`, `admin_handlers.rs`, `delivery.rs`, `recovery.rs`, `mod.rs`)
  - [ ] 1.6 Update `crates/fila-core/src/lua/bridge.rs` (passes `Arc<dyn Storage>`)
  - [ ] 1.7 Update `crates/fila-core/src/broker/mod.rs` (creates scheduler with storage)
  - [ ] 1.8 Update `crates/fila-server/src/main.rs` (creates `RocksDbStorage`)
  - [ ] 1.9 Update `crates/fila-sdk/` if it references storage types
  - [ ] 1.10 Update all test files in `crates/fila-core/src/broker/scheduler/tests/`
  - [ ] 1.11 Update e2e tests in `crates/fila-e2e/`

- [ ] Task 2: Rename `WriteBatchOp` to `Mutation` and `write_batch` to `apply_mutations` (AC: 2, 4)
  - [ ] 2.1 Rename `WriteBatchOp` enum to `Mutation` in `traits.rs`
  - [ ] 2.2 Rename `write_batch` method to `apply_mutations` on the trait
  - [ ] 2.3 Update `RocksDbEngine` implementation
  - [ ] 2.4 Update all call sites in scheduler code
  - [ ] 2.5 Update lib.rs exports

- [ ] Task 3: Review and clean trait documentation (AC: 2)
  - [ ] 3.1 Update trait doc comments to use "message store", "lease store", "config store" terminology
  - [ ] 3.2 Remove any mention of "CF" or "column family" from trait-level docs
  - [ ] 3.3 Add doc comment to `apply_mutations` explaining Raft state machine suitability

- [ ] Task 4: Verify no PartitionId concept exists (AC: 3)
  - [ ] 4.1 Search codebase for PartitionId — confirm absent (it should be, based on current design)

- [ ] Task 5: Implement `InMemoryEngine` (AC: 7)
  - [ ] 5.1 Create `crates/fila-core/src/storage/memory.rs`
  - [ ] 5.2 Implement `StorageEngine` using `HashMap`/`BTreeMap` behind a `Mutex`
  - [ ] 5.3 Message store: `BTreeMap<Vec<u8>, Message>` (BTreeMap for correct range scans/prefix iteration)
  - [ ] 5.4 Lease store: `BTreeMap<Vec<u8>, Vec<u8>>`
  - [ ] 5.5 Lease expiry store: `BTreeSet<Vec<u8>>` (sorted for `list_expired_leases`)
  - [ ] 5.6 Queue store: `HashMap<String, QueueConfig>`
  - [ ] 5.7 State store: `BTreeMap<String, Vec<u8>>` (BTreeMap for prefix scans)
  - [ ] 5.8 `apply_mutations` applies all ops atomically (hold lock for entire batch)
  - [ ] 5.9 `flush` is a no-op
  - [ ] 5.10 Export from `storage/mod.rs` and `lib.rs`

- [ ] Task 6: Add storage engine tests for InMemoryEngine (AC: 7, 8)
  - [ ] 6.1 Create a shared test suite that runs against any `StorageEngine` impl
  - [ ] 6.2 Run the shared suite against both `RocksDbEngine` and `InMemoryEngine`
  - [ ] 6.3 Cover: put/get/delete for messages, leases, queues, state
  - [ ] 6.4 Cover: prefix listing, expired lease listing, batch mutations
  - [ ] 6.5 Cover: atomicity of apply_mutations (all-or-nothing)

- [ ] Task 7: Rename `StorageError` variants (AC: 2, 10)
  - [ ] 7.1 Rename `StorageError::RocksDb` to `StorageError::Engine` — this is the generic "engine failed" variant
  - [ ] 7.2 Rename `StorageError::ColumnFamilyNotFound` to `StorageError::StoreNotFound` — "store" not "column family"
  - [ ] 7.3 Update error conversion `From<rocksdb::Error>` to map to `Engine` variant
  - [ ] 7.4 Update any code matching on these variants

- [ ] Task 8: Verify all tests pass (AC: 8, 9)
  - [ ] 8.1 Run `cargo test --workspace` — all 278 tests pass
  - [ ] 8.2 Run `cargo clippy --workspace` — no warnings
  - [ ] 8.3 Run e2e test suite — all 11 tests pass

## Dev Notes

### Current State (What Already Works)

The codebase already has a well-designed `Storage` trait in `crates/fila-core/src/storage/traits.rs`. RocksDB types are fully encapsulated in `rocksdb.rs` — zero public leaks. The trait uses domain types (`Message`, `QueueConfig`, `&[u8]` keys). All 17 RocksDB call sites are in one file. Tests already use `Arc<dyn Storage>` (12 instances).

**This story is primarily a rename + InMemoryEngine addition, not a redesign.** The existing abstraction is solid.

### Key Files to Modify

| File | Change |
|------|--------|
| `crates/fila-core/src/storage/traits.rs` | Rename trait, enum, method |
| `crates/fila-core/src/storage/rocksdb.rs` | Rename struct |
| `crates/fila-core/src/storage/mod.rs` | Update re-exports, add memory module |
| `crates/fila-core/src/storage/memory.rs` | **NEW** — InMemoryEngine |
| `crates/fila-core/src/lib.rs` | Update public exports |
| `crates/fila-core/src/error.rs` | Rename StorageError variants |
| `crates/fila-core/src/broker/mod.rs` | Update type references |
| `crates/fila-core/src/broker/scheduler/*.rs` | Update type references |
| `crates/fila-core/src/lua/bridge.rs` | Update `Arc<dyn Storage>` → `Arc<dyn StorageEngine>` |
| `crates/fila-server/src/main.rs` | Update `RocksDbStorage` → `RocksDbEngine` |
| All test files | Update type references |

### InMemoryEngine Design

Use `parking_lot::Mutex` (already in the workspace) wrapping an inner struct with `BTreeMap`s. BTreeMap (not HashMap) for stores that need range/prefix scans:

```rust
struct Inner {
    messages: BTreeMap<Vec<u8>, Message>,
    leases: BTreeMap<Vec<u8>, Vec<u8>>,
    lease_expiry: BTreeSet<Vec<u8>>,
    queues: HashMap<String, QueueConfig>,
    state: BTreeMap<String, Vec<u8>>,
}

pub struct InMemoryEngine {
    inner: Mutex<Inner>,
}
```

For `list_messages(prefix)`: use `BTreeMap::range` with the prefix as lower bound, iterate while key starts with prefix.
For `list_expired_leases(up_to)`: use `BTreeSet::range(..=up_to)`.

### Error Handling Pattern

Follow CLAUDE.md: explicit variant matching, preserved context. The `StorageError::Engine(String)` variant replaces `RocksDb(String)` — keeps the same shape but removes RocksDB naming from the public error type. `InMemoryEngine` should never produce `Engine` errors (in-memory ops don't fail) but may produce `Serialization` errors from serde.

### What NOT to Do

- Do NOT change the trait method signatures beyond renaming. The method shapes are correct.
- Do NOT add async to the trait. The scheduler runs on its own thread and calls storage synchronously.
- Do NOT add generic type parameters to the trait. `Arc<dyn StorageEngine>` works today, keep it.
- Do NOT touch key encoding (`keys.rs`). It stays unchanged.
- Do NOT add metrics/telemetry to storage operations (that's a separate concern, already handled at scheduler level).

### References

- [Source: crates/fila-core/src/storage/traits.rs] — Current Storage trait (13 methods + write_batch + flush)
- [Source: crates/fila-core/src/storage/rocksdb.rs] — Current RocksDB implementation (251 lines, 17 RocksDB calls)
- [Source: crates/fila-core/src/error.rs] — Current error types
- [Source: _bmad/docs/research/decoupled-scheduler-sharded-storage.md#storage-engine-abstraction] — Rationale for abstraction
- [Source: _bmad-output/planning-artifacts/epics.md#epic-13] — Epic ACs

## Dev Agent Record

### Agent Model Used

Claude Opus 4.6

### Debug Log References

### Completion Notes List

### File List
