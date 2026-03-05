# Story 12.4: Published Results & Benchmark Dashboard

Status: ready-for-dev

## Story

As an evaluator,
I want to view Fila's benchmark results and competitive positioning in published form,
so that I can reference performance data during architecture evaluation.

## Acceptance Criteria

1. **Given** benchmark results exist from Stories 12.1 and 12.3, **when** the results are published, **then** a `docs/benchmarks.md` page presents self-benchmark results with formatted tables for throughput, latency percentiles, and scaling curves.

2. **Given** the benchmarks page exists, **when** the competitive comparison is presented, **then** results are shown as tables with clear methodology links.

3. **Given** the README exists, **when** a user reads it, **then** it includes a performance summary line linking to the full benchmarks page.

4. **Given** historical benchmark results, **when** a release is cut, **then** CI updates results on each release (via the existing bench-regression workflow baseline mechanism).

5. **Given** the methodology documentation, **when** an external evaluator reads it, **then** exact commands, hardware specs, and configuration files are detailed enough for external reproduction.

6. **Given** the benchmarks page, **when** results are presented, **then** the page acknowledges limitations: hardware-specific results, configuration choices, workload representativeness.

7. **Given** results are published, **when** an evaluator reads them, **then** results include the Fila version and commit hash for traceability.

## Tasks / Subtasks

- [x] Task 1: Create `docs/benchmarks.md` with self-benchmark results (AC: 1, 5, 6, 7)
  - [x] Self-benchmark results tables: throughput, latency (p50/p95/p99 at light/moderate/saturated), fairness overhead, Lua overhead
  - [x] Scaling tables: key cardinality, consumer concurrency
  - [x] Memory and compaction impact results
  - [x] Fila-only features section: fair scheduling, throttle-aware delivery
  - [x] Methodology summary with link to `bench/competitive/METHODOLOGY.md`
  - [x] Limitations section (hardware-specific, config sensitivity, single-node only)
  - [x] Version and commit hash note

- [x] Task 2: Add competitive comparison section to `docs/benchmarks.md` (AC: 2, 5, 6)
  - [x] Workload descriptions table
  - [x] Broker configuration comparison table
  - [x] Reproduction instructions (`make bench-competitive`)
  - [x] Client language mismatch caveat (Rust for Fila, Python for competitors)
  - [x] Methodology link

- [x] Task 3: Add performance summary to README (AC: 3)
  - [x] Added "Performance" section between "Key concepts" and "Client SDKs"
  - [x] Link to `docs/benchmarks.md` for full results

- [x] Task 4: Document CI integration for result updates (AC: 4)
  - [x] CI regression detection section in benchmarks.md
  - [x] Reference to bench-regression workflow and artifacts

- [ ] Task 5: Update sprint status and tracking (AC: all)
  - [ ] Update sprint-status.yaml
  - [ ] Update epic-execution-state.yaml

## Dev Notes

### Architecture & Design Decisions

**Documentation-only story — no Rust code changes.**
This story creates `docs/benchmarks.md` and adds a performance section to `README.md`. No server, SDK, or tooling changes.

**Placeholder results with realistic structure.**
Since actual benchmark numbers are hardware-specific, the benchmarks page should use placeholder values marked as "example results" from a reference run. The structure (tables, sections, formatting) is the deliverable — not the specific numbers. When users run `cargo bench -p fila-bench` or `make bench-competitive`, they'll get their own numbers.

**Link to METHODOLOGY.md for full reproduction details.**
`docs/benchmarks.md` summarizes methodology but links to `bench/competitive/METHODOLOGY.md` for the complete details (broker configs, measurement windows, warmup, etc.).

### Existing Benchmark Infrastructure

**Self-benchmarks** (`crates/fila-bench/`):
- JSON output with `BenchReport` schema: `{ version, timestamp, commit, benchmarks: [{ name, value, unit, metadata }] }`
- Metric names: `enqueue_throughput_1kb`, `e2e_latency_p50_light`, `e2e_latency_p99_saturated`, `fairness_overhead_pct`, `fairness_accuracy_pct`, `lua_overhead_p99_us`, `queue_depth_1m_throughput`, `cardinality_10k_throughput`, `consumer_concurrency_100`, `memory_per_msg_bytes`, `compaction_p99_impact_us`
- Run: `cargo bench -p fila-bench --bench system`

**Competitive benchmarks** (`bench/competitive/`):
- 6 workloads per broker: throughput (3 sizes), latency, lifecycle, multi-producer, fan-out
- Run: `make bench-competitive` from `bench/competitive/`
- Results in `bench/competitive/results/bench-{broker}.json`

### Table Formats

Use standard markdown tables. Example self-benchmark table:

```markdown
| Metric | Value | Unit | Notes |
|--------|------:|------|-------|
| Enqueue throughput (1KB) | 125,000 | msg/s | Single producer |
```

Example competitive comparison table:

```markdown
| Broker | 64B | 1KB | 64KB |
|--------|----:|----:|-----:|
| Fila | 150,000 | 125,000 | 45,000 |
| Kafka | 200,000 | 180,000 | 95,000 |
```

### README Integration

Insert a performance section after "Key concepts" and before "Client SDKs" in `README.md`. Keep it brief — 3-4 lines max with a link to the full page.

### What NOT To Do

- Do NOT hardcode specific benchmark numbers as if they are official — mark them as "example results from reference hardware"
- Do NOT duplicate the full methodology in docs/benchmarks.md — link to bench/competitive/METHODOLOGY.md
- Do NOT modify any Rust code, CI workflows, or benchmark scripts
- Do NOT create a separate dashboard application — "dashboard" means the documentation page itself

### References

- [Source: crates/fila-bench/src/report.rs] — BenchReport JSON schema
- [Source: bench/competitive/METHODOLOGY.md] — Competitive benchmark methodology
- [Source: bench/competitive/bench.py] — Python benchmark harness
- [Source: bench/competitive/bench-fila.sh] — Fila self-benchmark wrapper
- [Source: docs/] — Existing documentation structure
- [Source: README.md] — Current README (insert point between "Key concepts" and "Client SDKs")

## Dev Agent Record

### Agent Model Used
Claude Opus 4.6

### Debug Log References
None — documentation-only story, no debug issues.

### Completion Notes List
- Created `docs/benchmarks.md` with actual self-benchmark results from local run (commit d355972)
- Self-benchmark sections: throughput, latency (3 load levels), fairness overhead, fairness accuracy, Lua overhead, key cardinality scaling, consumer concurrency, memory footprint, compaction impact
- Competitive comparison section structured with workload descriptions, broker configs, reproduction instructions, and methodology link
- README updated with "Performance" section linking to full benchmarks page
- CI regression detection documented with workflow artifact references
- Limitations prominently acknowledged (hardware-specific, client language mismatch, Docker overhead, single-node only)
- Traceability section with commit hash reference

### File List
- `docs/benchmarks.md` (new)
- `README.md` (modified — added Performance section)
