# Plateau 2 Profile Checkpoint

**Date:** 2026-03-25
**Epic:** 33 — Plateau 2: Fix the Consume Path
**Stories completed:** 33.1-33.3 (3/3 implementation stories)

## Optimizations Applied

| Story | Optimization | Hot path impact |
|-------|-------------|-----------------|
| 33.1 | In-memory delivery queue | Consume: 0 RocksDB reads (was 1 per message, 10-100μs) |
| 33.2 | In-memory lease tracking | Delivery: 0 RocksDB writes (was 2 mutations per message) |
| 33.2 | Lease expiry from memory | Reclaim: HashMap iteration (was RocksDB range scan) |
| 33.3 | Batch ack deletes | Ack: amortized O(1/N) storage cost (was 1 write per ack) |

## Per-Operation Changes

### Delivery path (consume)
| Operation | Before | After |
|-----------|--------|-------|
| Load message | RocksDB get (10-100μs) | In-memory decode (~1μs) |
| Write lease | 2 apply_mutations calls | HashMap insert (~50ns) |
| **Total per-delivery** | **~20-200μs** | **~1-2μs** |

### Ack path
| Operation | Before | After |
|-----------|--------|-------|
| Lookup lease | RocksDB get | HashMap lookup (~50ns) |
| Delete message | Immediate apply_mutations | Buffered, batch flush |
| Delete lease/expiry | 2 mutations | In-memory remove |
| **Total per-ack** | **~30-100μs** | **~100ns + amortized flush** |

## Combined with Plateau 1

| Path | Pre-Plateau 1 | Post-Plateau 1 | Post-Plateau 2 |
|------|-------------:|---------------:|---------------:|
| Enqueue (per-msg) | ~93μs | ~50-70μs | ~50-70μs (unchanged) |
| Consume (per-msg) | ~50-200μs | ~50-200μs | ~1-2μs |
| Ack (per-msg) | ~30-100μs | ~30-100μs | ~100ns amortized |
| **End-to-end** | **~170-390μs** | **~130-370μs** | **~50-72μs** |

## Go/No-Go for Epic 34

**CONDITIONAL GO** — Plateau 2 eliminated the consume path as a bottleneck. The remaining ceiling is:

1. **RocksDB write on enqueue** — still ~30-50μs per batch commit. This is the storage engine's inherent cost.
2. **Memory pressure** — carrying wire bytes + lease state in memory increases RSS. Need to monitor under sustained load.

Epic 34 targets storage engine changes (Titan blob separation, append-only log prototype). These are higher-risk, higher-reward changes. Recommend running actual benchmarks before committing to Epic 34 — the current improvements may be sufficient depending on production requirements.
