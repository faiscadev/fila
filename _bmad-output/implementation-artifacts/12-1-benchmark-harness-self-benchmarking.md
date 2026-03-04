# Story 12.1: Benchmark Harness & Self-Benchmarking

Status: ready-for-dev

## Story

As a developer,
I want a comprehensive benchmark suite that measures Fila's single-node performance across all critical dimensions,
so that we have a quantified baseline for optimization and can validate Phase 1 NFR targets.

## Acceptance Criteria

1. **Given** the Fila server is running, **when** the benchmark suite executes, **then** it measures single-node enqueue throughput (msg/s) for 1KB payload messages.

2. **Given** the benchmark suite is running, **when** latency benchmarks execute, **then** it measures enqueue-to-consume latency at p50/p95/p99 under varying load levels (light: 1 producer, moderate: 5 producers, saturated: 20 producers).

3. **Given** the benchmark suite is running, **when** fairness overhead benchmarks execute, **then** it measures throughput with fair scheduling enabled vs raw FIFO to validate NFR2 (<5% overhead).

4. **Given** the benchmark suite is running, **when** fairness accuracy benchmarks execute, **then** it measures fairness accuracy under sustained load across 5+ keys with varying weights to validate NFR3 (within 5%).

5. **Given** the benchmark suite is running, **when** Lua benchmarks execute, **then** it measures Lua `on_enqueue` execution latency at p99 to validate NFR4 (<50us).

6. **Given** the benchmark suite is running, **when** queue depth scaling benchmarks execute, **then** it measures enqueue/consume throughput at 1M and 10M queued messages.

7. **Given** the benchmark suite is running, **when** cardinality benchmarks execute, **then** it measures fairness key cardinality impact: 10, 1K, 10K, and 100K active keys.

8. **Given** the benchmark suite is running, **when** concurrency benchmarks execute, **then** it measures consumer concurrency impact: 1, 10, and 100 simultaneous consumers.

9. **Given** the benchmark suite is running, **when** memory benchmarks execute, **then** it measures memory footprint under load (RSS).

10. **Given** the benchmark suite is running, **when** compaction benchmarks execute, **then** it measures RocksDB compaction impact on tail latency (p99 during active compaction vs idle).

11. **Given** the benchmark suite completes, **when** results are collected, **then** benchmark results are output in machine-readable format (JSON) for CI consumption.

12. **Given** the benchmark crate exists, **when** added to the workspace, **then** the `fila-bench` crate is a member of the Cargo workspace.

13. **Given** the crate is in the workspace, **when** `cargo bench -p fila-bench` is run, **then** the full benchmark suite executes.

14. **Given** CI needs to include the crate, **when** the CI pipeline runs, **then** the `fila-bench` crate is included in CI build + clippy checks.

## Tasks / Subtasks

- [ ] Task 1: Create `fila-bench` crate scaffold (AC: 12, 13)
  - [ ] Create `crates/fila-bench/Cargo.toml` with workspace membership
  - [ ] Add `fila-bench` to root `Cargo.toml` workspace members
  - [ ] Create `src/lib.rs` with shared benchmark infrastructure
  - [ ] Create single `[[bench]]` entry with `harness = false`

- [ ] Task 2: Build benchmark server harness (AC: 1)
  - [ ] Create `BenchServer` struct reusing TestServer pattern from fila-e2e
  - [ ] Implement server lifecycle (spawn, wait-ready, teardown)
  - [ ] Implement `BenchClient` wrapping fila-sdk `FilaClient`
  - [ ] Create queue setup helpers (plain FIFO, with Lua, with fairness keys)

- [ ] Task 3: Build measurement + reporting infrastructure (AC: 11)
  - [ ] Create `BenchResult` struct (metric name, value, unit, metadata)
  - [ ] Create `BenchReport` struct collecting all results
  - [ ] Implement JSON serialization with serde
  - [ ] Implement human-readable stdout summary + JSON file output
  - [ ] Create timing utilities (percentile calculation from sorted samples)

- [ ] Task 4: Implement throughput benchmark (AC: 1)
  - [ ] Single-producer enqueue throughput with 1KB payloads
  - [ ] Measure over sustained window (e.g., 30s) for stable results
  - [ ] Report msg/s and MB/s

- [ ] Task 5: Implement latency benchmark (AC: 2)
  - [ ] Implement enqueue-then-consume round-trip latency measurement
  - [ ] Light load (1 producer), moderate (5 producers), saturated (20 producers)
  - [ ] Calculate p50/p95/p99 from collected samples
  - [ ] Use tokio tasks for concurrent producers

