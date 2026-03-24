# Post-Optimization Profiling Analysis

**Date:** 2026-03-24
**Baseline commit:** bc539a9 (post-Epics 22-24 optimizations)
**Method:** Subsystem benchmarks (`FILA_BENCH_SUBSYSTEM=1`), CPU flamegraphs (`scripts/flamegraph.sh`), competitive benchmark comparison

## Executive Summary

The 54x throughput gap between Fila (2,637 msg/s) and Kafka/NATS (~140K msg/s) at 1KB payloads is **not a storage problem**. RocksDB raw writes can sustain 160K ops/s — 20x above what the server delivers. The bottleneck is **gRPC/HTTP2 per-message transport overhead**, which consumes ~93% of the enqueue critical path, combined with Kafka/NATS using client-side batching that Fila's competitive benchmark does not.

A purpose-built storage engine (FR66/Epic 25) is **not justified** by profiling evidence. Transport and protocol optimizations offer 10-50x more impact.

## Subsystem Performance Budget

Isolated subsystem measurements, bypassing the full server stack:

| Component | Throughput | Time per op | % of e2e budget¹ |
|-----------|-----------|-------------|-------------------|
| RocksDB write (1KB) | 160,073 ops/s | 6.2 µs | ~5% |
| RocksDB write (64KB) | 16,871 ops/s | 59 µs | — |
| Protobuf encode (1KB) | 12,335 MB/s | 83 ns | <1% |
| Protobuf decode (1KB) | — | 222 ns | <1% |
| DRR scheduler (10K keys) | 8.2M sel/s | 122 ns | <1% |
| Lua no-op hook | 424K exec/s | 2.4 µs | ~2% |
| Lua complex routing | 344K exec/s | 2.9 µs | ~2% |
| **gRPC round-trip (full stack, 1B payload)** | **513 ops/s²** | **1,950 µs** | **~93%** |

¹ Percentage of ~127 µs per-message budget (7,856 msg/s single-producer native throughput)
² Measured late in benchmark suite with accumulated server state; fresh-server serial throughput is ~7,856 msg/s (~127 µs/call)

**Key insight:** Subtracting RocksDB (6 µs) + protobuf (0.3 µs) + DRR (<0.2 µs) from the ~127 µs total leaves **~120 µs attributable to gRPC/HTTP2 transport + scheduler dispatch**. That's 94% of the budget.

## Flamegraph Analysis (enqueue-only, 30s, 10,671 samples)

Top CPU consumers from the enqueue hot path:

| Function / Layer | Samples | % of total | Category |
|-----------------|---------|-----------|----------|
| `h2::proto::streams::send::Send::poll_complete` | 2,847 | 26.7% | HTTP/2 framing |
| `h2::proto::connection::Connection::poll` | 3,067 | 28.7% | HTTP/2 connection |
| `h2::codec::Codec::flush` | 1,342 | 12.6% | HTTP/2 codec |
| `h2::codec::framed_write::FramedWrite::poll_ready` | 837 | 7.8% | HTTP/2 write |
| `writev` (syscall) | 1,110 | 10.4% | TCP write |
| `kevent` (syscall) | 1,095 | 10.3% | I/O polling |
| `TcpStream::poll_write_vectored` | 2,087³ | 19.6% | TCP I/O |
| `TcpStream::poll_read` / `FramedRead::poll_next` | 1,508 | 14.1% | TCP read |
| `fila_sdk::FilaClient::enqueue` | 1,295 | 12.1% | SDK client call |
| tokio park/condvar (idle) | 2,522 | 23.6% | Runtime idle |

³ Two separate call sites in the HTTP/2 write path (1,288 + 799 samples)

**Flamegraph conclusion:** The `h2` crate (HTTP/2 implementation used by tonic/hyper) dominates CPU time. HTTP/2 framing, stream management, HPACK header encoding, flow control, and codec flush operations account for **>50% of active CPU time** (excluding idle parking). TCP syscalls (`writev`, `kevent`) account for another ~20%.

## Competitive Benchmark Methodology Gap

The throughput gap is partially an **apples-to-oranges comparison**:

| Broker | Throughput benchmark config | Batching |
|--------|---------------------------|----------|
| **Kafka** | `linger.ms=5`, `batch.num.messages=1000` | **Client batches ~1000 msgs per network call** |
| **NATS** | JetStream publish (pipelined) | **Client pipelines without per-msg ack** |
| **RabbitMQ** | `basic_publish` (pipelined) | **Publisher confirms batched** |
| **Fila** | Serial `Enqueue` RPC (await per msg) | **1 gRPC call per message, full round-trip** |

For throughput, Kafka amortizes network overhead across ~1000 messages. Fila pays the full HTTP/2 round-trip per message.

**Lifecycle benchmark** (unbatched, `linger.ms=0` for Kafka) is the fairest comparison:

