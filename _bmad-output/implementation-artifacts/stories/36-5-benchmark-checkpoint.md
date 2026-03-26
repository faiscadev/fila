# Story 36.5: Benchmark Checkpoint — FIBP vs gRPC

**Epic:** 35-36 — gRPC Streaming Tuning + Custom Binary Protocol (FIBP)
**Status:** Done

## Goal

Measure and compare FIBP vs gRPC transport performance to validate the custom protocol investment and identify remaining bottlenecks.

## Acceptance Criteria

- [x] `BenchServer` supports FIBP: `start_with_fibp()` and `start_with_fibp_in_memory()` methods
- [x] `BenchServer::fibp_addr()` returns the FIBP listen address
- [x] `profile-workload` supports `PROFILE_TRANSPORT=fibp` env var
- [x] When FIBP transport selected, embedded server starts with FIBP enabled
- [x] Queue creation still uses gRPC (admin ops) regardless of transport
- [x] 7 benchmark scenarios run with both gRPC and FIBP (14 total, 2+ runs each)
- [x] Results document at `_bmad-output/planning-artifacts/research/epic-36-checkpoint-benchmarks.md`
- [x] Comparison table: gRPC vs FIBP with percentage improvement
- [x] Comparison against Epic 35 baseline showing cumulative improvement
- [x] FIBP consume flow control bottleneck identified and documented
- [x] NFR assessment against 100K msg/s target

## Implementation

### Modified files
- `crates/fila-bench/src/server.rs` — added FIBP support to `BenchServer`
- `crates/fila-bench/src/bin/profile-workload.rs` — added `PROFILE_TRANSPORT` env var, FIBP client connection

### New files
- `_bmad-output/planning-artifacts/research/epic-36-checkpoint-benchmarks.md` — full benchmark results

## Key Findings

1. **FIBP delivers ~90% improvement** over gRPC for single-producer enqueue (18.1K vs 9.6K msg/s on RocksDB)
2. **Transport is no longer the bottleneck** — server-side processing is now dominant
3. **FIBP consume is bottlenecked at 324 msg/s** due to conservative credit-based flow control (100ms timer, 32 credits/tick). This is a tuning issue, not a protocol limitation.
4. **Cumulative improvement from Epic 35 baseline: +74-83%** for enqueue workloads
5. **100K msg/s single-producer not achievable** without server-side optimizations; multi-producer (8-10) can approach it

## Design Decisions

1. **Separate gRPC and FIBP addresses** — `BenchServer` uses `FILA_FIBP_PORT_FILE` env var for port discovery (same pattern as e2e tests), keeping gRPC for admin ops and FIBP for data path.
2. **`FILA_STORAGE=memory` checked in profile-workload** — when starting embedded server with FIBP, the binary checks the env var to choose between `start_with_fibp()` and `start_with_fibp_in_memory()`.
