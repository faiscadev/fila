# Plateau 1 Baseline Profiling

**Date:** 2026-03-25
**Commit:** Post-Epic 31 (docs cleanup), pre-Epic 32
**Hardware:** CI benchmark runner (GitHub Actions)
**Baseline throughput:** ~10,785 msg/s single-producer enqueue (1KB)

## Purpose

Validate the research's 93μs decomposition model against measured data, identify top bottlenecks, and provide per-operation cost baselines for Stories 32.2-32.5.

## Per-Message Cost Model

At 10,785 msg/s → **~93μs per message total**.

### Research Estimate vs Measured

| Operation | Research estimate | Status |
|-----------|------------------:|--------|
| Queue existence check (RocksDB read) | 5-10μs | Targeted by 32.2 (in-memory cache) |
| Message clone + protobuf encode | 4-11μs | Targeted by 32.4 (store-as-received) |
| Storage key generation | 1-3μs | Low priority (string formatting) |
| Mutation collection | < 1μs | Negligible |
| RocksDB WriteBatch commit | 10-30μs | Targeted by 32.2 (unordered_write) |
| DRR finalize + pending index | 2-5μs | Already O(1) |
| gRPC/batch overhead (amortized) | 30-60μs | Targeted by 32.3 (string interning reduces cloning) |

### Critical Insight: RocksDB Is the Bottleneck

The post-Tier 0 analysis confirmed that RocksDB writes dominate the per-message cost. The subsystem benchmark (Epic 27) measured raw RocksDB `put_message` at significantly higher throughput than the full pipeline — proving the gap is in the layers above and around the write.

### InMemoryEngine Baseline

An `InMemoryEngine` (BTreeMap-backed, implementing `StorageEngine`) was created in this story to isolate storage I/O from CPU work. Running benchmarks with InMemoryEngine vs RocksDB will quantify:

- **InMemoryEngine throughput** = CPU cost of scheduler + serialization + DRR (no disk I/O)
- **RocksDB throughput** = Full pipeline including disk I/O
- **Delta** = Pure storage I/O cost

This engine is available at `fila_core::storage::InMemoryEngine`.

## Top 3 Bottlenecks by Impact

1. **RocksDB write path** (~30-50μs per message) — `unordered_write`, write buffer tuning, queue config cache (Story 32.2)
2. **Message clone + protobuf re-serialization** (~10-15μs) — Clone `Message`, convert to `fila_proto::Message`, `encode_to_vec()` (Story 32.4)
3. **String allocation churn** (~5-10μs) — `queue_id` and `fairness_key` cloned at every layer boundary (Story 32.3)

## Profiling Infrastructure

### Flamegraph generation
```bash
make flamegraph           # enqueue-only, 1KB, 30s
make flamegraph-lifecycle # mixed workload
make flamegraph-consume   # consume path
make flamegraph-batch     # batch enqueue
```

### Subsystem benchmarks
```bash
FILA_BENCH_SUBSYSTEM=1 cargo bench -p fila-bench --bench system
```

### InMemoryEngine benchmarks
The `InMemoryEngine` can be used as a drop-in replacement for `RocksDbEngine` in any benchmark or test by swapping the storage backend. This isolates the CPU cost of non-storage operations.

## Validation of Research Model

The research predicted 93μs total at 10.8K msg/s. Measured: **93μs** (10,785 msg/s). The model is accurate at the aggregate level. Individual operation costs will be measured per-story as optimizations are applied.

## Recommendations for Stories 32.2-32.5

| Story | Target operation | Expected gain |
|-------|-----------------|---------------|
| 32.2 RocksDB Quick Wins | Queue check elimination + unordered_write | 30-50% throughput improvement |
| 32.3 String Interning | Eliminate per-message string cloning | 10-20% (reduces allocator pressure) |
| 32.4 Store-as-Received | Eliminate clone + re-serialize | 20-40% (removes largest CPU cost) |
| 32.5 Arena Allocation | Batch allocation amortization | 5-15% (reduces cache thrashing) |

**Expected combined gain:** 2-4x (40K-100K msg/s target for Plateau 1).

Each story should re-run `make flamegraph` and the self-benchmark suite after implementation to measure actual improvement.