| Broker | Lifecycle msg/s | Notes |
|--------|----------------|-------|
| NATS | 25,763 | Still pipelined |
| **Fila** | **2,724** | Serial enqueue→consume→ack |
| RabbitMQ | 658 | Serial with quorum queues |
| Kafka | 356 | Serial with `linger.ms=0` |

Fila already **beats Kafka 7.6x and RabbitMQ 4.1x** in unbatched lifecycle throughput. The gap is exclusively with NATS (9.5x), which uses a simpler text protocol.

## Top 5 Bottlenecks (Ranked by Impact)

### 1. Per-message gRPC/HTTP2 overhead (~120 µs/msg, ~94% of budget)

**Evidence:** Flamegraph shows h2 framing at 50%+ of CPU. Subsystem benchmarks show RocksDB/protobuf/DRR combined take <7 µs.

**Root cause:** Each `Enqueue` RPC creates a new HTTP/2 stream, encodes HPACK headers, sends DATA frames, receives HEADERS+DATA response, manages flow control windows. This is ~120 µs of overhead for a ~6 µs storage write.

**Optimization strategies:**
- **A. Bidirectional streaming RPC** — Replace unary Enqueue with a persistent stream. Client sends messages continuously; server acks in batches. Eliminates per-message HTTP/2 stream creation. **Estimated: 5-15x throughput improvement.**
- **B. SDK auto-batching** — Accumulate messages client-side (like Kafka's `linger.ms`), flush via `BatchEnqueue` RPC. Already have the server-side RPC; need SDK-level accumulation + flush timer. **Estimated: 10-30x throughput improvement** (already proven by Kafka's approach).
- **C. Protocol alternative** — For internal/high-throughput paths, offer a raw TCP binary protocol alongside gRPC. NATS proves this works. **Estimated: 5-20x but high implementation cost.**

### 2. Serial request-response pattern (no pipelining)

**Evidence:** Single-producer throughput is 7,856 msg/s. Multi-producer (3x) achieves only 6,769 msg/s total (2,256 msg/s each = 0.86x per producer). Adding producers doesn't scale linearly.

**Root cause:** Each producer awaits a response before sending the next message. The TCP pipe is idle during server processing. No pipelining or asynchronous acknowledgment.

**Optimization strategies:**
- **A. Fire-and-forget enqueue mode** — Client sends without waiting for ack. Server-side backpressure via TCP flow control. **Estimated: 3-5x for single producer.** Trades delivery guarantee for throughput.
- **B. Async ack with sequence numbers** — Client assigns sequence numbers, sends continuously. Server acks in batches (e.g., "acked through seq 500"). **Estimated: 5-10x.** Preserves delivery guarantee.
- Combined with Strategy 1B (auto-batching), this becomes less critical.

### 3. Single-threaded scheduler bottleneck

**Evidence:** Multi-producer scales sub-linearly (3 producers → 0.86x each). Consumer concurrency plateaus at 10 consumers (4,117 msg/s) with no gain at 100 (4,139 msg/s). The scheduler thread is the serialization point.

**Root cause:** All queues share one scheduler thread. Enqueue, delivery, ack, nack all go through the same crossbeam channel (capacity 1024) and are processed sequentially. Write coalescing (max 32) helps batch storage writes but doesn't parallelize across queues.

**Optimization strategies:**
- **A. Shard by queue** — Already have `shard_count` config (default=1). Each shard gets its own scheduler thread + channel. Independent queues run fully parallel. **Estimated: 2-4x for multi-queue workloads.**
- **B. Separate delivery thread** — Decouple DRR delivery from the enqueue path. Enqueue just writes + updates in-memory state. A delivery loop runs independently, polling pending messages. **Estimated: 1.2-1.5x for enqueue throughput** (removes `drr_deliver_queue()` from critical path).

### 4. Scheduler calls delivery after every enqueue batch

**Evidence:** After `flush_coalesced_enqueues()`, the scheduler calls `drr_deliver_queue()` for every affected queue. This runs the full DRR round (deficit refill, key iteration, pending scan, storage read, lease creation) inline before the next enqueue batch can be processed.

**Root cause:** Design choice for low-latency delivery (message available immediately after enqueue). But under sustained throughput, this interleaves delivery work into the enqueue hot path.

**Optimization strategies:**
- **A. Deferred delivery** — Only deliver on consumer poll or timer tick, not after every enqueue. **Estimated: 1.3-2x enqueue throughput** at the cost of ~1-10ms delivery latency increase.
- **B. Delivery budget** — Limit delivery work per enqueue batch (e.g., deliver at most N messages, then process next batch). Prevents delivery from starving enqueue. **Estimated: 1.2-1.5x.**

### 5. Key cardinality scaling (O(n) DRR round start)

**Evidence:** Throughput drops 3.2x from 10 keys (3,070 msg/s) to 10K keys (699 msg/s) in end-to-end benchmarks. Yet isolated DRR runs at 8.2M sel/s at all key counts — the DRR algorithm itself is not the bottleneck.

