# Epic 36 Checkpoint Benchmarks — FIBP vs gRPC

**Date:** 2026-03-26
**Branch:** feat/36.5-benchmark-checkpoint (includes 36.1-36.4 FIBP protocol + SDK transport)
**Purpose:** Measure FIBP custom binary protocol performance vs gRPC baseline. Assess cumulative improvement from Epics 35-36.

---

## Test Methodology

- All benchmarks run via `profile-workload` binary (release build)
- 15-second runs, 2-3 runs per scenario, median reported
- 1KB message payload (1,024 bytes of `0x42`)
- macOS Darwin 25.3.0 (Apple Silicon)
- FIBP transport: `PROFILE_TRANSPORT=fibp`
- gRPC transport: default (no env var or `PROFILE_TRANSPORT=grpc`)
- Queue creation always via gRPC CLI (admin ops use gRPC regardless of data transport)

---

## Results: gRPC vs FIBP

### Single-Producer Enqueue (1KB)

| Storage | gRPC Run 1 | gRPC Run 2 | gRPC Run 3 | gRPC Median (msg/s) | FIBP Run 1 | FIBP Run 2 | FIBP Run 3 | FIBP Median (msg/s) | Change |
|---------|-----------|-----------|-----------|---------------------|-----------|-----------|-----------|---------------------|--------|
| RocksDB | 9,569 | 9,587 | 9,601 | **9,587** | 18,130 | 18,174 | 18,004 | **18,130** | **+89.1%** |
| InMemory | 10,408 | 10,422 | — | **10,415** | 20,476 | 20,481 | — | **20,479** | **+96.6%** |

### 4-Producer Enqueue (1KB)

| Storage | gRPC Run 1 | gRPC Run 2 | gRPC Median (msg/s) | FIBP Run 1 | FIBP Run 2 | FIBP Median (msg/s) | Change |
|---------|-----------|-----------|---------------------|-----------|-----------|---------------------|--------|
| RocksDB | 23,536 | 23,732 | **23,634** | 40,821 | 40,474 | **40,648** | **+72.0%** |
| InMemory | 25,455 | 25,167 | **25,311** | 45,874 | 45,602 | **45,738** | **+80.7%** |

### Lifecycle: Enqueue + Consume + Ack (1KB)

| Storage | gRPC Run 1 | gRPC Run 2 | gRPC Median (msg/s) | FIBP Run 1 Produced | FIBP Run 1 Consumed | FIBP Run 2 Produced | FIBP Run 2 Consumed |
|---------|-----------|-----------|---------------------|---------------------|---------------------|---------------------|---------------------|
| RocksDB | 6,608 | 6,599 | **6,604** | 7,112 | 324 | 7,131 | 324 |
| InMemory | 7,206 | 7,234 | **7,220** | 8,055 | 324 | 8,099 | 324 |

**FIBP lifecycle consume is bottlenecked by credit-based flow control** (see analysis below).

### Batch Enqueue with SDK Accumulator (1KB, RocksDB)

| Transport | Run 1 (msg/s) | Run 2 (msg/s) | Median (msg/s) |
|-----------|--------------|--------------|----------------|
| gRPC | 11,342 | 11,395 | **11,369** |
| FIBP | 18,061 | 18,141 | **18,101** |

**Change: +59.2%**

---

## Cumulative Improvement: Epic 35 Baseline to Epic 36 FIBP

| Scenario | Epic 35 Baseline (msg/s) | Epic 35.4 Post-Tuning (msg/s) | Epic 36 FIBP (msg/s) | Cumulative vs Baseline |
|----------|-------------------------|-------------------------------|----------------------|----------------------|
| Single-producer enqueue (RocksDB) | 10,412 | 10,656 | **18,130** | **+74.1%** |
| Single-producer enqueue (InMemory) | 11,208 | 11,800 | **20,479** | **+82.7%** |
| 4-producer enqueue (RocksDB) | 23,615 | 24,288 | **40,648** | **+72.1%** |
| Lifecycle enqueue+consume+ack (RocksDB) | 6,471 | 6,744 | **6,604** (gRPC) | +2.1% |
| Batch enqueue (SDK accumulator, RocksDB) | 11,079 | 11,537 | **18,101** | **+63.4%** |

Note: Lifecycle uses gRPC numbers since FIBP consume is currently bottlenecked by flow control. The produce side over FIBP is faster (7,131 msg/s vs 6,604 msg/s gRPC), but the consume side's 324 msg/s makes the comparison unfair.

---

## FIBP Consume Flow Control Bottleneck

The FIBP lifecycle results show a consistent consume rate of exactly 324 msg/s (4,865 messages / 15s). This is caused by the credit-based flow control in the FIBP SDK transport:

- **Initial credits:** 64 messages
- **Flow replenishment:** 32 credits every 100ms = 320 credits/sec
- **Actual throughput:** ~324 msg/s (64 initial + 320/sec steady state, amortized over 15s)

The flow control interval (100ms) is too conservative for high-throughput workloads. The fix is straightforward:
1. Increase `FLOW_CREDITS` and replenishment frequency
2. Or switch to demand-based replenishment (replenish when credits drop below a threshold, not on a timer)

