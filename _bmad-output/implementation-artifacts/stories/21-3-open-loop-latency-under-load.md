# Story 21.3: Open-Loop Load Generation & Latency-Under-Load Benchmarks

Status: review

## Story

As a developer investigating tail latency under realistic load,
I want an open-loop load generator that sends at a fixed rate regardless of response time, with workloads for processing delay, backpressure, and queue depth effects,
so that latency measurements include coordinated-omission-corrected response time, not just service time.

## Acceptance Criteria

1. Open-loop producer sends at fixed rate via `tokio::time::interval`, each request spawned independently, latency = `completed_time - scheduled_time`, CO-corrected via `record_correct`
2. Per-worker histograms merged via `Histogram::add()`
3. "Latency under load" benchmark: 50%/80%/100% of max throughput, 30s per level
4. "Consumer processing time" benchmark: 0/1/10/100ms delays, concurrent consume, reports throughput + latency
5. "Backpressure ramp" benchmark: 10%-150% in 10% steps, 10s each, identifies saturation inflection
6. "Queue depth latency" benchmark: 0/1K/10K/100K pre-loaded messages, 10s measurement per depth

## Tasks / Subtasks

- [x] Task 1: Add `record_corrected()` to LatencyHistogram in measurement.rs
- [x] Task 2: Create `open_loop.rs` module with 4 benchmarks
- [x] Task 3: Register module in `benchmarks/mod.rs`
- [x] Task 4: Add benchmarks to `system.rs` gated behind `FILA_BENCH_OPENLOOP=1`

## Dev Agent Record

### Agent Model Used
Claude Opus 4.6 (1M context)

### Debug Log References
None.

### Completion Notes List
- Saturation probe (5s closed-loop) discovers max throughput before load-level benchmarks
- Open-loop pattern: `tokio::time::interval` + `tokio::spawn` per request
- 4 concurrent consumer tasks for processing time benchmark
- All benchmarks gated behind `FILA_BENCH_OPENLOOP=1` env var (long-running)

### File List
- `crates/fila-bench/src/measurement.rs` — added `record_corrected()`
- `crates/fila-bench/src/benchmarks/open_loop.rs` — NEW: 4 open-loop benchmarks
- `crates/fila-bench/src/benchmarks/mod.rs` — added `pub mod open_loop`
- `crates/fila-bench/benches/system.rs` — registered benchmarks [11-14]
