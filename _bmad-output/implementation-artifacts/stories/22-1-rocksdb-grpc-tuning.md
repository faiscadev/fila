# Story 22.1: RocksDB Queue-Optimized Configuration & gRPC Tuning

Status: ready-for-dev

## Story

As an operator,
I want Fila to use RocksDB configuration optimized for queue access patterns and tuned gRPC settings,
so that throughput improves 3-10x without any API or behavioral changes.

## Acceptance Criteria

1. **Given** the RocksDB storage engine currently uses entirely default configuration (`Options::default()` in `rocksdb.rs:44-56`)
   **When** queue-optimized settings are applied
   **Then** a shared LRU block cache of 256MB is configured with `cache_index_and_filter_blocks = true` and `pin_l0_filter_and_index_blocks_in_cache = true`

2. **And** `enable_pipelined_write = true` is set (WAL and memtable writes run in parallel)
   **And** `manual_wal_flush = true` with `wal_bytes_per_sync = 512KB` is set (buffered WAL — safe when Raft provides durability)

3. **And** the `messages` column family has: `write_buffer_size = 128MB`, `max_write_buffer_number = 4`, `min_write_buffer_number_to_merge = 2`
   **And** the `messages` column family has 10-bit bloom filters enabled with `memtable_prefix_bloom_size_ratio = 0.1`
   **And** the `messages` column family uses no compression on L0-L1 and LZ4 on L2+