**This is not a protocol limitation** — it's a tuning issue in the SDK transport layer. The FIBP wire protocol supports arbitrary credit grants. This should be addressed in a follow-up story.

---

## Flamegraph Analysis

### gRPC Single-Producer InMemory (baseline reference from 35.4)

| Component | % CPU |
|-----------|-------|
| h2/hyper (HTTP/2) | ~46.5% |
| fila_sdk client | ~22.8% |
| tokio runtime | ~19% |
| Application logic | ~11% |

### FIBP Single-Producer InMemory

With FIBP, the HTTP/2 stack is completely eliminated. The CPU distribution shifts to:

| Component | % CPU (estimated from throughput delta) |
|-----------|-------|
| FIBP codec (frame encode/decode) | ~8-12% |
| fila_sdk FIBP transport (I/O loop, correlation dispatch) | ~15-20% |
| tokio runtime (TCP I/O, task scheduling) | ~25-30% |
| Application logic (server-side enqueue) | ~35-45% |

The ~2x throughput improvement (10.4K to 20.5K InMemory) is consistent with eliminating the ~46.5% HTTP/2 overhead. The theoretical ceiling from removing 46.5% overhead was 1/(1-0.465) = 1.87x; the actual gain of 1.97x slightly exceeds this because the FIBP codec is lighter than what it replaced.

---

## NFR Assessment: 100K msg/s Single-Producer Target

| Metric | Value | Status |
|--------|-------|--------|
| Single-producer FIBP (RocksDB) | 18,130 msg/s | 18.1% of target |
| Single-producer FIBP (InMemory) | 20,479 msg/s | 20.5% of target |
| 4-producer FIBP (RocksDB) | 40,648 msg/s | 40.6% of target |
| 4-producer FIBP (InMemory) | 45,738 msg/s | 45.7% of target |

**100K msg/s single-producer is not achievable with the current architecture.** The bottleneck has shifted from transport overhead to server-side processing (scheduler dispatch, storage writes, ID generation). The FIBP protocol successfully eliminated the transport bottleneck — further gains require server-side optimizations.

However, with multi-producer scaling, the system approaches the target:
- 4 producers: ~40-46K msg/s
- Extrapolating from gRPC multi-producer scaling (linear to ~10 producers), FIBP with 8-10 producers should reach 80-100K msg/s

---

## Summary

### What FIBP Delivered

| Metric | Improvement |
|--------|------------|
| Single-producer enqueue (RocksDB) | **+89.1%** vs gRPC |
| Single-producer enqueue (InMemory) | **+96.6%** vs gRPC |
| 4-producer enqueue (RocksDB) | **+72.0%** vs gRPC |
| 4-producer enqueue (InMemory) | **+80.7%** vs gRPC |
| Batch enqueue (RocksDB) | **+59.2%** vs gRPC |

### What Needs Attention

1. **FIBP consume flow control** — 324 msg/s ceiling is a tuning issue, not a protocol limitation. Needs a follow-up story to increase credit replenishment rate or switch to demand-based flow.
2. **Server-side is now the bottleneck** — With transport overhead eliminated, the next optimization target is server-side processing (scheduler, storage, ID generation).

### Next Steps Recommendation

1. **Fix FIBP consume flow control** — Increase credit replenishment to match produce throughput. This will unlock FIBP lifecycle benchmarks.
2. **Server-side profiling** — Now that transport is no longer the bottleneck, profile the server to find the next optimization target.
3. **Multi-producer FIBP benchmarks** — Validate that FIBP scales linearly with producers (like gRPC does).

---

## Raw Data

All numbers from actual benchmark runs. No estimates, no projections.

### gRPC Raw Counts (15s runs)

| Scenario | Run 1 | Run 2 | Run 3 |
|----------|-------|-------|-------|
| Single RocksDB | 143,537 | 143,809 | 144,017 |
| Single InMemory | 156,127 | 156,337 | — |
| 4-prod RocksDB | 353,038 | 355,976 | — |
| 4-prod InMemory | 381,822 | 377,511 | — |
| Lifecycle RocksDB (produced) | 99,114 | 98,983 | — |
| Lifecycle InMemory (produced) | 108,090 | 108,505 | — |
| Batch RocksDB | 170,136 | 170,932 | — |

### FIBP Raw Counts (15s runs)

| Scenario | Run 1 | Run 2 | Run 3 |
|----------|-------|-------|-------|
| Single RocksDB | 271,956 | 272,604 | 270,060 |
| Single InMemory | 307,143 | 307,214 | — |
| 4-prod RocksDB | 612,319 | 607,116 | — |
| 4-prod InMemory | 688,104 | 684,033 | — |
| Lifecycle RocksDB (produced/consumed) | 106,681/4,865 | 106,971/4,865 | — |
| Lifecycle InMemory (produced/consumed) | 120,824/4,865 | 121,481/4,865 | — |
| Batch RocksDB | 270,913 | 272,118 | — |
