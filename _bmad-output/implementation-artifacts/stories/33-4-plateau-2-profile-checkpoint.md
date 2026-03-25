# Story 33.4: Plateau 2 Profile Checkpoint — Results & Go/No-Go for Plateau 3

Status: pending

## Story

As a developer,
I want full profiling after all Plateau 2 optimizations,
So that the system's throughput ceiling with RocksDB is known and a data-driven decision is made about Plateau 3 (storage engine replacement + scale out).

## Acceptance Criteria

1. **Given** stories 33.1-33.3 are complete
   **When** enqueue and consume throughput are measured
   **Then** both are compared against the Plateau 1 checkpoint (32.6)
   **And** the consume-to-enqueue throughput ratio is documented (expected: near parity)

2. **Given** multi-shard scheduling is already implemented (config change only)
   **When** `shard_count` is set to CPU core count
   **Then** multi-queue throughput is measured with multiple shards
   **And** linear scaling is verified (N shards ≈ Nx single-shard throughput for multi-queue workloads)
   **And** single-queue throughput is measured (expected: no improvement from sharding alone)

3. **Given** the competitive benchmark suite
   **When** Fila is benchmarked against Kafka, RabbitMQ, and NATS
   **Then** the competitive ratio is updated
   **And** Fila's position relative to NATS (~138K msg/s) and RabbitMQ (~43K msg/s) is documented

4. **Given** CPU flamegraphs and tracing spans
   **When** the new bottleneck is identified
   **Then** the document states whether RocksDB write amplification is now the dominant cost
   **And** the remaining per-message overhead breakdown is quantified

5. **Given** all profiling data
   **When** a go/no-go decision is made for Epic 34 (Plateau 3)
   **Then** the decision is documented with supporting data:
   - **Go**: RocksDB is the bottleneck, Titan or custom storage would unlock further gains
   - **No-go, sufficient**: throughput meets project goals, Plateau 3 is unnecessary
   - **No-go, different bottleneck**: something else limits throughput, address that first

6. **Given** multi-shard results
   **When** the decision about Plateau 3 scope is made
   **Then** if multi-shard + Plateau 2 already achieves target throughput, Plateau 3 may be deferred
   **And** the document quantifies: "N shards × Plateau 2 single-shard throughput = X msg/s, target is Y msg/s"

## Tasks / Subtasks

- [ ] Task 1: Single-shard benchmarks
  - [ ] 1.1 Enqueue throughput (1KB, single producer + multi-producer)
  - [ ] 1.2 Consume throughput (1, 10, 100 consumers)
  - [ ] 1.3 End-to-end lifecycle (enqueue → consume → ack)

- [ ] Task 2: Multi-shard benchmarks
  - [ ] 2.1 Set shard_count to CPU core count
  - [ ] 2.2 Multi-queue workload throughput
  - [ ] 2.3 Single-queue workload (verify no improvement)

- [ ] Task 3: CPU flamegraphs + tracing
  - [ ] 3.1 Generate flamegraphs (enqueue + consume)
  - [ ] 3.2 Identify new #1 CPU consumer

- [ ] Task 4: Competitive benchmarks
  - [ ] 4.1 Run `make bench-competitive`
  - [ ] 4.2 Update Fila ÷ Kafka ratio

- [ ] Task 5: Write analysis and go/no-go document
  - [ ] 5.1 Write to `_bmad-output/planning-artifacts/research/plateau-2-results-analysis.md`
  - [ ] 5.2 Go/no-go recommendation for Epic 34
  - [ ] 5.3 If go: recommend starting with Story 34.1 (Titan) or 34.2 (custom log)

## Dev Notes

### Decision Matrix

| Result | Recommendation |
|--------|---------------|
| Single-shard >100K, multi-shard >200K | Plateau 3 optional. Multi-shard may be sufficient. |
| Single-shard 50-100K, RocksDB is bottleneck | Go for Plateau 3, start with Titan (34.1) — lower risk |
| Single-shard 50-100K, not RocksDB | Investigate unexpected bottleneck before Plateau 3 |
| Single-shard <50K | Something went wrong in Plateau 1-2. Re-profile and diagnose. |

### Key Metrics

| Metric | Plateau 1 | Post-Plateau-2 |
|--------|-----------|----------------|
| Enqueue throughput (1 producer) | ? (from 32.6) | ? |
| Consume throughput (100 consumers) | ? (from 32.6) | ? |
| End-to-end lifecycle | not measured | ? |
| Multi-shard throughput | not tested | ? |
| Competitive ratio (Fila ÷ Kafka) | ? (from 32.6) | ? |

### References

- [Research: technical-kafka-parity-profiling-strategy-2026-03-25.md, "Profile Between Plateaus"] — Design principle
- [Story: 32-6-plateau-1-profile-checkpoint.md] — Plateau 1 results (dependency)