4. **And** `CompactOnDeletionCollector` is enabled on the `messages` and `raft_log` column families (critical for queue's delete-heavy pattern — recommended by RocksDB wiki "Implement Queue Service Using RocksDB")

5. **And** the `leases` column family uses similar settings with 64MB write buffer (smaller scale)
   **And** the `lease_expiry` column family has bloom filters disabled (range scans don't use them) and no compression

6. **And** `iterate_upper_bound` is set on all prefix scans to prevent iterators from walking past tombstones

7. **And** all RocksDB tuning settings are configurable via `fila.toml` under a `[storage.rocksdb]` section with the queue-optimized values as defaults

8. **And** the gRPC server is configured with: `initial_stream_window_size = 2MB`, `initial_connection_window_size = 4MB`, `tcp_nodelay = true`, `http2_keepalive_interval = 15s`, `http2_keepalive_timeout = 10s`

9. **And** the full benchmark suite (Epic 21) runs before and after the changes, with results compared in the PR description

10. **And** all existing tests pass (432+ tests)
    **And** all e2e tests pass

11. **And** memory RSS is measured and documented (expected increase from ~268MB to ~400-512MB due to larger block cache and write buffers)

## Tasks / Subtasks

- [ ] Task 1: Run baseline benchmarks (AC: #9)
  - [ ] Run full fila-bench suite on current main
  - [ ] Save baseline results JSON for comparison
- [ ] Task 2: Add RocksDB configuration structs to BrokerConfig (AC: #7)
  - [ ] Create `StorageRocksDbConfig` struct with all tuning params and queue-optimized defaults
  - [ ] Add `[storage.rocksdb]` section to config deserialization
  - [ ] Update `deploy/fila.toml` example
- [ ] Task 3: Apply RocksDB tuning in storage engine (AC: #1, #2, #3, #4, #5)
  - [ ] Create shared LRU block cache (256MB)
  - [ ] Configure DB-level options: pipelined write, WAL buffering
  - [ ] Configure `messages` CF: write buffers, bloom filters, tiered compression
  - [ ] Configure `leases` CF: 64MB write buffer, bloom filters
  - [ ] Configure `lease_expiry` CF: no bloom filters, no compression
  - [ ] Configure `raft_log` CF: CompactOnDeletionCollector
  - [ ] Enable CompactOnDeletionCollector on `messages` and `raft_log` CFs
- [ ] Task 4: Add iterate_upper_bound to prefix scans (AC: #6)
  - [ ] Audit all prefix scan operations in RocksDbEngine
  - [ ] Set iterate_upper_bound using prefix increment for each scan
- [ ] Task 5: Apply gRPC server tuning (AC: #8)
  - [ ] Configure tonic Server::builder with window sizes, keepalive, tcp_nodelay
  - [ ] Add gRPC tuning to config struct (optional, with defaults)
- [ ] Task 6: Run post-optimization benchmarks and compare (AC: #9, #11)
  - [ ] Run full fila-bench suite on optimized branch
  - [ ] Compare before/after in PR description
  - [ ] Measure and document RSS delta
- [ ] Task 7: Verify all tests pass (AC: #10)
  - [ ] Run full workspace tests
  - [ ] Run e2e tests

## Dev Notes

### Current State

RocksDB is opened with `Options::default()` for all column families (`crates/fila-core/src/storage/rocksdb.rs:44-56`). This means:
- 8MB block cache (tiny for a message broker)
- No bloom filters
- Default compression
- No pipelined writes
- No WAL buffering
- No compaction deletion collector (critical for queue pattern where every message is eventually deleted)

The gRPC server (`crates/fila-server/src/main.rs:174-188`) uses tonic defaults: 64KB initial window, no keepalive, tcp_nodelay not set.

### Key Files to Modify

1. `crates/fila-core/src/storage/rocksdb.rs` — Main target. Restructure `RocksDbEngine::open()` to accept config and apply per-CF tuning.
2. `crates/fila-core/src/broker/config.rs` — Add `StorageRocksDbConfig` and `GrpcConfig` structs. Current config has `ServerConfig`, `SchedulerConfig`, `LuaConfig`, `TelemetryConfig`, `ClusterConfig`, `TlsParams`, `AuthConfig`, `GuiConfig`.
3. `crates/fila-server/src/main.rs` — Apply gRPC tuning to `Server::builder()` chain.
4. `deploy/fila.toml` — Add example `[storage.rocksdb]` section.

### Column Families

Current CFs defined at `rocksdb.rs:14-31`:
- `messages` — highest write/delete volume, queue payload storage
- `leases` — active consumer leases, moderate volume
- `lease_expiry` — time-indexed expiry keys, range scans dominant
- `queues` — queue metadata, low volume
- `state` — persistent broker state, low volume
- `raft_log` — Raft consensus entries, high append/delete (cluster mode)
- `msg_index` — message ID → full key map, write/delete mirrors messages

### RocksDB Tuning Strategy (from research)

The RocksDB wiki "Implement Queue Service Using RocksDB" recommends:
- **CompactOnDeletionCollector**: Critical for queue patterns where every message is eventually deleted. Triggers compaction when tombstone ratio exceeds threshold, preventing scan degradation.
- **Bloom filters**: 10-bit filters reduce false-positive rate for point lookups. Enable on CFs with point reads (messages, leases, msg_index). Disable on CFs with range scans only (lease_expiry).
- **Large write buffers**: Reduce flush frequency under sustained write load. 128MB for messages, 64MB for leases.
- **Pipelined writes**: WAL and memtable writes in parallel — ~30% write throughput improvement.
- **Tiered compression**: No compression on L0-L1 (speed), LZ4 on L2+ (space). Queue messages are short-lived so most never reach L2.

### gRPC Tuning Strategy

- **Initial window sizes**: 2MB stream + 4MB connection (vs 64KB defaults) reduces flow-control stalls under sustained produce/consume streams.
- **tcp_nodelay**: Eliminates Nagle's algorithm delay on small writes (important for low-latency ack/nack paths).
- **Keepalive**: 15s interval + 10s timeout prevents idle connection detection issues with load balancers.

### iterate_upper_bound

All prefix scans in `RocksDbEngine` (e.g., listing messages for a queue, listing leases) should set `iterate_upper_bound` to `prefix + 1` (increment the last byte of the prefix). This prevents the iterator from walking past tombstones into unrelated key ranges — a significant performance issue with RocksDB when the database has many deleted keys (the queue pattern).

### Testing Strategy

- All 432+ existing tests validate behavior is unchanged (config changes should not affect correctness)
- E2e tests validate end-to-end functionality under the new configuration
- Benchmark comparison provides performance validation

### Project Structure Notes

- Configuration nested under `[storage.rocksdb]` in TOML matches the nested struct pattern used by `[scheduler]`, `[lua]`, `[cluster]`, etc.
- The `BrokerConfig` struct at `config.rs` already uses `#[serde(default)]` for all optional sections — follow the same pattern for `StorageConfig`.

### References

- [Source: crates/fila-core/src/storage/rocksdb.rs] — Current RocksDB engine
- [Source: crates/fila-core/src/broker/config.rs] — BrokerConfig
- [Source: crates/fila-server/src/main.rs] — gRPC server setup
- [Source: _bmad-output/planning-artifacts/performance-optimization-epics.md#Story 22.1] — Epic plan ACs
- [Ref: RocksDB wiki "Implement Queue Service Using RocksDB"]

## Dev Agent Record

### Agent Model Used

Claude Opus 4.6

### Debug Log References

### Completion Notes List

### File List