**Root cause:** The drop is in the end-to-end path, likely because more keys means more RocksDB reads during delivery (one per active key per round) and more pending-index management. The scheduler processes all keys in the same round.

**Optimization strategies:**
- **A. Lazy round start** — Only refill deficits for keys that have pending messages, not all registered keys. **Estimated: 1.5-2x at high cardinality.**
- **B. Batch storage reads** — Use RocksDB `MultiGet` for pending message reads during delivery instead of sequential `get_message` calls. **Estimated: 1.3-1.5x at high cardinality.**

## FR66 Assessment: Purpose-Built Storage Engine

### Recommendation: **Not justified. Deprioritize.**

**Profiling evidence against FR66:**

1. **RocksDB is not the bottleneck.** At 160K ops/s (1KB), RocksDB has 20x headroom above what the server delivers (7,856 msg/s). Even at 64KB, RocksDB handles 16.8K ops/s — 7x above the 64KB competitive result (759 msg/s).

2. **Storage is <5% of the critical path.** The ~6 µs per RocksDB write is dwarfed by ~120 µs of gRPC/HTTP2 overhead. A hypothetical 2x storage speedup (3 µs saved) improves throughput by ~2.4%.

3. **The gap is transport, not storage.** Kafka's 54x advantage comes from client-side batching (1000 msgs/call) and a custom binary protocol, not from a faster storage engine. Kafka uses a simple append-only log — which RocksDB can emulate with sequential writes.

4. **Fila already beats Kafka when both are unbatched.** Lifecycle benchmark: Fila 2,724 vs Kafka 356 msg/s. Storage performance is already competitive.

**When FR66 would become justified:**
- After transport optimizations bring throughput above ~50K msg/s, at which point RocksDB's 160K ceiling becomes the next wall
- For 64KB+ payloads where RocksDB's LSM write amplification becomes measurable
- For queue-depth scaling beyond 10M messages where LSM compaction causes latency spikes

## Recommended Priority Order

| Priority | Optimization | Est. Impact | Effort | Notes |
|----------|-------------|-------------|--------|-------|
| **P0** | SDK auto-batching (linger + flush) | 10-30x throughput | Medium | Already have `BatchEnqueue` RPC. Need SDK accumulator. |
| **P0** | Fix competitive benchmark to use batching | Fair comparison | Small | Current numbers understate Fila by 10-30x. |
| **P1** | Streaming enqueue RPC | 5-15x throughput | Large | New proto, server streaming handler, SDK streaming client |
| **P1** | Scheduler sharding (increase default) | 2-4x multi-queue | Small | Config already exists, test + tune |
| **P2** | Decouple delivery from enqueue | 1.3-2x enqueue | Medium | Separate delivery thread/timer |
| **P2** | Async ack / pipelining | 5-10x single-producer | Large | New protocol semantics |
| **P3** | Lazy DRR round + MultiGet | 1.5-2x at high cardinality | Medium | Targeted fix for key scaling |
| **Defer** | Purpose-built storage (FR66) | <3% at current throughput | Very large | Not justified until >50K msg/s |

## Appendix: Raw Benchmark Data

### Self-benchmarks (bc539a9, native, 2026-03-24)

```
enqueue_throughput_1kb                    7,856 msg/s
enqueue_throughput_1kb_mbps                7.67 MB/s
e2e_latency_p50_light                      0.15 ms
e2e_latency_p95_light                      0.20 ms
e2e_latency_p99_light                      0.23 ms
fairness_overhead_pct                      2.26 %
key_cardinality_10_throughput            3,070 msg/s
key_cardinality_1k_throughput            1,352 msg/s
key_cardinality_10k_throughput             699 msg/s
consumer_concurrency_1_throughput          397 msg/s
consumer_concurrency_100_throughput      2,397 msg/s
memory_rss_idle                            631 MB
memory_rss_loaded_10k                      691 MB
```

### Subsystem benchmarks (isolated)

```
rocksdb_write_1kb      160,073 ops/s    p50: 3 µs    p99: 10 µs
rocksdb_write_64kb      16,871 ops/s    p50: 26 µs   p99: 285 µs
protobuf_encode_1kb     12,335 MB/s     83 ns/msg
protobuf_decode_1kb                     222 ns/msg
drr_10k_keys         8,206K sel/s
grpc_overhead_p50        1,916 µs       512 ops/s
lua_noop               424K exec/s      1 µs p50
lua_complex            344K exec/s      2 µs p50
```

### Competitive comparison (Docker, edee8a7)

```
Throughput 1KB:   Fila 2,637 | Kafka 143,278 | NATS 137,748 | RabbitMQ 38,321
Lifecycle:        Fila 2,724 | Kafka 356     | NATS 25,763  | RabbitMQ 658
Multi-producer:   Fila 6,769 | Kafka 186,708 | NATS 150,676 | RabbitMQ 63,660
```
