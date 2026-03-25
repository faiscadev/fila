# Post-Tier 0 Profiling Analysis

**Date:** 2026-03-25
**Commit:** Stories 30.1-30.4 (batch pipeline Tier 0)
**Hardware:** Development machine (macOS, Apple Silicon)

## Summary

Tier 0 (Stories 30.1-30.4) unified the API surface and eliminated the per-message gRPC→scheduler round-trip. Benchmark results show a **~3.1x throughput improvement** for single-producer 1KB enqueue.

## Benchmark Results

### Single-producer enqueue throughput (1KB)

| Metric | Pre-Tier 0 | Post-Tier 0 | Change |
|--------|----------:|------------:|-------:|
| Enqueue throughput (1KB) | 3,478 msg/s | 10,785 msg/s | +3.1x |
| Enqueue throughput (MB/s) | 3.40 MB/s | 10.53 MB/s | +3.1x |

### End-to-end latency

| Level | Producers | p50 | p99 |
|-------|----------:|----:|----:|
| Light load | 1 | 0.33 ms | 0.49 ms |
| Multi-producer | 4 | 0.37 ms | 0.81 ms |

Light-load latency is well under the 1ms NFR-B1 target.

### Fairness & scheduling

- Key cardinality 10: 3,155 msg/s
- Key cardinality 1K: 1,354 msg/s
- Key cardinality 10K: 708 msg/s
- Equal-weight fairness: 0% deviation, Jain's index 1.0

### Consumer concurrency

- 1 consumer: 401 msg/s
- 10 consumers: 2,480 msg/s
- 100 consumers: 2,500 msg/s

## Where Time Is Now Spent

The single-producer benchmark (10.8K msg/s) is dominated by:

1. **RocksDB writes** — Each enqueue requires at least one RocksDB write (message storage). At 10.8K msg/s, this is ~93μs per message. RocksDB write coalescing (Epic 23) already batches mutations, but the write amplification and fsync overhead remain.

2. **Protobuf serialization** — Each message is serialized to protobuf before storage. Zero-copy passthrough (Epic 24) reduced this for the hot path but the storage encoding remains.

3. **gRPC/HTTP2 overhead** — Now amortized across the batch (not per-message). The batch pipeline fix removed this as the primary bottleneck.

4. **Lua hooks** — When configured, Lua execution adds ~300μs per message (visible in lua_throughput_with_hook: 1,300 msg/s vs 10,785 msg/s without).

## Is gRPC Protocol Overhead the New Bottleneck?

**No.** After Tier 0, the per-message gRPC overhead is amortized across the batch. The new bottleneck is RocksDB write throughput — specifically, the synchronous `apply_mutations` call that commits the write batch.

The `StreamEnqueue` batch-within-stream optimization (Story 30.6) would reduce HTTP/2 framing overhead but would not address the RocksDB bottleneck. At 10.8K msg/s, the storage layer is the constraint, not the transport layer.

## Go/No-Go Recommendation for Stories 30.6-30.8

**Recommendation: NO-GO for Stories 30.6-30.8.**

Rationale:
- The gRPC protocol overhead is no longer the bottleneck — RocksDB write throughput is.
- Story 30.6 (batch-within-stream) would optimize transport, but the gain would be marginal since transport is no longer the constraint.
- Story 30.7 (SDK adaptive accumulator) already exists via `AccumulatorMode::Auto` — the Nagle-style algorithm was implemented in Epics 23/26.
- Story 30.8 (final profile checkpoint) is moot if 30.6-30.7 are not implemented.

The next meaningful optimization target is the storage layer (Epic 25: Purpose-Built Storage Engine, currently deferred) or further RocksDB tuning.

## Updated Benchmark Comparison

| System | 1KB Throughput | Ratio to Fila |
|--------|---------------:|---------------|
| Kafka (batched) | ~400,000 msg/s | 37x |
| Fila (post-Tier 0) | 10,785 msg/s | 1x |
| Fila (pre-Tier 0) | 3,478 msg/s | 0.32x |

The remaining gap to Kafka is architectural — Kafka's append-only log with page cache vs Fila's B-tree storage with per-message indexing. Closing this gap requires the purpose-built storage engine (Epic 25), not transport optimizations.