- [ ] Task 6: Implement fairness overhead benchmark (AC: 3)
  - [ ] Baseline: FIFO queue (no Lua, single fairness key)
  - [ ] Test: queue with Lua on_enqueue assigning fairness_key from header
  - [ ] Compare throughput, report overhead percentage
  - [ ] Validate NFR2 (<5% overhead)

- [ ] Task 7: Implement fairness accuracy benchmark (AC: 4)
  - [ ] Enqueue messages across 5+ keys with varying weights (e.g., 1:2:3:4:5)
  - [ ] Consume all messages, track per-key delivery count
  - [ ] Compare actual delivery ratio vs expected fair share
  - [ ] Validate NFR3 (within 5% of fair share)

- [ ] Task 8: Implement Lua latency benchmark (AC: 5)
  - [ ] Enqueue messages through a queue with on_enqueue Lua hook
  - [ ] Measure per-message Lua execution time (via enqueue latency delta vs no-Lua baseline)
  - [ ] Calculate p99 execution latency
  - [ ] Validate NFR4 (<50us p99)

- [ ] Task 9: Implement queue depth scaling benchmark (AC: 6)
  - [ ] Pre-load 1M messages, then measure enqueue+consume throughput
  - [ ] Pre-load 10M messages, then measure enqueue+consume throughput
  - [ ] Compare against baseline (empty queue) to detect degradation

- [ ] Task 10: Implement fairness key cardinality benchmark (AC: 7)
  - [ ] Test with 10, 1K, 10K, 100K distinct fairness keys
  - [ ] Measure scheduling throughput at each cardinality level
  - [ ] Report throughput degradation curve

- [ ] Task 11: Implement consumer concurrency benchmark (AC: 8)
  - [ ] Spawn 1, 10, 100 concurrent consume streams
  - [ ] Measure aggregate consume throughput at each level
  - [ ] Report scaling efficiency

- [ ] Task 12: Implement memory footprint benchmark (AC: 9)
  - [ ] Measure RSS before load, during load, after load
  - [ ] Use `/proc/self/status` (Linux) or `mach_task_info` (macOS) for RSS
  - [ ] Alternatively use `sysinfo` crate for cross-platform RSS

- [ ] Task 13: Implement RocksDB compaction impact benchmark (AC: 10)
  - [ ] Measure p99 latency during idle (no compaction)
  - [ ] Force compaction via large write + delete cycle
  - [ ] Measure p99 latency during active compaction
  - [ ] Report delta

- [ ] Task 14: Update CI pipeline (AC: 14)
  - [ ] Add `fila-bench` to CI clippy + build steps
  - [ ] Keep bench execution as `continue-on-error` (matches existing pattern)

## Dev Notes

### Architecture & Design Decisions

**Benchmark type: System-level (blackbox), not micro-benchmark.**
The existing `fila-core/benches/drr.rs` and `throttle.rs` are micro-benchmarks testing internal data structures. Story 12.1 benchmarks are system-level: spawn a real server, use the SDK, measure end-to-end behavior. This is a blackbox benchmark suite — no internal imports from fila-core.

**Single `[[bench]]` binary running full suite.**
Use `harness = false` with one bench file (`benches/system.rs`) that calls into `src/lib.rs` functions. This allows `cargo bench -p fila-bench` to run everything and produce a unified JSON report. Individual benchmark modules in `src/benchmarks/`.

**Reuse TestServer pattern, don't import it.**
The `fila-e2e` crate's `TestServer` is perfect — spawn binary, poll TCP, kill on drop. Copy the pattern into `fila-bench` as `BenchServer` with benchmark-specific additions (e.g., configurable warmup). Do NOT add a dependency on `fila-e2e` (it has no library target, just tests).

### Project Structure

```
crates/fila-bench/
├── Cargo.toml
├── benches/
│   └── system.rs           # [[bench]] entry point, calls lib functions
└── src/
    ├── lib.rs              # Public API: run_all_benchmarks(), BenchReport
    ├── server.rs           # BenchServer (spawn + lifecycle)
    ├── measurement.rs      # Timing, percentiles, sample collection
    ├── report.rs           # BenchResult, BenchReport, JSON serialization
    └── benchmarks/
        ├── mod.rs
        ├── throughput.rs   # AC 1: enqueue msg/s
        ├── latency.rs      # AC 2: e2e latency p50/p95/p99
        ├── fairness.rs     # AC 3-4: overhead + accuracy
        ├── lua.rs          # AC 5: Lua p99
        ├── scaling.rs      # AC 6-8: depth, cardinality, concurrency
        ├── memory.rs       # AC 9: RSS
        └── compaction.rs   # AC 10: compaction impact
```

