# Story 1.3: Core Domain Types & Storage Layer

Status: ready-for-dev

## Story

As a developer,
I want message domain types and a persistent storage layer with RocksDB,
So that messages can be durably stored and retrieved with correct ordering guarantees.

## Acceptance Criteria

1. A `Storage` trait is defined with methods for message CRUD, lease management, queue config, and state operations
2. A `RocksDbStorage` implementation opens RocksDB with column families: `default`, `messages`, `leases`, `lease_expiry`, `queues`, `state`
3. Message keys in the `messages` CF use the format `{queue_id}:{fairness_key}:{enqueue_ts_ns}:{msg_id}` with big-endian numeric encoding
4. Lease keys use `{queue_id}:{msg_id}` format in the `leases` CF
5. Lease expiry keys use `{expiry_ts_ns}:{queue_id}:{msg_id}` format in the `lease_expiry` CF for efficient timeout scanning
6. Message IDs are UUIDv7 (time-ordered, globally unique)
7. All multi-key mutations use RocksDB `WriteBatch` for atomicity
8. A `FilaError` enum using `thiserror` is defined with variants: `QueueNotFound`, `MessageNotFound`, `LuaError`, `StorageError`, `InvalidConfig`, `QueueAlreadyExists`
9. Unit tests verify key encoding produces correct lexicographic ordering
10. Unit tests verify WriteBatch atomicity (all-or-nothing writes)

## Tasks / Subtasks

