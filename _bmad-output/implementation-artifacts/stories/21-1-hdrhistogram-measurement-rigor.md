# Story 21.1: Statistical Foundation — HdrHistogram & Measurement Rigor

Status: ready-for-dev

## Story

As a developer optimizing Fila's performance,
I want latency benchmarks that use HDR histograms with sufficient sample counts and extended percentile reporting,
so that I can trust p99+ numbers for engineering decisions instead of relying on single-datapoint noise.

## Acceptance Criteria

1. **Given** the `hdrhistogram` crate is added as a dependency to `fila-bench`
   **When** any latency benchmark runs
   **Then** latency is recorded into an `hdrhistogram::Histogram` (3 significant figures) instead of the current `LatencySampler`
   **And** `LatencySampler` is removed from `measurement.rs`

2. **Given** a latency benchmark (e2e latency light/moderate/saturated, compaction impact)
   **When** the benchmark completes
   **Then** at least 10,000 latency samples are collected per load level (up from 100)
   **And** measurement duration is at least 30 seconds per load level (configurable via `FILA_BENCH_DURATION_SECS` env var, default 30)

3. **Given** any benchmark that reports latency percentiles
   **When** results are emitted to the JSON report
   **Then** the report includes p50, p95, p99, p99.9, p99.99, and max values
   **And** the `BenchReport` schema is updated to support multiple percentile fields per latency metric

4. **Given** the CI regression workflow runs 3 times and aggregates
   **When** aggregation computes the median
   **Then** aggregation uses histogram merging via `Histogram::add()` (not median-of-percentiles)
   **And** percentiles are computed from the merged histogram

5. **Given** the benchmark suite runs end-to-end with the new measurement infrastructure
   **When** run 5 times on the same machine
   **Then** p99 latency variance is < 10% across runs (NFR-B1 reproducibility)

6. **Given** the benchmark harness records latency
   **When** comparing wall-clock overhead of HdrHistogram recording vs old LatencySampler
   **Then** measurement overhead does not exceed 1% of measured values (NFR-B3)

## Tasks / Subtasks

- [ ] Task 1: Replace LatencySampler with HdrHistogram (AC: 1)
  - [ ] 1.1 Add `hdrhistogram = "7"` to `crates/fila-bench/Cargo.toml`
  - [ ] 1.2 Create new `HdrHistogramRecorder` in `measurement.rs` wrapping `hdrhistogram::Histogram` with 3 sigfigs, recording in microseconds
  - [ ] 1.3 Provide method `percentiles(&self) -> PercentileSet` returning p50, p95, p99, p99.9, p99.99, max
  - [ ] 1.4 Provide `merge(&mut self, other: &Self)` method calling `Histogram::add()`
  - [ ] 1.5 Remove `LatencySampler` struct and all references
  - [ ] 1.6 Update all benchmark files that use `LatencySampler` to use `HdrHistogramRecorder`

- [ ] Task 2: Update BenchReport schema for extended percentiles (AC: 3)
  - [ ] 2.1 Change latency metric naming: instead of 3 separate `BenchResult` per load level (p50/p95/p99), emit 6 entries: p50, p95, p99, p99_9, p99_99, max
  - [ ] 2.2 Update `latency.rs` to emit all 6 percentile metrics per load level
  - [ ] 2.3 Update `compaction.rs` to emit extended percentiles for compaction-impact latency

- [ ] Task 3: Increase sample count and measurement duration (AC: 2)
  - [ ] 3.1 Replace `SAMPLES_PER_LEVEL = 100` with duration-based collection: run for `FILA_BENCH_DURATION_SECS` (env var, default 30) and collect as many samples as possible
  - [ ] 3.2 Add `FILA_BENCH_DURATION_SECS` env var parsing in the benchmark entry point
  - [ ] 3.3 Update latency benchmarks to use time-based loop instead of count-based loop
  - [ ] 3.4 Ensure minimum 10,000 samples collected per level; if duration expires with fewer samples, extend until minimum is met

- [ ] Task 4: Switch aggregation to histogram merging (AC: 4)
  - [ ] 4.1 Change `BenchResult` serialization: when the metric is a latency metric, include a base64-encoded serialized histogram in `metadata` (using `hdrhistogram::serialization::V2Serializer`)
  - [ ] 4.2 Update `aggregate.rs`: for latency metrics, deserialize histograms from each run, merge via `Histogram::add()`, compute percentiles from merged histogram
  - [ ] 4.3 For non-latency metrics (throughput, memory, etc.), keep existing median aggregation
  - [ ] 4.4 Update `bench-regression.yml` if any CLI argument changes are needed

