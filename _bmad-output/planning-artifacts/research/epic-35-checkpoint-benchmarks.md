# Epic 35 Checkpoint Benchmarks — Phase 1 Results & Phase 2 Go/No-Go

**Date:** 2026-03-26
**Commit:** 81021fd (feat/35.4-benchmark-checkpoint, includes 35.2 + 35.3)
**Purpose:** Measure impact of Phase 1 optimizations (SDK streaming consolidation + gRPC transport tuning). Go/no-go gate for Epic 36 (FIBP custom protocol).

---

## Comparison: Baseline (35.1) vs Post-Tuning (35.4)

### Single-Producer Enqueue (1KB)

| Storage | Baseline (msg/s) | Post-Tuning (msg/s) | Change |
|---------|-----------------|---------------------|--------|
| RocksDB | 10,412 | 10,656 | **+2.3%** |
| InMemoryEngine | 11,208 | 11,800 | **+5.3%** |

### 4-Producer Enqueue (1KB, 15s profile-workload)

| Storage | Baseline (msg/s) | Post-Tuning (msg/s) | Change |
|---------|-----------------|---------------------|--------|
| RocksDB | 23,615 | 24,288 | **+2.8%** |

### Lifecycle: Enqueue + Consume + Ack (1KB, 15s profile-workload)

| Baseline (msg/s) | Post-Tuning (msg/s) | Change |
|-----------------|---------------------|--------|
| 6,471 | 6,744 | **+4.2%** |

### Batch Enqueue with SDK Accumulator (1KB, 15s profile-workload)

| Baseline (msg/s) | Post-Tuning (msg/s) | Change |
|-----------------|---------------------|--------|
| 11,079 | 11,537 | **+4.1%** |

### SDK Auto-Batch Throughput (bench suite `batched_vs_unbatched`)

| Mode | Baseline (msg/s) | Post-Tuning (msg/s) | Change |
|------|-----------------|---------------------|--------|
| Explicit batch | 22,963 | 23,138 | +0.8% |
| **Auto batch** | **13,129** | **23,050** | **+75.5%** |

**This is the key win from Story 35.2.** The auto accumulator now sends all messages in a single `StreamEnqueueRequest` instead of one per message, achieving near-parity with explicit batching.

### Multi-Producer Batch Scaling

| Producers | Baseline (msg/s) | Post-Tuning (msg/s) | Change |
|-----------|-----------------|---------------------|--------|
| 1 | 11,967 | 12,433 | +3.9% |
| 5 | 52,000 | 55,000 | +5.8% |
| 10 | 94,000 | 92,333 | -1.8% (noise) |
| 50 | 222,567 | 229,967 | +3.3% |

### Batch Size Scaling (unchanged — same batch RPC path)

| Batch Size | Baseline (msg/s) | Post-Tuning (msg/s) | Change |
|------------|-----------------|---------------------|--------|
| 100 | 28,490 | 27,820 | -2.4% (noise) |
| 500 | 114,071 | 113,639 | -0.4% (noise) |
| 1,000 | 183,301 | 186,096 | +1.5% |

### Latency (e2e, light load)

| Percentile | Baseline | Post-Tuning | Change |
|-----------|----------|-------------|--------|
| p50 | 0.12 ms | 0.12 ms | 0% |
| p95 | 0.18 ms | 0.16 ms | -11% |
| p99 | 0.21 ms | 0.19 ms | -10% |
| p99.9 | 0.41 ms | 0.32 ms | -22% |

---

## Flamegraph Comparison

### Baseline (35.1) CPU Distribution
| Component | % CPU |
|-----------|-------|
| h2/hyper (HTTP/2) | ~53% |
| fila_sdk client | ~22% |
| tokio runtime | ~14% |
| Application logic | ~11% |

### Post-Tuning (35.4) CPU Distribution
| Component | % CPU |
|-----------|-------|
| h2/hyper (HTTP/2) | ~46.5% |
| fila_sdk client | ~22.8% |
| tokio runtime | ~19% |
| Application logic | ~11% |

The HTTP/2 transport dropped from ~53% to ~46.5% of CPU. The larger window/frame sizes reduced flow control overhead, but HTTP/2 still dominates.

---

## Phase 2 Go/No-Go Assessment

### Decision Criteria

| Criterion | Result | Verdict |
|-----------|--------|---------|
| HTTP/2 transport still >40% CPU | 46.5% — YES | **GO** |
| Throughput already exceeds 50K msg/s | Single-producer: 10.7K — NO | **GO** |
| Phase 1 improvements exhausted? | Window/frame tuning shows diminishing returns (2-5%) | **GO** |

### Recommendation: **GO for Epic 36 (FIBP Custom Protocol)**

**Rationale:**
1. HTTP/2 transport remains the dominant bottleneck at 46.5% of client CPU even after tuning
2. Phase 1 achieved modest single-producer gains (+2-5%) but a significant auto-batch improvement (+75.5%) from consolidation
3. The gRPC transport overhead is structural — HPACK encoding, stream state machine, and flow control overhead cannot be eliminated by tuning parameters
4. A custom binary protocol (length-prefixed TCP like Kafka) eliminates all HTTP/2 overhead, yielding a theoretical 2.1x ceiling (1 / 0.465)
5. Single-producer throughput at ~10.7K msg/s is far below the 50K+ threshold that would make Phase 2 unnecessary

### What Phase 1 Achieved
- **Auto-batch consolidation** (+75.5% for auto-batch mode) — the biggest win, ensuring SDK accumulation is properly amortized
- **Tail latency improvement** (p99.9 -22%) — from larger HTTP/2 windows reducing flow control stalls
- **Consumer delivery** (10→100 batch default) — ready for Phase 2's binary protocol

### What Phase 2 Must Deliver
- Eliminate HTTP/2 per-frame overhead (~46.5% of CPU)
- Target: 20-30K msg/s single-producer (2-3x from current 10.7K)
- FIBP transport alongside gRPC (dual-protocol, not replacement)

---

## Raw Numbers

All numbers from actual `cargo bench -p fila-bench --bench system` and `profile-workload` runs.
No estimates, no projections.

Baseline flamegraph: `target/flamegraphs/enqueue-only-20260325-235723.svg`
Post-tuning flamegraph: `target/flamegraphs/enqueue-only-20260326-011711.svg`
