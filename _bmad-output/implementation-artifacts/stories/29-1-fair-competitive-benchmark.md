# Story 29.1: Fair Competitive Benchmark

Status: review

## Story

As a developer evaluating Fila against other brokers,
I want the competitive benchmark to use each broker's recommended high-throughput configuration,
So that the comparison reflects real-world performance rather than penalizing Fila for not using its own batching features.

## Acceptance Criteria

1. **Given** the competitive benchmark (`bench/competitive/`) currently sends 1 Fila message per RPC while Kafka uses `linger.ms=5` + `batch.num.messages=1000`
   **When** the benchmark is updated for fair comparison
   **Then** the throughput scenario uses the Rust SDK with `BatchMode::Auto` (default) and multiple concurrent producers, matching how each broker recommends high-throughput usage

2. **And** the multi-producer scenario also uses `BatchMode::Auto` for Fila

3. **And** the lifecycle scenario (enqueue->consume->ack) remains unbatched for all brokers — this is the latency-focused, fair serial comparison where Fila already leads

4. **And** results JSON includes a `batching` field per scenario indicating the batching strategy used (e.g., `"auto"`, `"none"`, `"linger_ms=5"`)

5. **And** `METHODOLOGY.md` is updated to document: why throughput scenarios use each broker's recommended batching, why lifecycle stays unbatched, and what "fair comparison" means

6. **And** `docs/benchmarks.md` is updated with the new results and clearly labels which scenarios use batching vs unbatched

7. **And** the benchmark is run end-to-end (all 4 brokers) and produces valid results JSON

8. **And** SDK auto-batching (`BatchMode::Auto`) is validated to work correctly under the benchmark's concurrent producer load — if any issues are found, they are fixed in `fila-sdk` as part of this story

9. **And** all existing tests pass (zero regressions)

## Tasks / Subtasks

- [x] Task 1: Update Fila throughput benchmark to use SDK auto-batching (AC: #1, #2)
  - [x] 1.1 Throughput now uses 4 concurrent producers with `BatchMode::Auto` (default)
  - [x] 1.2 Concurrent producers trigger auto-batching via message accumulation
  - [x] 1.3 Verified: auto-batcher uses `BatchEnqueue` RPC when messages accumulate

- [x] Task 2: Preserve unbatched lifecycle scenario (AC: #3)
  - [x] 2.1 Lifecycle uses explicit `BatchMode::Disabled` for serial enqueue→consume→ack
  - [x] 2.2 Latency also uses `BatchMode::Disabled` for accurate per-message measurement
  - [x] 2.3 All results include `batching` metadata: `"none"` for lifecycle/latency

- [x] Task 3: Update results format and documentation (AC: #4, #5, #6)
  - [x] 3.1 Added `batching` field to all result metadata across all 4 brokers
  - [x] 3.2 Updated `METHODOLOGY.md` with batching table and fair comparison rationale
  - [x] 3.3 Updated `docs/benchmarks.md` with batching labels, cleared stale numbers

- [ ] Task 4: Validate SDK auto-batching under benchmark load (AC: #8)
  - [ ] 4.1 Run benchmark and verify no SDK errors or hangs (requires Docker)
  - [ ] 4.2 Fix any issues found in `fila-sdk`

- [x] Task 5: Verify no regressions (AC: #9)
  - [x] 5.1 All 437 non-e2e tests pass
  - [ ] 5.2 End-to-end benchmark run requires Docker (AC: #7)

## Design Notes

The competitive benchmark binary is at `crates/fila-bench/src/bin/bench-competitive.rs` (~1,420 lines). It already uses `fila-sdk` (`client.enqueue()`, `client.consume()`, `client.ack()`), but connects with default settings — which means `BatchMode::Auto` is technically active but with a single producer per throughput scenario. The issue is that with only 1 producer sending sequentially, auto-batching rarely kicks in because the batcher sees idle → sends immediately (Nagle-style: sends immediately when no RPC is in flight).

**What needs to change:** The throughput and multi-producer scenarios need concurrent producers so that messages accumulate while RPCs are in flight, triggering the auto-batcher to batch them via `BatchEnqueue` RPC. This matches how Kafka's `linger.ms=5` amortizes network calls.

**What stays the same:** Lifecycle scenario (enqueue→consume→ack) remains serial/unbatched — this is where Fila already beats Kafka 7.6x and RabbitMQ 4.1x.

**Current broker tuning in benchmark:**
- Kafka throughput: `linger.ms=5`, `batch.num.messages=1000` (batched)
- Kafka latency/lifecycle: `linger.ms=0` (unbatched)
- RabbitMQ: `basic_publish` per message (no client-side batching)
- NATS: `publish` per message (no client-side batching)
- Fila: `client.enqueue()` per message, single producer (effectively unbatched despite SDK)

**Results JSON format:** Each benchmark emits `{ name, value, unit, metadata }`. Add `batching` to `metadata`.

**Benchmark infrastructure:**
- `Makefile` orchestrates via docker-compose, 3 runs per broker, median aggregation via `bench-aggregate`
- Measurement: `ThroughputMeter` and `LatencyHistogram` from `fila_bench::measurement`
- Container stats via `docker stats` parsing

### Key Files to Modify

- `crates/fila-bench/src/bin/bench-competitive.rs` — Fila throughput + multi-producer scenarios (use concurrent producers to trigger auto-batching)
- `bench/competitive/METHODOLOGY.md` — fair comparison rationale
- `docs/benchmarks.md` — published results with batching labels

### References

- [Research: _bmad-output/planning-artifacts/research/post-optimization-profiling-analysis-2026-03-24.md]
- [Source: crates/fila-sdk/src/client.rs — BatchMode::Auto, run_auto_batcher (lines 605-636)]
- [Source: crates/fila-proto/proto/fila/v1/service.proto — BatchEnqueue RPC]
- [Source: crates/fila-bench/src/bin/bench-competitive.rs — current benchmark implementation]

## Dev Agent Record

### Agent Model Used

Claude Opus 4.6 (1M context)

### Debug Log References

None.

### Completion Notes List

- Throughput scenarios now use 4 concurrent producers (THROUGHPUT_PRODUCERS const) to trigger Nagle-style auto-batching
- Lifecycle and latency scenarios explicitly use `BatchMode::Disabled` via `ConnectOptions::new(addr).with_batch_mode(BatchMode::Disabled)`
- `batching_meta()` helper function added at module scope for reuse across all 4 broker modules
- All broker results (Kafka, RabbitMQ, NATS, Fila) now include `batching` metadata
- METHODOLOGY.md gained a "Batching & Fair Comparison" section with broker batching table
- docs/benchmarks.md cleared stale throughput numbers (will be populated on next benchmark run) and added batching context
- Tasks 4.1 and 5.2 (end-to-end benchmark run) require Docker and broker containers — deferred to PR verification

### File List

- `crates/fila-bench/src/bin/bench-competitive.rs` — Major rewrite of Fila benchmark module + batching metadata for all brokers
- `bench/competitive/METHODOLOGY.md` — Fair comparison documentation
- `docs/benchmarks.md` — Updated competitive comparison section with batching labels