### Key Dependencies

```toml
[dependencies]
fila-sdk = { workspace = true }             # Client for blackbox benchmarking
fila-proto = { workspace = true }           # Admin client for queue creation
tokio = { workspace = true }                # Async runtime
serde = { workspace = true }                # JSON serialization
serde_json = { workspace = true }           # JSON output
tonic = { workspace = true }                # gRPC admin client
tempfile = "3"                              # Temp dirs for server data
```

No `fila-core` or `fila-server` dependency — this is a blackbox suite.

### Measurement Approach

**Throughput**: Enqueue N messages in a tight loop, measure wall time. `msg/s = N / elapsed_secs`. Use warmup period (discard first 5s of measurements).

**Latency**: Enqueue one message, immediately consume it, measure round-trip. Collect many samples. Sort for percentiles. Use `tokio::time::Instant` for high-resolution timing.

**Fairness overhead**: Run identical workload (same message count, same payload) on FIFO queue vs fair-scheduling queue. Compare throughput. `overhead_pct = (1 - fair_throughput/fifo_throughput) * 100`.

**Memory (RSS)**: On macOS use `mach_task_basic_info` via `libc` FFI or the `sysinfo` crate. On Linux read `/proc/self/status` VmRSS. Cross-platform: use `sysinfo` crate.

**RocksDB compaction**: Enqueue and ack a large batch to create dead entries, then enqueue new messages and measure latency while compaction is active. RocksDB compaction is triggered automatically.

### Critical Patterns from Existing Codebase

**Server binary resolution** (from `fila-e2e/tests/helpers/mod.rs:312-320`):
```rust
fn workspace_binary(name: &str) -> PathBuf {
    let mut path = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    path.pop(); // crates/
    path.pop(); // workspace root
    path.push("target");
    path.push("debug");
    path.push(name);
    path
}
```

**Queue creation with Lua** (from e2e helpers):
Use the admin gRPC client (`fila_proto::fila::v1::fila_admin_client::FilaAdminClient`) to create queues with on_enqueue scripts. Pattern in `fila-e2e/tests/helpers/mod.rs:254-286`.

**SDK consume stream**: `client.consume(queue)` returns `impl Stream<Item = Result<ConsumeMessage, StatusError>>`. Use `tokio_stream::StreamExt::next()` to receive messages.

### CI Integration

The existing `ci.yml` runs `cargo bench` with `continue-on-error: true` (line in CI). Adding `fila-bench` to workspace members is sufficient — `cargo bench` already runs all workspace benches. Verify:
- `cargo clippy -p fila-bench` passes
- `cargo build -p fila-bench` compiles
- `cargo bench -p fila-bench` executes (even if results vary on CI hardware)

### JSON Output Format

```json
{
  "version": "1.0",
  "timestamp": "2026-03-04T12:00:00Z",
  "commit": "abc1234",
  "benchmarks": [
    {
      "name": "enqueue_throughput_1kb",
      "value": 125000.0,
      "unit": "msg/s",
      "metadata": { "payload_size": 1024, "duration_secs": 30 }
    },
    {
      "name": "e2e_latency_p99_light",
      "value": 0.85,
      "unit": "ms",
      "metadata": { "producers": 1, "samples": 10000 }
    }
  ]
}
```

This format is consumed by Story 12.2 (CI regression detection) for baseline comparison.

### What NOT To Do

- Do NOT import from `fila-core` or `fila-server` — this is blackbox
- Do NOT use `criterion` for system benchmarks — criterion is for micro-benchmarks; system benchmarks need custom harness with server lifecycle
- Do NOT hardcode ports — use `free_port()` pattern from e2e helpers
- Do NOT skip warmup — first few seconds of measurements are noisy
- Do NOT run benchmarks in parallel against the same server — single-threaded scheduler means results would be meaningless
- Do NOT add a lib target to fila-e2e to reuse TestServer — copy the pattern instead

### References

- [Source: crates/fila-core/benches/drr.rs] — Existing DRR micro-benchmark (criterion pattern)
- [Source: crates/fila-core/benches/throttle.rs] — Existing throttle micro-benchmark
- [Source: crates/fila-e2e/tests/helpers/mod.rs] — TestServer spawn/lifecycle pattern
- [Source: crates/fila-sdk/src/client.rs] — SDK client API
- [Source: Cargo.toml] — Workspace structure and dependencies
- [Source: .github/workflows/ci.yml] — CI benchmark integration
- [Source: _bmad-output/planning-artifacts/prd.md#NFR1-NFR7] — Performance NFR targets

## Dev Agent Record

### Agent Model Used

### Debug Log References

### Completion Notes List

### File List
