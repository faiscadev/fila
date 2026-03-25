# Story 32.6: Plateau 1 Profile Checkpoint — Results & Go/No-Go for Plateau 2

Status: pending

## Story

As a developer,
I want full profiling and competitive benchmarks after all Plateau 1 optimizations,
So that the new bottleneck is identified and a data-driven go/no-go decision is made for Plateau 2.

## Acceptance Criteria

1. **Given** stories 32.2-32.5 are complete
   **When** `fila-bench` enqueue throughput is measured
   **Then** the result is compared against the 32.1 baseline (10,785 msg/s)
   **And** per-story improvement attribution is documented

2. **Given** the enqueue path has been optimized
   **When** consume throughput is measured (1 consumer, 10 consumers, 100 consumers)
   **Then** consume throughput is quantified and compared against the baseline
   **And** the consume-to-enqueue throughput ratio is documented

3. **Given** the optimized code
   **When** CPU flamegraphs are generated
   **Then** the new hot path is identified (what is now the #1 CPU consumer?)
   **And** the flamegraph is compared side-by-side with the 32.1 baseline

4. **Given** the optimized code
   **When** tracing spans are collected
   **Then** the new per-operation cost breakdown is documented
   **And** the 93μs decomposition is updated with measured post-Plateau-1 values

5. **Given** the competitive benchmark suite
   **When** Fila is benchmarked against Kafka, RabbitMQ, and NATS
   **Then** the competitive ratio (Fila ÷ Kafka) is updated
   **And** the result validates or invalidates the research's Plateau 1 projection (40-100K msg/s)

6. **Given** all profiling data
   **When** an analysis document is written
   **Then** it identifies whether the consume path is now the bottleneck (as predicted by research)
   **And** it provides a go/no-go recommendation for Epic 33 (Plateau 2 — Fix the Consume Path)
   **And** if go: it identifies which Pattern (P2: in-memory delivery, P3: in-memory lease) should be prioritized
   **And** if no-go: it identifies what unexpected bottleneck was found and proposes alternative next steps

## Tasks / Subtasks

- [ ] Task 1: Enqueue throughput benchmarks
  - [ ] 1.1 Run `fila-bench` enqueue (1KB, single producer)
  - [ ] 1.2 Run multi-producer benchmark (4 producers)
  - [ ] 1.3 Run 64KB message benchmark
  - [ ] 1.4 Document per-story attribution table

- [ ] Task 2: Consume throughput benchmarks
  - [ ] 2.1 Run consume benchmark: 1, 10, 100 consumers
  - [ ] 2.2 Compare against baseline consume numbers

- [ ] Task 3: CPU flamegraphs + tracing spans
  - [ ] 3.1 Generate flamegraphs for enqueue and consume
  - [ ] 3.2 Collect tracing span data
  - [ ] 3.3 Update per-operation cost breakdown

- [ ] Task 4: Competitive benchmarks
  - [ ] 4.1 Run `make bench-competitive`
  - [ ] 4.2 Update competitive ratio

- [ ] Task 5: Write analysis and go/no-go document
  - [ ] 5.1 Write to `_bmad-output/planning-artifacts/research/plateau-1-results-analysis.md`
  - [ ] 5.2 Go/no-go recommendation for Epic 33

## Dev Notes

### Expected Outcome

The research projects 40-100K msg/s enqueue after Plateau 1. Previous projections missed by 3x, so anything from 20K to 100K+ is plausible. The critical question is: what is the new bottleneck?

**If consume is the bottleneck (expected):** Go for Plateau 2 (in-memory delivery + lease tracking).
**If RocksDB writes dominate:** Consider Titan blob separation (Epic 34, Story 34.1) before Plateau 2.
**If something unexpected:** Revise the roadmap based on measurement.

### Key Metrics to Capture

| Metric | 32.1 Baseline | Post-Plateau-1 |
|--------|--------------|----------------|
| Enqueue throughput (1KB, 1 producer) | 10,785 msg/s | ? |
| Consume throughput (1 consumer) | ~401 msg/s | ? |
| Consume throughput (100 consumers) | ~2,500 msg/s | ? |
| Multi-producer (4x) throughput | ~22K msg/s | ? |
| Per-message enqueue cost | ~93μs | ? |
| Per-message consume cost | ~100-200μs | ? |
| Competitive ratio (Fila ÷ Kafka) | 0.027x | ? |

### References

- [Research: technical-kafka-parity-profiling-strategy-2026-03-25.md, "Profile Between Plateaus"] — Design principle
- [Story: 32-1-profile-baseline.md] — Baseline profiling methodology
