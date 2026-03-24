# Story 29.1: Fair Competitive Benchmark

Status: backlog

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

- [ ] Task 1: Update Fila throughput benchmark to use SDK auto-batching (AC: #1, #2)
  - [ ] 1.1 Replace serial unary `Enqueue` calls with `BatchMode::Auto` SDK client
  - [ ] 1.2 Use concurrent producers (match existing multi-producer pattern)
  - [ ] 1.3 Verify throughput scenario uses `BatchEnqueue` RPC under the hood

- [ ] Task 2: Preserve unbatched lifecycle scenario (AC: #3)
  - [ ] 2.1 Verify lifecycle scenario remains serial enqueue->consume->ack with `BatchMode::Disabled`
  - [ ] 2.2 Clearly label lifecycle as "unbatched" in results

- [ ] Task 3: Update results format and documentation (AC: #4, #5, #6)
  - [ ] 3.1 Add `batching` field to results JSON schema
  - [ ] 3.2 Update `METHODOLOGY.md` with fair comparison rationale
  - [ ] 3.3 Update `docs/benchmarks.md` with new results

- [ ] Task 4: Validate SDK auto-batching under benchmark load (AC: #8)
  - [ ] 4.1 Run benchmark and verify no SDK errors or hangs
  - [ ] 4.2 Fix any issues found in `fila-sdk`

- [ ] Task 5: End-to-end benchmark run (AC: #7, #9)
  - [ ] 5.1 Run all 4 brokers, verify valid JSON output
  - [ ] 5.2 Run existing test suite, verify zero regressions

## Design Notes

The competitive benchmark binary (`bench-competitive`) is built from `fila-bench` with `--features competitive`. It currently uses raw gRPC calls for Fila (bypassing the SDK). The fix should use `fila-sdk` with `BatchMode::Auto` for throughput scenarios, which will naturally use `BatchEnqueue` RPC when messages accumulate.

The key insight from profiling: Kafka's `linger.ms=5` + `batch.num.messages=1000` means ~1 network call per 1000 messages. Fila's `BatchMode::Auto` with concurrent producers should achieve similar amortization via the opportunistic batcher.

### Key Files to Modify

- `crates/fila-bench/` — competitive benchmark binary (throughput + multi-producer scenarios)
- `bench/competitive/METHODOLOGY.md` — fair comparison documentation
- `docs/benchmarks.md` — published results page

### References

- [Research: post-optimization-profiling-analysis-2026-03-24.md — competitive benchmark methodology gap]
- [Source: crates/fila-sdk/src/client.rs — BatchMode::Auto, run_auto_batcher]
- [Source: crates/fila-proto/proto/fila/v1/service.proto — BatchEnqueue RPC]