- [ ] Task 1: Define domain types in fila-core (AC: #8)
  - [ ] 1.1 Create `crates/fila-core/src/error.rs` with `FilaError` enum using `thiserror`
  - [ ] 1.2 Create `crates/fila-core/src/message.rs` with core `Message` struct (id, headers, payload, metadata, timestamps)
  - [ ] 1.3 Create `crates/fila-core/src/queue.rs` with `QueueConfig` struct (name, on_enqueue_script, on_failure_script, visibility_timeout_ms, dlq_queue_id)
  - [ ] 1.4 Update `crates/fila-core/src/lib.rs` to declare and re-export all modules
  - [ ] 1.5 Add `thiserror`, `uuid`, `bytes`, `serde` to fila-core dependencies
- [ ] Task 2: Create key encoding module (AC: #3, #4, #5, #6)
  - [ ] 2.1 Create `crates/fila-core/src/storage/keys.rs` with key encoding/decoding functions
  - [ ] 2.2 Implement `message_key(queue_id, fairness_key, enqueue_ts_ns, msg_id) -> Vec<u8>` using big-endian encoding for numeric fields and `:` separators
  - [ ] 2.3 Implement `lease_key(queue_id, msg_id) -> Vec<u8>`
  - [ ] 2.4 Implement `lease_expiry_key(expiry_ts_ns, queue_id, msg_id) -> Vec<u8>`
  - [ ] 2.5 Implement `message_prefix(queue_id) -> Vec<u8>` and `message_prefix_with_key(queue_id, fairness_key) -> Vec<u8>` for prefix iteration
  - [ ] 2.6 Implement UUIDv7 generation helper using the `uuid` crate
- [ ] Task 3: Define Storage trait (AC: #1)
  - [ ] 3.1 Create `crates/fila-core/src/storage/mod.rs` and `crates/fila-core/src/storage/traits.rs`
  - [ ] 3.2 Define `Storage` trait with message operations: `put_message`, `get_message`, `delete_message`, `list_messages_by_queue`, `list_messages_by_key`
  - [ ] 3.3 Define lease operations: `put_lease`, `delete_lease`, `get_lease`, `list_expired_leases`
  - [ ] 3.4 Define queue operations: `put_queue`, `get_queue`, `delete_queue`, `list_queues`
  - [ ] 3.5 Define state operations: `put_state`, `get_state`, `delete_state`
  - [ ] 3.6 Define batch operation: `write_batch(WriteBatchOp)` that atomically applies multiple operations
- [ ] Task 4: Implement RocksDB storage (AC: #2, #7)
  - [ ] 4.1 Create `crates/fila-core/src/storage/rocksdb.rs` with `RocksDbStorage` struct
  - [ ] 4.2 Implement `RocksDbStorage::open(path)` that creates/opens DB with all 6 column families
  - [ ] 4.3 Implement all `Storage` trait methods using column-family-specific operations
  - [ ] 4.4 Implement `write_batch` using RocksDB `WriteBatch` for atomic multi-CF writes
  - [ ] 4.5 Add `rocksdb` to fila-core dependencies
- [ ] Task 5: Write unit tests (AC: #9, #10)
  - [ ] 5.1 Test key encoding: verify message keys sort correctly by queue, then fairness_key, then timestamp, then msg_id
  - [ ] 5.2 Test key encoding: verify lease_expiry keys sort by expiry timestamp (earliest first)
  - [ ] 5.3 Test key encoding: verify big-endian encoding of u64 values produces correct lexicographic order
  - [ ] 5.4 Test WriteBatch: verify atomic write of message + lease + lease_expiry succeeds together
  - [ ] 5.5 Test WriteBatch: verify partial failure doesn't leave orphaned entries
  - [ ] 5.6 Test basic CRUD: put/get/delete message, lease, queue, state
  - [ ] 5.7 Test prefix iteration: list messages by queue, list messages by fairness key
  - [ ] 5.8 Test expired lease scanning: list_expired_leases returns only leases past given timestamp
- [ ] Task 6: Verify build (AC: all)
  - [ ] 6.1 Run `cargo build` — all crates compile
  - [ ] 6.2 Run `cargo clippy -- -D warnings` — no warnings
  - [ ] 6.3 Run `cargo fmt --check` — formatted
  - [ ] 6.4 Run `cargo nextest run` — all tests pass

## Dev Notes

### RocksDB Column Family Design

From architecture doc: 6 column families with specific key encoding schemes.

| Column Family | Key Format | Value | Purpose |
|---------------|-----------|-------|---------|
| `default` | — | — | Required by RocksDB; unused |
| `messages` | `{queue_id}:{fairness_key}:{enqueue_ts_ns}:{msg_id}` | Serialized message | Message storage; prefix iteration by queue → key → time |
| `leases` | `{queue_id}:{msg_id}` | `{consumer_id}:{expiry_ts_ns}` | Active lease tracking |
| `lease_expiry` | `{expiry_ts_ns}:{queue_id}:{msg_id}` | `""` (empty) | Secondary index for timeout scanning — iterate from earliest |
| `queues` | `{queue_id}` | Serialized QueueConfig | Queue definitions |
| `state` | `{key}` | Arbitrary bytes | Runtime key-value config |

### Key Encoding Rules

- All numeric values (timestamps, queue IDs) **must** be big-endian `u64` bytes for correct lexicographic ordering in RocksDB
- Composite keys use `:` byte separator (0x3A)
- Message IDs are UUIDv7 (16 bytes) — already lexicographically sortable
- Fairness keys are user-defined strings (UTF-8, variable length) — use length-prefix encoding to avoid ambiguity: `{len_u16_be}{bytes}`
- Queue IDs are strings — also length-prefixed in composite keys

### Domain Types

**Message struct** (NOT the protobuf `Message` — this is the core domain type):
```rust
pub struct Message {
    pub id: uuid::Uuid,           // UUIDv7
    pub queue_id: String,
    pub headers: HashMap<String, String>,
    pub payload: bytes::Bytes,
    pub fairness_key: String,
    pub weight: u32,
    pub throttle_keys: Vec<String>,
    pub attempt_count: u32,
    pub enqueued_at: u64,         // nanos since epoch
    pub leased_at: Option<u64>,   // nanos since epoch, if leased
}
```

**QueueConfig struct:**
```rust
pub struct QueueConfig {
    pub name: String,
    pub on_enqueue_script: Option<String>,
    pub on_failure_script: Option<String>,
    pub visibility_timeout_ms: u64,
    pub dlq_queue_id: Option<String>,
}
```

**FilaError enum:**
```rust
#[derive(Debug, thiserror::Error)]
pub enum FilaError {
    #[error("queue not found: {0}")]
    QueueNotFound(String),
    #[error("message not found: {0}")]
    MessageNotFound(String),
    #[error("lua script error: {0}")]
    LuaError(String),
    #[error("storage error: {0}")]
    StorageError(String),
    #[error("invalid configuration: {0}")]
    InvalidConfig(String),
    #[error("queue already exists: {0}")]
    QueueAlreadyExists(String),
}
```

### Storage Trait Pattern

The `Storage` trait should be `Send + Sync` so it can be shared across threads. All methods take `&self` since RocksDB handles internal concurrency.

**WriteBatchOp enum** for atomic batch operations:
```rust
pub enum WriteBatchOp {
    PutMessage { key: Vec<u8>, value: Vec<u8> },
    DeleteMessage { key: Vec<u8> },
    PutLease { key: Vec<u8>, value: Vec<u8> },
    DeleteLease { key: Vec<u8> },
    PutLeaseExpiry { key: Vec<u8> },
    DeleteLeaseExpiry { key: Vec<u8> },
    PutQueue { key: Vec<u8>, value: Vec<u8> },
    DeleteQueue { key: Vec<u8> },
    PutState { key: Vec<u8>, value: Vec<u8> },
    DeleteState { key: Vec<u8> },
}
```

### RocksDB API Notes (v0.24)

- `DB::open_cf_descriptors(opts, path, cfs)` to open with column families
- `ColumnFamilyDescriptor::new(name, cf_opts)` — name accepts `impl Into<String>`
- `db.cf_handle("name")` returns `Option<&ColumnFamily>`
- `db.put_cf(cf, key, value)` for single puts
- `WriteBatch::new()` + `batch.put_cf(cf, key, value)` + `db.write(batch)` for atomic writes
- `db.iterator_cf(cf, IteratorMode::From(prefix, Direction::Forward))` for prefix scans
- `db.prefix_iterator_cf(cf, prefix)` for prefix iteration (requires prefix extractor)

### Serialization

Use `serde` + `serde_json` for serializing domain types to/from RocksDB values. This is simple and debuggable. Performance-critical path can switch to bincode later if needed.

### Workspace Dependencies to Add

fila-core needs these new workspace dependencies:
- `thiserror` (already in workspace) — error types
- `uuid` (already in workspace, features: `v7`) — message IDs
- `bytes` (already in workspace) — payload handling
- `serde` (already in workspace, features: `derive`) — serialization
- `serde_json` — serialization format for RocksDB values
- `rocksdb` (already in workspace) — storage engine

Add to `crates/fila-core/Cargo.toml`:
```toml
[dependencies]
fila-proto = { workspace = true }
thiserror = { workspace = true }
uuid = { workspace = true }
bytes = { workspace = true }
serde = { workspace = true }
serde_json = "1"
rocksdb = { workspace = true }
```

Add `serde_json` to workspace deps too:
```toml
serde_json = "1"
```

Also add `tempfile = "3"` as a dev-dependency for tests that need temporary directories.

### Previous Story Intelligence

**Story 1.1 learnings:**
- tonic 0.14 uses `tonic-prost-build` (not old `tonic-build` + `prost-build`)
- `tonic-prost` and `prost-types` are runtime dependencies
- build.rs uses `CARGO_MANIFEST_DIR` + PathBuf for robust path resolution
- fila-proto re-exports all generated types via `tonic::include_proto!("fila.v1")`

**Story 1.2 learnings:**
- CI runs on all PRs (not just those targeting main)
- GitHub Actions pinned to commit SHAs for security
- Placeholder test in fila-core ensures nextest doesn't fail on zero tests

**Current file structure relevant to this story:**
```
crates/fila-core/
├── Cargo.toml          # Currently only depends on fila-proto
└── src/
    └── lib.rs          # Has placeholder test, needs module declarations
```

### Anti-Patterns to Avoid

- Do NOT use `anyhow` — project uses `thiserror` for typed errors
- Do NOT use `unwrap()` in production code — use proper error handling
- Do NOT use string formatting for numeric key encoding — must use big-endian bytes
- Do NOT call RocksDB directly from outside the storage module — always go through `Storage` trait
- Do NOT create separate `mod.rs` files for simple modules — only use directories for modules with sub-modules (storage has sub-modules)
- Do NOT log message payloads or headers that might contain PII
- Do NOT use `Arc<Mutex<...>>` for storage — RocksDB is internally thread-safe

### Project Structure After This Story

```
crates/fila-core/
├── Cargo.toml
└── src/
    ├── lib.rs          # Module declarations + re-exports
    ├── error.rs        # FilaError enum
    ├── message.rs      # Message domain type
    ├── queue.rs        # QueueConfig type
    └── storage/
        ├── mod.rs      # Re-export Storage trait + RocksDbStorage
        ├── traits.rs   # Storage trait definition + WriteBatchOp
        ├── keys.rs     # Key encoding/decoding functions
        └── rocksdb.rs  # RocksDbStorage implementation
```

### References

- [Source: _bmad-output/planning-artifacts/architecture.md#Data Architecture — RocksDB]
- [Source: _bmad-output/planning-artifacts/architecture.md#Error Handling Strategy]
- [Source: _bmad-output/planning-artifacts/architecture.md#Project Structure & Boundaries]
- [Source: _bmad-output/planning-artifacts/architecture.md#Implementation Patterns & Consistency Rules]
- [Source: _bmad-output/planning-artifacts/epics.md#Story 1.3: Core Domain Types & Storage Layer]
- [Source: _bmad-output/planning-artifacts/prd.md#Functional Requirements]

## Dev Agent Record

### Agent Model Used

{{agent_model_name_version}}

### Debug Log References

### Completion Notes List

### File List
