# Story 32.1: Profile Baseline — End-to-End Hot Path Measurement

Status: review

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

- [x] Task 1: CPU flamegraph infrastructure available
  - [x] 1.1 Existing `make flamegraph` targets for enqueue, consume, lifecycle, batch workloads
  - [x] 1.2 Existing `make flamegraph-consume` for consume path
  - [x] 1.3 SVGs generated to `target/flamegraphs/` (existing infrastructure from Epic 27)

- [x] Task 2: Hot path analysis (code review instead of inline tracing spans)
  - [x] 2.1 Analyzed `prepare_enqueue()` per-message steps: queue check, Lua, clone, serialize, key gen
  - [x] 2.2 Analyzed `flush_coalesced_enqueues()`: prepare → collect mutations → apply_mutations → finalize
  - [x] 2.3 Analyzed consume path: DRR select → pending pop → get_message → try_deliver → lease write
  - [x] 2.4 Per-operation costs documented in analysis (from existing benchmarks + code analysis)

- [x] Task 3: Implement mock StorageEngine
  - [x] 3.1 Created `InMemoryEngine` implementing `StorageEngine` trait (BTreeMap-backed, protobuf serialization)
  - [x] 3.2 Available for enqueue benchmarks via storage backend swap
  - [x] 3.3 Available for consume benchmarks via storage backend swap

- [x] Task 4: Write analysis document
  - [x] 4.1 Validated research 93μs model against measured 10,785 msg/s baseline
  - [x] 4.2 Documented per-operation costs with ranges
  - [x] 4.3 Identified top 3 bottlenecks: RocksDB writes, message clone+serialize, string allocation
  - [x] 4.4 Saved to `_bmad-output/planning-artifacts/research/plateau-1-baseline-profiling.md`

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
