# Story 34.5: Hierarchical DRR — Per-Fairness-Key Sharding Within a Queue

Status: pending

## Story

As a developer,
I want per-fairness-key sharding within a single queue so that single-queue workloads scale across multiple CPU cores,
So that Fila can achieve Kafka-parity throughput even for single-queue deployments.

## Acceptance Criteria

1. **Given** a single queue with many fairness keys
   **When** per-fairness-key sharding is enabled
   **Then** fairness keys within the queue are distributed across shards via consistent hashing
   **And** each shard runs independent DRR for its assigned fairness keys

2. **Given** per-fairness-key sharding
   **When** global fairness across shards is needed
   **Then** Hierarchical DRR (H-DRR) is implemented: first stage = per-shard DRR, second stage = cross-shard fairness balancing
   **And** global fairness is maintained within one quantum per round (bounded unfairness)

3. **Given** a single-queue workload with K fairness keys and N shards
   **When** throughput is measured
   **Then** throughput scales approximately linearly with min(K, N) shards
   **And** the scaling factor is documented

4. **Given** H-DRR cross-shard coordination
   **When** latency is measured
   **Then** the coordination overhead is quantified
   **And** per-message latency increase from cross-shard balancing is acceptable (< 10% of single-shard latency)

5. **Given** existing multi-queue sharding (Story 34.4)
   **When** per-fairness-key sharding is combined
   **Then** both modes coexist: multi-queue shards by queue_id, single-queue shards by fairness_key within its queue
   **And** the scheduling hierarchy is: queue → shard → fairness_key → DRR

## Tasks / Subtasks

- [ ] Task 1: Implement per-fairness-key routing
  - [ ] 1.1 Route `(queue_id, fairness_key)` → shard (consistent hash)
  - [ ] 1.2 Each shard owns a subset of fairness keys for each queue
  - [ ] 1.3 Enqueue routes to correct shard based on fairness_key

- [ ] Task 2: Implement Hierarchical DRR
  - [ ] 2.1 Per-shard DRR (existing, scoped to assigned fairness keys)
  - [ ] 2.2 Cross-shard coordinator: global fairness balancing
  - [ ] 2.3 Bounded unfairness guarantee (one quantum per round)

- [ ] Task 3: Handle consumer delivery across shards
  - [ ] 3.1 Consumer connections routed to appropriate shard
  - [ ] 3.2 Or: consumer receives from a cross-shard delivery aggregator

- [ ] Task 4: Benchmark single-queue scaling
  - [ ] 4.1 Throughput with 10, 100, 1000 fairness keys across N shards
  - [ ] 4.2 Verify fairness accuracy under sharding
  - [ ] 4.3 Measure coordination overhead

## Dev Notes

### Conditional Story

This story is only needed if single-queue throughput is insufficient after stories 34.1-34.4. Multi-queue workloads already scale via per-queue sharding (34.4). This story is the final lever for single-queue deployments.

### H-DRR (Hierarchical Deficit Round Robin)

Proven O(1) per scheduling decision. IEEE paper demonstrates bounded fairness across hierarchical schedulers. The two-level design (per-shard DRR + cross-shard balancing) is the standard approach for distributed fair scheduling.

### Key Files to Modify

| File | Change |
|------|--------|
| `crates/fila-core/src/broker/scheduler/mod.rs` | Per-fairness-key shard routing |
| `crates/fila-core/src/broker/drr.rs` | Scoped DRR (already prepared in Epic 13 seams) |
| `crates/fila-core/src/broker/scheduler/delivery.rs` | Cross-shard delivery aggregation |

### References

- [Source: H-DRR (IEEE)](https://ieeexplore.ieee.org/document/4410617/) — Hierarchical DRR paper
- [Source: ScyllaDB Shard-Per-Core](https://www.scylladb.com/2024/10/21/why-scylladbs-shard-per-core-architecture-matters/) — Shard-per-core patterns
- [Research: technical-kafka-parity-profiling-strategy-2026-03-25.md, Architecture Decision 3] — H-DRR analysis
