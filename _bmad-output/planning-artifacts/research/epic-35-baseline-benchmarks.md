# Epic 35 Baseline Benchmarks — Post-Plateau State

**Date:** 2026-03-25
**Commit:** 57871ae (feat/35.1-benchmark-baseline, based on main HEAD)
**Purpose:** Verified starting point for Phase 1 gRPC tuning. Quantifies cumulative impact of Epics 32-34.

---

## Benchmark Results

### Single-Producer Enqueue (1KB)

| Storage Engine | Run 1 (msg/s) | Run 2 (msg/s) | Average (msg/s) |
|---------------|---------------|---------------|-----------------|
| RocksDB | 10,149 | 10,674 | **10,412** |
| InMemoryEngine | 11,139 | 11,277 | **11,208** |

### 4-Producer Enqueue (1KB, 15s runs)

| Storage Engine | Run 1 (msg/s) | Run 2 (msg/s) | Average (msg/s) |
|---------------|---------------|---------------|-----------------|
| RocksDB | 23,462 | 23,768 | **23,615** |
| InMemoryEngine | 24,985 | — | **~24,985** |

### Lifecycle: Enqueue + Consume + Ack (1KB, 15s runs)

| Storage Engine | Run 1 (msg/s) | Run 2 (msg/s) | Average (msg/s) |
|---------------|---------------|---------------|-----------------|
| RocksDB | 6,431 | 6,510 | **6,471** |
| InMemoryEngine | 7,163 | — | **~7,163** |

### Batch Enqueue with SDK Accumulator (1KB, RocksDB, 15s runs)

| Run 1 (msg/s) | Run 2 (msg/s) | Average (msg/s) |
|---------------|---------------|-----------------|
| 11,064 | 11,094 | **11,079** |

### Batch Scaling (from bench suite, batch size → throughput)

| Batch Size | Throughput (msg/s) |
|------------|-------------------|
| 1 | 321 |
| 10 | 3,182 |
| 50 | 15,123 |
| 100 | 28,490 |
| 250 | 67,245 |
| 500 | 114,071 |
| 1,000 | 183,301 |

### Multi-Producer Batch Scaling

| Producers | Throughput (msg/s) |
|-----------|-------------------|
| 1 | 11,967 |
| 5 | 52,000 |
| 10 | 94,000 |
| 50 | 222,567 |

---

## Comparison vs Pre-Plateau Baseline

| Scenario | Pre-Plateau (msg/s) | Post-Plateau (msg/s) | Improvement |
|----------|--------------------|--------------------|-------------|
| Single-producer enqueue (RocksDB) | 8,264 | 10,412 | **+26.0%** |
| Single-producer enqueue (InMemory) | — | 11,208 | — |
| 4-producer enqueue (RocksDB) | — | 23,615 | — |
| Lifecycle enqueue+consume+ack | — | 6,471 | — |
| Batch enqueue (SDK accumulator) | — | 11,079 | — |

The 8,264 msg/s baseline was from Epic 30 profiling. The +26% cumulative gain comes from Epics 32-34 (RocksDB tuning, string interning, hybrid envelope, arena allocation, Titan blob separation, multi-shard default).

---

## Flamegraph Analysis — InMemoryEngine Single-Producer Enqueue

Flamegraph captured via `cargo flamegraph` (DTrace on macOS, 15s run).

### CPU Distribution (approximate, from SVG sample widths)

| Component | % of CPU | Notes |
|-----------|---------|-------|
| h2 (HTTP/2) + hyper | ~53% | `h2::client::Connection` (35%), `h2::proto::streams::send` (16%), frame encoding |
| fila_sdk client | ~22% | `FilaClient::enqueue`, protobuf serialization, tonic call dispatch |
| tokio runtime | ~14% | Task scheduling, I/O driver, waker management |
| Application logic | ~11% | Server-side enqueue processing (not fully visible in client-side profile) |

**Key finding:** HTTP/2 transport (h2/hyper) dominates the client-side profile at ~53% of CPU. This is consistent with the server-side profiling from the custom transport research doc (62% server-side HTTP/2). The gRPC/HTTP/2 overhead is the primary bottleneck for single-connection throughput.

### Implications for Phase 1

1. **SDK streaming batch consolidation (35.2)** — Sending N messages per StreamEnqueueRequest instead of 1 should reduce HTTP/2 frame overhead proportionally. Currently each enqueue creates a separate gRPC stream frame.
2. **gRPC transport tuning (35.3)** — Increasing HTTP/2 window sizes and frame sizes can reduce flow control overhead. But the fundamental per-frame overhead of HTTP/2 (HPACK encode/decode, stream state machine) limits gains.
3. **Ceiling estimate** — If transport is ~53% of client CPU, eliminating it entirely yields ~2.1x theoretical max. Realistic Phase 1 gains: 30-80% improvement on single-producer, more on batch workloads.

---

## Other Notable Metrics

| Metric | Value |
|--------|-------|
| e2e latency p50 (light) | 0.12-0.13 ms |
| e2e latency p99 (light) | 0.21 ms |
| Fairness overhead (DRR vs FIFO) | 2.81% |
| Lua hook overhead | 0.00 us (negligible) |
| Memory RSS idle | 655 MB |
| Memory per-message overhead | 7,471 bytes/msg |
| Compaction p99 delta | 0.29 ms |
| Equal-weight Jain's index | 1.00 (perfect) |

---

## Raw Data

Full benchmark report saved to `bench-results.json` at commit 57871ae.
Flamegraph SVG: `target/flamegraphs/enqueue-only-20260325-235723.svg`
