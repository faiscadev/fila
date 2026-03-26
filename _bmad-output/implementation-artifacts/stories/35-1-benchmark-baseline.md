# Story 35.1: Benchmark Baseline — Measure Post-Plateau State

Status: done

## Story

As a developer,
I want actual benchmark numbers for the current codebase (post-Plateaus 1-3 + tracing fix),
So that I have a verified starting point for Phase 1 tuning and can quantify the cumulative impact of Epics 32-34.

## Acceptance Criteria

1. **Given** the current codebase at HEAD of main
   **When** the full benchmark suite is executed (`cargo bench -p fila-bench --bench system`)
   **Then** the following scenarios are measured and numbers pasted into the results document:
   - Single-producer enqueue (1KB, RocksDB) — **10,412 msg/s avg**
   - Single-producer enqueue (1KB, InMemoryEngine) — **11,208 msg/s avg**
   - 4-producer enqueue (1KB, RocksDB) — **23,615 msg/s avg**
   - 4-producer enqueue (1KB, InMemoryEngine) — **~24,985 msg/s**
   - Lifecycle enqueue+consume+ack (1KB, RocksDB) — **6,471 msg/s avg**
   - Lifecycle enqueue+consume+ack (1KB, InMemoryEngine) — **~7,163 msg/s**
   - Batch enqueue with SDK accumulator (1KB, RocksDB) — **11,079 msg/s avg**

2. **Given** benchmark scenarios
   **When** each scenario runs
   **Then** each runs for at least 10 seconds with 2+ runs for stability — **DONE** (15s runs, 2 runs each)

3. **Given** the results
   **When** a server-side flamegraph is captured during single-producer enqueue (InMemoryEngine)
   **Then** the current CPU distribution is verified — **DONE** (h2/hyper ~53%, SDK ~22%, tokio ~14%, app ~11%)

4. **Given** the results document
   **When** comparison against pre-Plateau numbers (8,264 msg/s baseline) is included
   **Then** cumulative gain is clearly documented — **+26% from 8,264 to 10,412 msg/s**

5. **Given** the benchmark document
   **When** committed as `_bmad-output/planning-artifacts/research/epic-35-baseline-benchmarks.md`
   **Then** it includes all measured numbers with no estimates or projections — **DONE**

## Tasks / Subtasks

- [x] Task 1: Run full benchmark suite (AC: 1, 2)
  - [x] 1.1 Build release binaries: `cargo build --release -p fila-bench`
  - [x] 1.2 Run `cargo bench -p fila-bench --bench system` (RocksDB default) — 2 runs
  - [x] 1.3 Run with `FILA_STORAGE=memory cargo bench -p fila-bench --bench system` — 2 runs
  - [x] 1.4 Record all throughput numbers from benchmark output

- [x] Task 2: Capture flamegraph (AC: 3)
  - [x] 2.1 Run `profile-workload` with InMemoryEngine for flamegraph capture
  - [x] 2.2 Document CPU distribution percentages

- [x] Task 3: Write results document (AC: 4, 5)
  - [x] 3.1 Create `_bmad-output/planning-artifacts/research/epic-35-baseline-benchmarks.md`
  - [x] 3.2 Include comparison table: pre-Plateau baseline vs current
  - [x] 3.3 Include flamegraph CPU distribution analysis
  - [x] 3.4 Commit the document

## Dev Notes

- The benchmark suite at `crates/fila-bench/benches/system.rs` runs 10+ categories including throughput, latency, fairness, scaling
- `profile-workload` binary at `crates/fila-bench/src/bin/profile-workload.rs` supports `PROFILE_WORKLOAD`, `PROFILE_DURATION`, `PROFILE_MSG_SIZE` env vars
- InMemoryEngine is activated via `FILA_STORAGE=memory` environment variable
- Pre-Plateau baseline numbers (from Epic 30 research): 8,264 msg/s single-producer 1KB enqueue
- This is a measurement-only story — no code changes required

### References

- [Source: _bmad-output/planning-artifacts/research/epic-35-baseline-benchmarks.md]
- [Source: _bmad-output/planning-artifacts/research/technical-custom-transport-kafka-parity-2026-03-25.md]
- [Source: crates/fila-bench/benches/system.rs]
- [Source: crates/fila-bench/src/bin/profile-workload.rs]

## Dev Agent Record

### Agent Model Used

Claude Opus 4.6 (1M context)

### Debug Log References

None.

### Completion Notes List

- All benchmark numbers are from actual measured runs, no estimates
- Single-producer RocksDB: 10,412 msg/s (+26% vs 8,264 pre-Plateau baseline)
- InMemoryEngine ~8% faster than RocksDB (11,208 vs 10,412) — storage is not the bottleneck
- Flamegraph confirms HTTP/2 transport dominates at ~53% of client CPU
- Batch scaling shows dramatic improvement with batch size (183K msg/s at bs=1000)

### File List

- _bmad-output/planning-artifacts/research/epic-35-baseline-benchmarks.md (NEW)
- _bmad-output/implementation-artifacts/stories/35-1-benchmark-baseline.md (MODIFIED)
