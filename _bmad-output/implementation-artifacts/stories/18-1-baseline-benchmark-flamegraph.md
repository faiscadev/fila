# Story 18.1: Baseline Benchmark & Flamegraph

Status: review

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

- [x] Task 1: Build release binary and run full benchmark suite (AC: 1)
  - [x] `cargo build --release --workspace`
  - [x] `cargo bench -p fila-bench --bench system` — recorded all output
  - [x] Benchmark results documented in research document
- [x] Task 2: Analyze tracing overhead on hot path (AC: 2)
  - [x] Profiled server with macOS `sample` tool during sustained 1KB enqueue workload (~6,131 msg/s)
  - [x] Binary built with `CARGO_PROFILE_RELEASE_DEBUG=2` (optimized + debug symbols)
  - [x] 12-second sample captures show 73.5% of enqueue CPU in `tracing::span::Span::new` → `Debug::fmt`
  - [x] Confirmed `request` parameter NOT skipped — payload bytes formatted twice (OTel + fmt layers)
  - [x] RocksDB WAL write is second bottleneck at 47.2% of scheduler thread active time
  - [x] Generated flamegraph SVG via inferno-collapse-sample + inferno-flamegraph
  - [x] Identified all 4 hot-path functions + 13 admin functions with same pattern
- [x] Task 3: Create baseline research document (AC: 1, 2)
  - [x] Written `_bmad-output/planning-artifacts/research/tracing-hot-path-baseline.md`
  - [x] Includes: raw benchmark numbers, tracing overhead analysis, optimization targets
  - [x] Identifies specific `#[instrument]` macros and the fix (`skip(self, request)`)
- [x] Task 4: PR with baseline numbers in description (AC: 3)

## Dev Notes

### Key Baseline Numbers

| Metric | Value |
|--------|-------|
| Enqueue throughput (1KB) | 6,697 msg/s |
| E2E latency p50 (light) | 0.20 ms |
| E2E latency p99 (light) | 0.57 ms |
| Fairness overhead | 1.41% |
| Lua overhead | 8.21 us |

### Identified Optimization: `skip(self, request)` on Hot-Path `#[instrument]`

All 4 hot-path functions use `skip(self)` but leave `request` to be Debug-formatted. The fix is `skip(self, request)` — this skips the expensive Debug-formatting while still capturing any future params. See research doc for full analysis including server-side profile data.

### References

- [Source: _bmad-output/planning-artifacts/research/tracing-hot-path-baseline.md] — full baseline and analysis
- [Source: crates/fila-server/src/service.rs] — hot-path #[instrument] macros

## Dev Agent Record

### Agent Model Used

Claude Opus 4.6 (1M context)

### Debug Log References

None.

### Completion Notes List

- Benchmark suite ran 10 categories successfully (queue depth skipped — optional, needs FILA_BENCH_DEPTH=1)
- Server profiled with macOS `sample` tool (no sudo required). 12-second capture during sustained 1KB enqueue workload.
- `#[instrument]` overhead confirmed: 73.5% of enqueue CPU in `Span::new` → `Debug::fmt` of `Request<EnqueueRequest>` (payload bytes formatted as decimal integers, twice — OTel + fmt layers).

### File List

- `_bmad-output/planning-artifacts/research/tracing-hot-path-baseline.md` (new)
- `_bmad-output/planning-artifacts/research/enqueue-flamegraph-baseline.svg` (new)
- `_bmad-output/implementation-artifacts/stories/18-1-baseline-benchmark-flamegraph.md` (new)
