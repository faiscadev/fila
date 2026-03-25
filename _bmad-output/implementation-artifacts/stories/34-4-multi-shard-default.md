# Story 34.4: Multi-Shard Default — Linear Scaling with CPU Cores

Status: pending

## Story

As an operator,
I want the scheduler to default to one shard per CPU core,
So that multi-queue workloads scale linearly with available hardware without manual tuning.

## Acceptance Criteria

1. **Given** `shard_count` currently defaults to 1
   **When** the default is changed to CPU core count
   **Then** the scheduler creates one shard per available CPU core on startup
   **And** queues are distributed across shards via consistent hashing of queue_id (existing behavior)

2. **Given** a multi-queue workload (N queues, M shards where M >= N)
   **When** throughput is measured
   **Then** throughput scales approximately linearly with shard count
   **And** the scaling factor is documented (e.g., 4 shards ≈ 3.6x single-shard)

3. **Given** a single-queue workload
   **When** throughput is measured with multiple shards
   **Then** throughput is approximately equal to single-shard (all messages go to one shard)
   **And** this limitation is documented (per-fairness-key sharding, Story 34.5, addresses this)

4. **Given** the shard_count is still configurable
   **When** an operator sets `shard_count = 1` explicitly
   **Then** the single-shard behavior is preserved (backward compatible)
   **And** the config documentation is updated

5. **Given** multi-shard mode
   **When** combined with Plateau 1+2 optimizations
   **Then** multi-queue throughput is measured end-to-end
   **And** if single-shard throughput is X msg/s, N shards should achieve ~0.9Nx msg/s

## Tasks / Subtasks

- [ ] Task 1: Change shard_count default
  - [ ] 1.1 Default `shard_count` to `num_cpus::get()` (or `available_parallelism()`)
  - [ ] 1.2 Update config documentation
  - [ ] 1.3 Log shard count on startup

- [ ] Task 2: Benchmark multi-shard throughput
  - [ ] 2.1 Multi-queue workload: 4, 8, 16 queues with core-count shards
  - [ ] 2.2 Single-queue workload: verify no regression
  - [ ] 2.3 Document scaling factor

- [ ] Task 3: Verify cluster mode compatibility
  - [ ] 3.1 Multi-shard + Raft clustering: verify queue-to-shard routing works with Raft groups
  - [ ] 3.2 Run cluster e2e tests with multi-shard

## Dev Notes

### Already Implemented

The scheduler already supports sharding (`shard_count` config, hash-based routing by queue_id). This story is primarily a default change + benchmarking, not new implementation.

### Scaling Math

| Shards | Multi-Queue (N queues) | Single-Queue |
|--------|----------------------|--------------|
| 1 (current) | X msg/s | X msg/s |
| 4 | ~4X msg/s | X msg/s (no help) |
| 8 | ~8X msg/s | X msg/s (no help) |

With Plateau 1+2 single-shard at ~100-200K msg/s, 4 shards = 400-800K msg/s for multi-queue workloads.

### Key Files to Modify

| File | Change |
|------|--------|
| `crates/fila-core/src/config.rs` | Change shard_count default |
| `docs/configuration.md` | Update shard_count documentation |

### References

- [Research: technical-kafka-parity-profiling-strategy-2026-03-25.md, Architecture Decision 3] — Scheduler scaling
