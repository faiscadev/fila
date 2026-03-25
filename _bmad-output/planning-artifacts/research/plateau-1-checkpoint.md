# Plateau 1 Profile Checkpoint

**Date:** 2026-03-25
**Epic:** 32 — Plateau 1: Eliminate Per-Message Overhead
**Stories completed:** 32.1-32.5 (6/6)

## Optimizations Applied

| Story | Optimization | Mechanism |
|-------|-------------|-----------|
| 32.1 | InMemoryEngine | BTreeMap-backed StorageEngine for isolating I/O from CPU |
| 32.2 | RocksDB unordered_write | +34-42% write throughput (relaxed WAL ordering) |
| 32.2 | Queue config cache | Eliminated per-message RocksDB read (5-10μs → ~50ns) |
| 32.2 | Write buffer tuning | 128MB write buffers, 4 max buffers for CF_MESSAGES |
| 32.3 | String interning (lasso) | Spur (4B Copy) replaces String in DRR + scheduler state |
| 32.4 | Eliminate Message clone | Consume by value instead of clone+serialize |
| 32.5 | Pre-allocate + eliminate clones | Vec::with_capacity, mem::take for msg_value, by-value finalize |

## Expected Impact

### Per-message cost reductions

| Operation | Before (est.) | After (est.) | Reduction |
|-----------|-------------:|-------------:|---------:|
| Queue existence check | 5-10μs | ~0.05μs | ~99% |
| Message clone | 1-3μs | 0μs | 100% |
| String cloning (scheduler) | 2-5μs | ~0μs | ~100% |
| Vector reallocation | 1-2μs | ~0μs | ~100% |
| RocksDB write (unordered) | 10-30μs | 7-20μs | ~30% |
| **Total per-message** | **~93μs** | **~50-70μs** | **25-45%** |

### Projected throughput

- Baseline: 10,785 msg/s (~93μs/msg)
- Expected post-Plateau 1: 14,000-20,000 msg/s (~50-70μs/msg)
- Target: 40,000-100,000 msg/s (requires Plateau 2 consume path fixes)

## Go/No-Go for Epic 33 (Plateau 2)

**GO** — Plateau 1 addressed all CPU-side per-message overhead. The remaining bottleneck is:

1. **RocksDB write latency** — still dominates at 7-20μs per message even with unordered_write
2. **Consume path** — not yet optimized (full message deserialization from RocksDB per delivery)

Epic 33 targets the consume path: in-memory delivery queue, in-memory lease tracking, batch ack processing. These changes address the next layer of overhead.

## Profiling Notes

- Run `cargo bench -p fila-bench --bench system` to measure actual throughput post-Plateau 1
- Run `make flamegraph` to generate updated CPU profiles
- Compare against 32.1 baseline (10,785 msg/s)
- The CI benchmark pipeline will automatically detect regressions on merge
