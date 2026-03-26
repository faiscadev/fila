# Story 35.1: Benchmark Baseline — Measure Post-Plateau State

Status: ready-for-dev

## Story

As a developer,
I want actual benchmark numbers for the current codebase (post-Plateaus 1-3 + tracing fix),
So that I have a verified starting point for Phase 1 tuning and can quantify the cumulative impact of Epics 32-34.

## Acceptance Criteria

1. **Given** the current codebase at HEAD of main
   **When** the full benchmark suite is executed (`cargo bench -p fila-bench --bench system`)
   **Then** the following scenarios are measured and numbers pasted into the results document:
   - Single-producer enqueue (1KB, RocksDB)
   - Single-producer enqueue (1KB, InMemoryEngine via `FILA_STORAGE=memory`)
   - 4-producer enqueue (1KB, RocksDB)
   - 4-producer enqueue (1KB, InMemoryEngine)
   - Lifecycle enqueue+consume+ack (1KB, RocksDB)
   - Lifecycle enqueue+consume+ack (1KB, InMemoryEngine)
   - Batch enqueue with SDK accumulator (1KB, RocksDB)

2. **Given** benchmark scenarios
   **When** each scenario runs
   **Then** each runs for at least 10 seconds with 2+ runs for stability

3. **Given** the results
   **When** a server-side flamegraph is captured during single-producer enqueue (InMemoryEngine)
   **Then** the current CPU distribution is verified

4. **Given** the results document
   **When** comparison against pre-Plateau numbers (8,264 msg/s baseline) is included
   **Then** cumulative gain is clearly documented

5. **Given** the benchmark document
   **When** committed as `_bmad-output/planning-artifacts/research/epic-35-baseline-benchmarks.md`
   **Then** it includes all measured numbers with no estimates or projections

## Tasks / Subtasks

- [ ] Task 1: Run full benchmark suite (AC: 1, 2)
  - [ ] 1.1 Build release binaries: `cargo build --release -p fila-bench`
  - [ ] 1.2 Run `cargo bench -p fila-bench --bench system` (RocksDB default) — 2 runs
  - [ ] 1.3 Run with `FILA_STORAGE=memory cargo bench -p fila-bench --bench system` — 2 runs
  - [ ] 1.4 Record all throughput numbers from benchmark output

- [ ] Task 2: Capture flamegraph (AC: 3)
  - [ ] 2.1 Run `profile-workload` with InMemoryEngine for flamegraph capture
  - [ ] 2.2 Document CPU distribution percentages

- [ ] Task 3: Write results document (AC: 4, 5)
  - [ ] 3.1 Create `_bmad-output/planning-artifacts/research/epic-35-baseline-benchmarks.md`
  - [ ] 3.2 Include comparison table: pre-Plateau baseline vs current
  - [ ] 3.3 Include flamegraph CPU distribution analysis
  - [ ] 3.4 Commit the document

## Dev Notes

- The benchmark suite at `crates/fila-bench/benches/system.rs` runs 10+ categories including throughput, latency, fairness, scaling
- `profile-workload` binary at `crates/fila-bench/src/bin/profile-workload.rs` supports `PROFILE_WORKLOAD`, `PROFILE_DURATION`, `PROFILE_MSG_SIZE` env vars
- InMemoryEngine is activated via `FILA_STORAGE=memory` environment variable
- Pre-Plateau baseline numbers (from Epic 30 research): 8,264 msg/s single-producer 1KB enqueue
- Post-Plateau numbers from epic plan: 9,488 msg/s (RocksDB), 10,344 msg/s (InMemory) — these are the expected ballpark
- Flamegraph script at `scripts/flamegraph.sh` if available, otherwise use `cargo flamegraph` or `samply`
- This is a measurement-only story — no code changes required

### References

- [Source: _bmad-output/planning-artifacts/research/technical-custom-transport-kafka-parity-2026-03-25.md]
- [Source: crates/fila-bench/benches/system.rs]
- [Source: crates/fila-bench/src/bin/profile-workload.rs]

## Dev Agent Record

### Agent Model Used

Claude Opus 4.6 (1M context)

### Debug Log References

### Completion Notes List

### File List
