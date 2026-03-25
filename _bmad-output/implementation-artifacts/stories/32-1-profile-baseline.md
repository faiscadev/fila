# Story 32.1: Profile Baseline — End-to-End Hot Path Measurement

Status: pending

## Story

As a developer,
I want flamegraph and tracing-span profiling of the current 10.8K msg/s enqueue and consume paths,
So that optimization work in stories 32.2-32.5 targets measured bottlenecks, not estimates.

## Acceptance Criteria

1. **Given** the current codebase at ~10,785 msg/s enqueue throughput
   **When** CPU flamegraphs are generated for the enqueue path
   **Then** the flamegraph shows per-function CPU time breakdown from gRPC handler through scheduler to RocksDB commit
   **And** the flamegraph is saved as a reproducible artifact (SVG + raw perf data)

2. **Given** the enqueue hot path
   **When** tracing spans are added to key operations
   **Then** wall-clock time is measured for: `prepare_enqueue()` per message, protobuf serialization, message clone, storage key generation, mutation collection, RocksDB WriteBatch commit, `finalize_enqueue()`, and DRR delivery trigger
   **And** the tracing output distinguishes CPU time from I/O wait time

3. **Given** the consume hot path (DRR selection → message delivery)
   **When** profiled under load (100 consumers)
   **Then** wall-clock time is measured for: DRR `next_key()`, storage read (`get_message()`), protobuf decode, lease write (2 mutations), ReadyMessage construction, and consumer channel send
   **And** per-message consume cost is quantified in μs

4. **Given** both enqueue and consume profiles
   **When** a mock `StorageEngine` (in-memory, no I/O) is used as baseline
   **Then** the delta between mock and RocksDB isolates pure storage I/O cost from CPU work
   **And** the mock engine is implemented behind the existing `StorageEngine` trait

5. **Given** all profiling data
   **When** an analysis document is written
   **Then** it validates or revises the research's 93μs decomposition model (queue check 5-10μs, clone 1-3μs, protobuf 3-8μs, RocksDB 10-30μs, overhead 30-60μs)
   **And** it provides per-operation cost measurements that serve as the baseline for stories 32.2-32.5
   **And** it identifies any bottlenecks not predicted by the research

6. **Given** the profiling infrastructure
   **When** the story is complete
   **Then** profiling can be re-run after each subsequent story to measure improvement (reproducible command or make target)

## Tasks / Subtasks

- [ ] Task 1: Generate CPU flamegraphs
  - [ ] 1.1 Run `fila-bench` enqueue workload under `cargo-flamegraph` / `samply`
  - [ ] 1.2 Run consume workload under flamegraph profiler
  - [ ] 1.3 Save SVG artifacts to `bench/profiles/`

- [ ] Task 2: Add tracing spans to hot path
  - [ ] 2.1 Instrument `prepare_enqueue()` per-message steps (clone, serialize, key gen, mutation)
  - [ ] 2.2 Instrument `flush_coalesced_enqueues()` RocksDB commit
  - [ ] 2.3 Instrument consume delivery path (DRR select, storage read, lease write)
  - [ ] 2.4 Run under load, collect span timings

- [ ] Task 3: Implement mock StorageEngine
  - [ ] 3.1 Create `InMemoryEngine` implementing `StorageEngine` trait (HashMap-backed)
  - [ ] 3.2 Run enqueue benchmark with mock engine to isolate I/O cost
  - [ ] 3.3 Run consume benchmark with mock engine

- [ ] Task 4: Write analysis document
  - [ ] 4.1 Compare measured breakdown against research estimates
  - [ ] 4.2 Document per-operation costs with confidence ranges
  - [ ] 4.3 Identify the top 3 bottlenecks by measured wall-clock contribution
  - [ ] 4.4 Save to `_bmad-output/planning-artifacts/research/plateau-1-baseline-profiling.md`

## Dev Notes

### Profiling Toolchain

| Tool | Purpose |
|------|---------|
| `cargo-flamegraph` or `samply` | CPU flamegraphs — where is CPU time spent? |
| `tracing` spans with `tracing-timing` | Wall-clock breakdown — includes I/O wait invisible to flamegraphs |
| `InMemoryEngine` mock | Isolate storage I/O from CPU work |
| `fila-bench` | Consistent workload generation |

### Critical Insight

Flamegraphs show CPU time only. Time spent awaiting I/O, locks, or channel receives is invisible. The 93μs/message includes both CPU work and I/O wait. Use BOTH CPU flamegraphs and tracing spans to decompose it accurately.

### Key Files to Instrument

| File | What to measure |
|------|----------------|
| `crates/fila-core/src/broker/scheduler/handlers.rs` | `prepare_enqueue()` per-message steps |
| `crates/fila-core/src/broker/scheduler/mod.rs` | `flush_coalesced_enqueues()` batch commit |
| `crates/fila-core/src/broker/scheduler/delivery.rs` | DRR delivery, `try_deliver_to_consumer()` |
| `crates/fila-core/src/storage/rocksdb.rs` | `apply_mutations()` RocksDB write |

### References

- [Research: technical-kafka-parity-profiling-strategy-2026-03-25.md] — 93μs decomposition model to validate
- [Research: post-tier0-profiling-analysis.md] — Previous profiling baseline
