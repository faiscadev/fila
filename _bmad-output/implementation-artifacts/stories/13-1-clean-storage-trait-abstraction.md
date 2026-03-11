# Story 13.1: Clean Storage Trait Abstraction

Status: review

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

- [x] Task 1: Rename `Storage` to `StorageEngine` and `RocksDbStorage` to `RocksDbEngine` (AC: 1, 5, 6)
  - [x] 1.1–1.10: All references updated across 22 files
  - [x] 1.9: fila-sdk has no storage type references (only a comment)
  - [x] 1.11: fila-e2e has no storage type references

- [x] Task 2: Rename `WriteBatchOp` to `Mutation` and `write_batch` to `apply_mutations` (AC: 2, 4)
  - [x] 2.1–2.5: All renames complete

- [x] Task 3: Review and clean trait documentation (AC: 2)
  - [x] 3.1–3.3: Trait uses "message store", "lease store", "config store", "state store" terminology

- [x] Task 4: Verify no PartitionId concept exists (AC: 3)
  - [x] 4.1: Confirmed absent — grep returns zero results

- [x] ~~Task 5: Implement `InMemoryEngine`~~ (AC: 7) — **SKIPPED per Lucas's direction**: keep RocksDB for all tests, add mockall only if needed later
- [x] ~~Task 6: Add storage engine tests for InMemoryEngine~~ — **SKIPPED** (follows Task 5)

- [x] Task 7: Rename `StorageError` variants (AC: 2, 10)
  - [x] 7.1–7.4: RocksDb→Engine, ColumnFamilyNotFound→StoreNotFound, all conversions updated

- [x] Task 8: Verify all tests pass (AC: 8, 9)
  - [x] 8.1: `cargo test --workspace` — all 278 tests pass
  - [x] 8.2: `cargo clippy --workspace` — zero new warnings
  - [x] 8.3: e2e test suite — all 11 tests pass

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

- All 278 tests pass (`cargo test --workspace`)
- Zero clippy warnings (`cargo clippy --workspace`)

### Completion Notes List

- Story was primarily a rename operation — existing Storage trait abstraction was already well-designed with RocksDB types fully encapsulated.
- InMemoryEngine was initially implemented then removed per Lucas's direction: RocksDB-backed tests are preferred since they test what runs in production.
- Code review caught 11 stale "CF" (column family) references in scheduler/test comments that needed domain-term updates.

### File List

- `crates/fila-core/src/storage/traits.rs` — Renamed trait + enum + method, updated doc comments to domain terms
- `crates/fila-core/src/storage/rocksdb.rs` — Renamed struct, updated impl block and error variants
- `crates/fila-core/src/storage/mod.rs` — Updated re-exports
- `crates/fila-core/src/error.rs` — Renamed StorageError variants: RocksDb→Engine, ColumnFamilyNotFound→StoreNotFound
- `crates/fila-core/src/lib.rs` — Updated public exports
- `crates/fila-core/src/broker/mod.rs` — Updated type references
- `crates/fila-core/src/broker/scheduler/mod.rs` — Updated imports and type references
- `crates/fila-core/src/broker/scheduler/handlers.rs` — WriteBatchOp→Mutation, write_batch→apply_mutations
- `crates/fila-core/src/broker/scheduler/admin_handlers.rs` — Same renames + doc comment update
- `crates/fila-core/src/broker/scheduler/delivery.rs` — Same renames + doc comment update
- `crates/fila-core/src/broker/scheduler/recovery.rs` — Same renames + doc comment updates
- `crates/fila-core/src/broker/scheduler/tests/mod.rs` — Updated imports
- `crates/fila-core/src/broker/scheduler/tests/common.rs` — Updated type references
- `crates/fila-core/src/broker/scheduler/tests/ack_nack.rs` — Updated type references + comment fixes
- `crates/fila-core/src/broker/scheduler/tests/config.rs` — Updated type references + comment fix
- `crates/fila-core/src/broker/scheduler/tests/fairness.rs` — Updated type references
- `crates/fila-core/src/broker/scheduler/tests/lua.rs` — Updated type references + comment fix
- `crates/fila-core/src/broker/scheduler/tests/recovery.rs` — Updated type references
- `crates/fila-core/src/lua/mod.rs` — Updated type references
- `crates/fila-core/src/lua/bridge.rs` — Updated type references + doc comment + test comment
- `crates/fila-core/src/queue.rs` — Updated doc comment
- `crates/fila-server/src/main.rs` — Updated imports
- `crates/fila-server/src/admin_service.rs` — Updated test imports
