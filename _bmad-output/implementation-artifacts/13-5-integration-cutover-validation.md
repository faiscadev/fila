# Story 13.5: Integration, Cutover & Validation

Status: ready-for-dev

## Story

As an operator,
I want to select between the new Fila storage engine and RocksDB via configuration,
so that I can adopt the new engine with confidence while retaining a fallback.

## Critical Context

**Stories 13.1–13.4 delivered** the complete Fila storage engine: Storage trait abstraction, WAL write path, in-memory indexes for reads, and background compaction. All `Storage` trait methods are fully functional.

**This story integrates** the new engine into the server, adds configuration-based engine selection, runs the full test suite with both backends, and validates performance targets.

**What is NOT in scope:**
- Data migration tools (engine selection is per-deployment, no migration needed)
- Removing RocksDB support (kept for one release cycle as fallback)

## Acceptance Criteria

1. **Given** server configuration with `[storage] engine = "fila"` (or default)
   **When** the server starts
   **Then** `FilaStorage` is instantiated and used as the storage backend
   **And** compaction is enabled by default

2. **Given** server configuration with `[storage] engine = "rocksdb"`
   **When** the server starts
   **Then** `RocksDbStorage` is instantiated (legacy fallback)

3. **Given** the full scheduler test suite (unit + integration tests)
   **When** tests run with `FilaStorage` as the backend
   **Then** all tests pass identically to RocksDB

4. **Given** the e2e blackbox test suite
   **When** tests run with the server using `FilaStorage`
   **Then** all 11 e2e tests pass

5. **Given** benchmark comparison between FilaStorage and RocksDB
   **When** benchmarks run on the same hardware
   **Then** FilaStorage meets NFR targets:
   - NFR30: >= 2x write throughput vs RocksDB
   - NFR31: no p99 latency spikes > 10ms during compaction
   - NFR33: storage footprint < 1.5x raw data after compaction

6. **Given** operator documentation
   **When** an operator reads the configuration guide
   **Then** storage engine selection is documented with examples

## Tasks / Subtasks

- [ ] Task 1: Add storage engine configuration (AC: #1, #2)
  - [ ] Add `StorageConfig` struct with `engine` field (enum: `fila`, `rocksdb`)
  - [ ] Add `[storage]` section to `BrokerConfig` TOML parsing
  - [ ] Wire Fila-specific config fields: `data_dir`, `segment_size_bytes`, `sync_mode`, compaction settings
  - [ ] Default: engine = fila, compaction_enabled = true

- [ ] Task 2: Update server startup to use storage config (AC: #1, #2)
  - [ ] In `main.rs`, instantiate storage based on `config.storage.engine`
  - [ ] Pass `Arc<dyn Storage>` to Broker (already generic)
  - [ ] Add environment variable override: `FILA_STORAGE_ENGINE=fila|rocksdb`

- [ ] Task 3: Run scheduler tests with FilaStorage (AC: #3)
  - [ ] Update scheduler test common.rs to support configurable storage backend
  - [ ] Run full test suite with FilaStorage, verify all pass
  - [ ] Keep RocksDB as default for existing tests (backward compatible)

- [ ] Task 4: Run e2e tests with FilaStorage (AC: #4)
  - [ ] Update TestServer helper to pass storage engine via env var
  - [ ] Run all 11 e2e tests with FilaStorage server
  - [ ] Verify all pass

- [ ] Task 5: Benchmark validation (AC: #5)
  - [ ] Update BenchServer to support storage engine selection
  - [ ] Run self-benchmarks with both engines
  - [ ] Verify NFR targets are met

- [ ] Task 6: Documentation (AC: #6)
  - [ ] Update docs/configuration.md with `[storage]` section examples
  - [ ] Document engine selection, defaults, and fallback path

## Dev Notes

### Configuration Structure

```toml
[storage]
engine = "fila"       # "fila" (default) or "rocksdb"
data_dir = "data"     # shared data directory

# Fila-specific settings (only used when engine = "fila")
segment_size_bytes = 67108864    # 64 MB
compaction_enabled = true
compaction_interval_secs = 60
message_ttl_ms = 0               # 0 = no TTL
```

### Server Startup Changes

```rust
// In main.rs
let storage: Arc<dyn Storage> = match config.storage.engine {
    StorageEngine::Fila => {
        let fila_config = FilaStorageConfig { ... };
        Arc::new(FilaStorage::open(&fila_config)?)
    }
    StorageEngine::RocksDb => {
        Arc::new(RocksDbStorage::open(&config.storage.data_dir)?)
    }
};
```

### Test Parametrization Strategy

For scheduler tests in `tests/common.rs`, the simplest approach is to add a helper that creates a FilaStorage instance using the same temp directory pattern:

```rust
pub fn fila_storage(dir: &Path) -> Arc<dyn Storage> {
    Arc::new(FilaStorage::open(&FilaStorageConfig::new(dir.to_path_buf())).unwrap())
}
```

Then add a second test run configuration or duplicate key tests with the new backend.

For e2e tests, the TestServer helper can set `FILA_STORAGE_ENGINE=fila` in the child process environment.

### Benchmark Comparison

The benchmark harness (`BenchServer`) writes a `fila.toml` config before starting the server. Add the `[storage]` section to the generated config. Run the same benchmarks twice (once with each engine) and compare results.

### Key Constraints

- `BrokerConfig` uses `serde::Deserialize` with TOML — new fields need serde attributes
- `FilaStorageConfig` needs `Clone` (already has it)
- The server's `FILA_DATA_DIR` env var should be consolidated into the storage config
- RocksDB stays as a fallback; it's still built and linked

### References

- [Source: crates/fila-server/src/main.rs] — server startup, currently hardcoded RocksDB
- [Source: crates/fila-core/src/broker/config.rs] — BrokerConfig TOML deserialization
- [Source: crates/fila-core/src/storage/fila/mod.rs] — FilaStorage implementation
- [Source: crates/fila-core/src/storage/fila/config.rs] — FilaStorageConfig
- [Source: crates/fila-core/src/storage/rocksdb.rs] — RocksDbStorage
- [Source: crates/fila-core/src/broker/scheduler/tests/common.rs] — test helper with RocksDB
- [Source: crates/fila-e2e/tests/helpers/mod.rs] — TestServer helper
- [Source: crates/fila-bench/src/server.rs] — BenchServer helper
- [Source: docs/configuration.md] — existing config documentation
- [Source: _bmad-output/planning-artifacts/epics.md#Epic 13] — epic plan

## Dev Agent Record

### Agent Model Used
Claude Opus 4.6

### Debug Log References

### Completion Notes List

### File List
