# Epic 36 Retrospective: Phase 2 — Custom Binary Protocol (FIBP)

**Date:** 2026-03-26
**Stories:** 5/5 completed (100%)
**PRs:** #134-#138 — all merged

---

## What Went Well

1. **FIBP delivered massive enqueue improvement** — Single-producer enqueue nearly doubled (+89-97% over gRPC). 4-producer improved +72-81%. HTTP/2 overhead was successfully eliminated.

2. **Clean sequential execution** — Each story built on the previous with no conflicts. Protocol foundation → data ops → admin/security → SDK transport → benchmarks.

3. **Dual-protocol architecture works** — gRPC and FIBP run simultaneously on the same server, sharing the same scheduler and storage. Queue creation works via gRPC, data operations via FIBP.

4. **SDK API parity** — The Rust SDK exposes identical API whether using gRPC or FIBP. Only `ConnectOptions` changes.

5. **Wire format design** — Custom binary encoding (data ops) + protobuf (admin ops) proved practical. Binary encoding is zero-copy for payloads; protobuf gives schema evolution for admin.

## What Didn't Go Well

1. **FIBP consume bottleneck** — Credit-based flow control is too conservative (32 credits every 100ms = 320 msg/s max). This is a tuning issue in the SDK's `fibp_transport.rs` constants, not a protocol design flaw. Needs follow-up.

2. **TLS path duplicates dispatch logic** — `FibpConnection` is typed on `TcpStream`, so TLS connections need a separate code path. A future refactor could make it generic over `AsyncRead + AsyncWrite`.

3. **100K msg/s target not reached** — Best single-producer result: 20.5K msg/s (InMemory). Bottleneck shifted from transport to server-side processing (scheduler, message handling). Further gains require server-side optimization.

4. **No Cubic review findings** — While this seems positive, the lack of findings on a large custom protocol implementation suggests Cubic may not deeply analyze binary protocol code.

## Action Items

| # | Action | Encoded In | Status |
|---|--------|-----------|--------|
| 1 | Fix FIBP consume credit flow constants for higher throughput | Deferred — tuning fix, not structural | Deferred |
| 2 | Consider making FibpConnection generic over AsyncRead+AsyncWrite | Knowledge — cleanup, not blocking | Deferred |
| 3 | Update external SDKs (Go, Python, etc.) with FIBP support | Future epic — significant scope | Deferred |

## Previous Retro Action Items

| # | From Epic | Action | Status |
|---|-----------|--------|--------|
| 1 | Epic 35 | Phase 2 (Epic 36: FIBP) is GO | Done — Epic 36 complete |

## Key Metrics

- **Single-producer enqueue (RocksDB):** 9,587 → 18,130 msg/s (+89%)
- **Single-producer enqueue (InMemory):** 10,415 → 20,479 msg/s (+97%)
- **4-producer enqueue (RocksDB):** 23,634 → 40,648 msg/s (+72%)
- **4-producer enqueue (InMemory):** 25,311 → 45,738 msg/s (+81%)
- **Batch enqueue (RocksDB):** 11,369 → 18,101 msg/s (+59%)
- **Cumulative from pre-Plateau baseline (8,264 msg/s):** +119% to 18,130 msg/s (RocksDB)
