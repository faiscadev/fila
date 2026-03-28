# Story 18.1: Baseline Benchmark & Flamegraph

Status: ready-for-dev

## Story

As an operator,
I want a documented performance baseline with flamegraph profiling,
So that subsequent optimizations can be measured against real data.

## Acceptance Criteria

1. **Given** the current codebase at Epic 17 head
   **When** `cargo bench -p fila-bench --bench system` runs the throughput benchmarks
   **Then** the results (msg/s for 1KB messages, p50/p99 latency) are recorded in a baseline research document at `_bmad-output/planning-artifacts/research/tracing-hot-path-baseline.md`

2. **Given** the benchmark suite has completed
   **When** a flamegraph is captured for a sustained enqueue workload
   **Then** the flamegraph clearly shows where CPU time is spent, with specific attention to `#[instrument]` / `Debug` / `fmt` formatting on hot-path functions (enqueue, ack, nack, consume delivery)

3. **Given** the baseline results are recorded
   **When** the PR is created
   **Then** the PR description includes the key throughput and latency numbers from the benchmark run

## Tasks / Subtasks

- [ ] Task 1: Build release binary and run full benchmark suite (AC: 1)
  - [ ] `cargo build --release --workspace`
  - [ ] `cargo bench -p fila-bench --bench system` — record all output
  - [ ] Copy `bench-results.json` content into the research document
- [ ] Task 2: Capture flamegraph of enqueue hot path (AC: 2)
  - [ ] Install `cargo-flamegraph` if not present
  - [ ] Run a sustained enqueue workload under `flamegraph` or `perf`/`dtrace` profiling
  - [ ] Analyze the flamegraph for `#[instrument]`, `Debug::fmt`, `tracing` overhead
  - [ ] Document findings: which functions show tracing overhead, estimated % of CPU time
- [ ] Task 3: Create baseline research document (AC: 1, 2)
  - [ ] Write `_bmad-output/planning-artifacts/research/tracing-hot-path-baseline.md`
  - [ ] Include: raw benchmark numbers, flamegraph analysis, identified hot-path tracing overhead
  - [ ] Identify specific `#[instrument]` macros that Debug-format payloads
- [ ] Task 4: PR with baseline numbers in description (AC: 3)

## Dev Notes

### Benchmark Infrastructure (already exists)

The `fila-bench` crate at `crates/fila-bench/` has a complete benchmark harness. **Do NOT create new benchmarks** — use the existing ones.

- **Entry point:** `cargo bench -p fila-bench --bench system` (harness=false, runs `benches/system.rs`)
- **10 categories:** throughput, latency (3 load levels), fairness overhead, fairness accuracy, Lua overhead, memory footprint, compaction impact, key cardinality, consumer concurrency, queue depth
- **Server management:** `BenchServer` in `crates/fila-bench/src/server.rs` — spawns release binary on random port, auto-cleanup
- **Output:** `bench-results.json` + formatted stdout summary

### Hot-Path `#[instrument]` Locations

These are the instrumented hot-path functions to examine in the flamegraph:

- `crates/fila-server/src/service.rs:62` — `enqueue()` with `#[instrument(skip(self), fields(queue_id, msg_id))]`
- `crates/fila-server/src/service.rs:181` — `consume()` with `#[instrument(skip(self), fields(queue_id))]`
- `crates/fila-server/src/service.rs:299` — `ack()` with `#[instrument(skip(self), fields(queue_id, msg_id))]`
- `crates/fila-server/src/service.rs:391` — `nack()` with `#[instrument(skip(self), fields(queue_id, msg_id))]`

The epic hypothesis: `#[instrument]` Debug-formats request payloads (including 1KB message bodies as hex) twice per request, causing +15% overhead. The flamegraph should confirm or refute this.

### Flamegraph Approach

On macOS (darwin), use `cargo flamegraph` with `dtrace` backend, or use `samply` for profiling. The goal is to capture CPU time during a sustained enqueue workload (e.g., 10K messages) and identify `Debug::fmt`, `tracing_core`, and `#[instrument]` overhead.

If `cargo-flamegraph` doesn't work on macOS (SIP restrictions), alternatives:
- `samply record` — works well on macOS
- `instruments` — Xcode profiler
- Manual `dtrace` with collapsed stacks

### Research Document Structure

Create `_bmad-output/planning-artifacts/research/tracing-hot-path-baseline.md` with:
1. **Benchmark Results** — full output from `cargo bench -p fila-bench --bench system`
2. **Flamegraph Analysis** — which functions consume CPU, where tracing overhead appears
3. **Identified Optimization Targets** — specific `#[instrument]` macros to fix in Story 18.2
4. **Baseline Numbers** — key metrics to compare against after Story 18.2

### Project Structure Notes

- Benchmark crate: `crates/fila-bench/` (existing, do not modify)
- Research output: `_bmad-output/planning-artifacts/research/` (existing directory)
- No code changes in this story — measurement only

### References

- [Source: crates/fila-bench/src/benchmarks/] — all benchmark modules
- [Source: crates/fila-bench/src/server.rs] — BenchServer lifecycle
- [Source: crates/fila-server/src/service.rs] — hot-path #[instrument] macros
- [Source: benches/system.rs] — benchmark harness entry point

## Dev Agent Record

### Agent Model Used

### Debug Log References

### Completion Notes List

### File List