- [ ] Task 5: Update bench-compare for new percentile metrics (AC: 4)
  - [ ] 5.1 Update `compare.rs` `higher_is_better()` to handle new metric name suffixes (`_p99_9`, `_p99_99`, `_max`)
  - [ ] 5.2 Verify regression detection works correctly for all 6 percentile levels

- [ ] Task 6: Verify reproducibility and overhead (AC: 5, 6)
  - [ ] 6.1 This is a manual verification AC — no code needed. The benchmark itself validates this when run multiple times.

## Dev Notes

### Key Files to Modify

- `crates/fila-bench/Cargo.toml` — add `hdrhistogram` dependency
- `crates/fila-bench/src/measurement.rs` — replace `LatencySampler` with HdrHistogram wrapper
- `crates/fila-bench/src/report.rs` — `BenchResult` schema (add histogram serialization to metadata)
- `crates/fila-bench/src/benchmarks/latency.rs` — main latency benchmark (update to HdrHistogram, duration-based collection, 6 percentile output)
- `crates/fila-bench/src/benchmarks/compaction.rs` — compaction impact latency (same changes)
- `crates/fila-bench/src/aggregate.rs` — switch to histogram merging for latency metrics
- `crates/fila-bench/src/compare.rs` — handle new metric name patterns
- `crates/fila-bench/benches/system.rs` — pass duration config to benchmarks

### Current Architecture (What Exists)

**LatencySampler** (`measurement.rs` lines 3–53): Collects `Vec<Duration>`, sorts on read, index-based percentile selection. Only returns p50/p95/p99.

**BenchResult** (`report.rs`): Flat `{name, value, unit, metadata}`. Each percentile is a separate entry: `e2e_latency_p50_light`, `e2e_latency_p95_light`, `e2e_latency_p99_light`.

**Latency benchmark** (`latency.rs`): 3 load levels (light=1 producer, moderate=5, saturated=20). Per level: pre-load background messages, drain, then sequential enqueue-one/consume-one for 100 samples.

**Aggregation** (`aggregate.rs`): Groups by metric name, sorts values across runs, picks median. Works per-scalar — no histogram awareness.

### Implementation Guidance

**HdrHistogram crate**: Use `hdrhistogram` version 7.x. Create with `Histogram::<u64>::new_with_bounds(1, 60_000_000, 3)` — 1µs to 60s range, 3 significant figures. Record values in microseconds (`duration.as_micros() as u64`).

**Histogram serialization for aggregation**: Use `V2Serializer` to serialize histograms to bytes, then base64-encode into the `metadata` map of `BenchResult` with key `"histogram"`. In `aggregate.rs`, decode and merge via `Histogram::add()`. This replaces median-of-percentiles with statistically correct merged histogram.

**Duration-based collection**: Replace the `for _ in 0..SAMPLES_PER_LEVEL` loop with `while start.elapsed() < duration && samples >= MIN_SAMPLES`. Use `MIN_SAMPLES = 10_000` as a floor.

**Metric naming convention**: Keep existing pattern but extend: `e2e_latency_p50_light`, `e2e_latency_p99_9_light`, `e2e_latency_p99_99_light`, `e2e_latency_max_light`.

**ThroughputMeter**: Leave `ThroughputMeter` in `measurement.rs` unchanged — it's unrelated to latency measurement.

### Testing

- All existing benchmarks must still compile and run
- Run `cargo bench -p fila-bench --bench system` to verify end-to-end
- Run `cargo build -p fila-bench --release` for the bench-compare and bench-aggregate binaries
- Verify JSON output includes all 6 percentile levels per latency metric
- Verify aggregation produces merged histogram results

### References

- [Source: _bmad-output/planning-artifacts/research/technical-benchmarking-methodology-research-2026-03-23.md — P0 gaps 1-3, Phase 1 Foundation]
- [Source: _bmad-output/planning-artifacts/epics.md — Epic 21, Story 21.1 ACs]
- [Source: crates/fila-bench/src/measurement.rs — current LatencySampler]
- [Source: crates/fila-bench/src/report.rs — current BenchResult/BenchReport schema]
- [Source: crates/fila-bench/src/aggregate.rs — current median aggregation]

## Dev Agent Record

### Agent Model Used

### Debug Log References

### Completion Notes List

### File List
