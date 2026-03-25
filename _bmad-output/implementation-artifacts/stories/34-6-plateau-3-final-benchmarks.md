# Story 34.6: Plateau 3 Final Benchmarks — Kafka Parity Assessment

Status: pending

## Story

As a developer,
I want comprehensive final benchmarks comparing Fila against Kafka after all three plateaus of optimization,
So that the Kafka parity goal is assessed with data and the performance roadmap is concluded.

## Acceptance Criteria

1. **Given** all Plateau 3 stories (34.1-34.5, as applicable) are complete
   **When** full benchmark suite is run
   **Then** enqueue throughput, consume throughput, end-to-end lifecycle, and multi-shard scaling are all measured

2. **Given** the competitive benchmark suite
   **When** Fila is benchmarked against Kafka under identical conditions
   **Then** the competitive ratio (Fila ÷ Kafka) is calculated for all workload profiles:
   - Single-producer 1KB throughput
   - Multi-producer throughput
   - Single-consumer latency
   - Fan-out (1 producer / N consumers)
   - Varying message sizes (64B, 1KB, 64KB)

3. **Given** Fila's unique features (fairness scheduling, Lua hooks, per-message ack)
   **When** throughput is measured with and without these features
   **Then** the per-feature overhead is quantified
   **And** the "price of fairness" (throughput with DRR vs raw FIFO) is documented at the final throughput level

4. **Given** all profiling data
   **When** a final analysis document is written
   **Then** it declares one of:
   - **Kafka parity achieved** (Fila ÷ Kafka >= 0.8x for primary workloads)
   - **Near-parity** (0.5-0.8x) with specific bottleneck identified
   - **Gap remains** (<0.5x) with next-step recommendations
   **And** the document summarizes the full optimization journey: baseline → Plateau 1 → Plateau 2 → Plateau 3

5. **Given** the final results
   **When** published benchmark documentation is updated
   **Then** `docs/benchmarks.md` reflects the latest numbers
   **And** competitive comparison tables are current

## Tasks / Subtasks

- [ ] Task 1: Full benchmark suite
  - [ ] 1.1 Enqueue: 1KB single-producer, multi-producer, 64KB
  - [ ] 1.2 Consume: 1, 10, 100 consumers
  - [ ] 1.3 End-to-end lifecycle: enqueue → consume → ack
  - [ ] 1.4 Multi-shard: 1, 4, 8 shards with multi-queue workload
  - [ ] 1.5 Single-queue with per-FK sharding (if 34.5 implemented)

- [ ] Task 2: Competitive benchmarks
  - [ ] 2.1 Run `make bench-competitive`
  - [ ] 2.2 Calculate Fila ÷ Kafka ratio per workload

- [ ] Task 3: Feature overhead measurement
  - [ ] 3.1 Throughput with DRR fairness enabled vs disabled
  - [ ] 3.2 Throughput with Lua hooks vs without
  - [ ] 3.3 Throughput with per-message ack vs batch commit

- [ ] Task 4: Write final analysis
  - [ ] 4.1 Save to `_bmad-output/planning-artifacts/research/kafka-parity-final-assessment.md`
  - [ ] 4.2 Full journey summary: baseline → P1 → P2 → P3
  - [ ] 4.3 Competitive ratio declaration
  - [ ] 4.4 Next steps (if any)

- [ ] Task 5: Update published benchmarks
  - [ ] 5.1 Update `docs/benchmarks.md`
  - [ ] 5.2 Update README performance summary

## Dev Notes

### Success Criteria

| Ratio (Fila ÷ Kafka) | Verdict |
|----------------------|---------|
| >= 0.8x | Kafka parity achieved |
| 0.5-0.8x | Near-parity, competitive for Fila's use case |
| 0.25-0.5x | Significant progress, above NATS/RabbitMQ tier |
| < 0.25x | Gap remains, further optimization needed |

### The Journey

| Stage | Expected Throughput | Competitive Ratio |
|-------|-------------------|------------------|
| Baseline (Epic 30) | 10,785 msg/s | 0.027x |
| Post-Plateau 1 (32.6) | 40-100K msg/s | 0.1-0.25x |
| Post-Plateau 2 (33.4) | 100-200K msg/s | 0.25-0.5x |
| Post-Plateau 3 (this) | 200-400K+ msg/s | 0.5-1.0x |

### References

- [Research: technical-kafka-parity-profiling-strategy-2026-03-25.md, Success Metrics] — KPI definitions
- [Story: 33-4-plateau-2-profile-checkpoint.md] — Plateau 2 results (dependency)
