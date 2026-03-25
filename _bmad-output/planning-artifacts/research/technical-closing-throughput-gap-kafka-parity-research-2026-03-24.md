---
stepsCompleted: [1, 2, 3, 4, 5, 6]
inputDocuments: []
workflowType: 'research'
lastStep: 1
research_type: 'technical'
research_topic: 'Closing the throughput gap to Kafka parity (1x)'
research_goals: 'Identify architectural changes needed to reach 400K+ msg/s from current 22K msg/s. Profile-first analysis of server-side bottlenecks.'
user_name: 'Lucas'
date: '2026-03-24'
web_research_enabled: true
source_verification: true
---

# Research Report: Technical

**Date:** 2026-03-24
**Author:** Lucas
**Research Type:** Technical

---

## Research Overview

This research investigates why Fila achieves ~22K msg/s while Kafka achieves 422K msg/s on the same benchmark, and what architectural changes are needed to close the gap. Through profiling-first analysis, codebase tracing, and reference architecture study of Kafka, NATS, Redpanda, and TiKV, we identified a single root cause: **sequential message processing in gRPC handlers starves the scheduler's existing batch infrastructure**. Both `BatchEnqueue` and `StreamEnqueue` await a full scheduler round-trip (~3ms) per message, explaining the measured 328 msg/s per-client cap exactly. The fix is structural and the codebase is already batch-ready — `prepare_enqueue()`, `apply_mutations()`, and DRR delivery all support batch operations today. A tiered implementation strategy (batch commands → batch-aware protocol → per-message CPU elimination → horizontal sharding) projects a path to 400K+ msg/s. See the Executive Summary in the Research Synthesis for the complete findings and recommendations.

---

<!-- Content will be appended sequentially through research workflow steps -->

## Technical Research Scope Confirmation

**Research Topic:** Closing the throughput gap to Kafka parity (1x)
**Research Goals:** Identify architectural changes needed to reach 400K+ msg/s from current 22K msg/s. Profile-first analysis of server-side bottlenecks.

**Technical Research Scope:**

- Server-side enqueue path analysis — gRPC handler → protobuf decode → scheduler command channel → RocksDB write → response
- Scheduler serialization bottleneck — single command channel, single scheduler thread
- Server-side write batching — coalescing into WriteBatch
- gRPC protocol overhead — high-perf Rust gRPC reference points (TiKV, Redpanda, NATS)
- Alternative protocol evaluation — custom binary protocol, QUIC, flatbuffers
- Architecture patterns — thread-per-core, io_uring, append-only storage, sharded schedulers
- Realistic architecture for 400K+ msg/s

**Research Methodology:**

- Current web data with rigorous source verification
- Multi-source validation for critical technical claims
- Confidence level framework for uncertain information
- Profile-first — no speculative targets, reference architectures from production systems

**Scope Confirmed:** 2026-03-24

## Technology Stack Analysis

### The Root Cause: Sequential Message Processing

**Finding: Both BatchEnqueue and StreamEnqueue process messages sequentially through the scheduler, each awaiting a full round-trip before sending the next message. The scheduler's write coalescing exists but is starved because gRPC handlers feed messages one-at-a-time.**

**Evidence (code trace):**

`crates/fila-server/src/service.rs:224-225` — BatchEnqueue:
```rust
for enqueue_req in req.messages {
    let result = self.enqueue_single(&caller, enqueue_req).await; // SEQUENTIAL AWAIT
}
```

`crates/fila-server/src/service.rs:303-312` — StreamEnqueue (after draining up to 256 messages):
```rust
for req in batch {
    let result = enqueue_single_standalone(&caller, &broker, &cluster, enqueue_req).await; // SEQUENTIAL AWAIT
}
```

`crates/fila-server/src/service.rs:120-131` — enqueue_single_standalone (single-node path):
```rust
let (reply_tx, reply_rx) = tokio::sync::oneshot::channel();
broker.send_command(SchedulerCommand::Enqueue { message, reply: reply_tx }); // Send to crossbeam
let msg_id = reply_rx.await; // BLOCK until scheduler processes and replies
```

**Per-message round-trip:** gRPC handler → crossbeam channel send → scheduler `try_recv()` → `prepare_enqueue()` (Lua + protobuf serialize) → `apply_mutations()` (RocksDB WriteBatch for 1 message) → oneshot reply → gRPC handler.

**Measured latency:** ~3ms per message round-trip. 1000 messages × 3ms = 3 seconds → **333 msg/s** (matches the 328 msg/s measurement exactly).

**The scheduler's write coalescing** (`drain_and_coalesce()` at `scheduler/mod.rs:169-216`) collects up to 100 enqueue commands and commits them in one `WriteBatch`. But since BatchEnqueue/StreamEnqueue feed messages one-at-a-time (each awaiting before sending the next), the coalescing window never contains more than 1 message from a single client.

**Why 4-producer parallel achieves 22K msg/s:** 4 independent clients send to the crossbeam channel concurrently. The scheduler's `try_recv()` loop picks up messages from all 4 producers in a single drain cycle, enabling actual coalescing. This is the only path that currently benefits from write batching.

### Fila's Server Architecture (Current)

| Component | Technology | Performance |
|-----------|-----------|-------------|
| Transport | tonic gRPC over HTTP/2 | ~512 unary ops/s per connection |
| Serialization | protobuf (prost) | 12,335 MB/s encode, 222 ns/msg decode |
| Command channel | crossbeam bounded (cap: 100) | Sub-microsecond send/recv |
| Scheduler | Single thread per shard (default: 1 shard) | Runs drain → DRR delivery → park loop |
| Write coalescing | Up to 100 messages per WriteBatch | Works, but starved by sequential handlers |
| Storage | RocksDB with column families | 160K ops/s (1KB), 6.2 µs/write |
| Fairness scheduling | Deficit Round Robin | 8.2M selections/s |
| Lua hooks | mlua (LuaJIT) | 424K exec/s (no-op), 344K (complex) |

### How Kafka Achieves 422K msg/s

Kafka's architecture is fundamentally different in its write path:

1. **Client-side batching (RecordAccumulator):** Messages accumulate in `RecordBatch` structures. With `linger.ms=5` and `batch.size=1MB`, ~1000 messages per network call. The default `linger.ms` changed from 0 to 5 in Kafka 4.0 because "the efficiency gains from larger batches typically result in similar or lower producer latency despite the increased linger."
_Source: [Kafka Performance Tuning: linger.ms and batch.size](https://www.automq.com/blog/kafka-performance-tuning-linger-ms-batch-size)_

2. **Standardized binary format:** Producer, broker, and consumer share the same binary message format. The broker writes the batch as-is — no per-message deserialization, re-serialization, or processing on the write path. "The batch of messages is written to disk in compressed form."
_Source: [Kafka Batch Processing for Efficiency](https://docs.confluent.io/kafka/design/efficient-design.html)_

3. **Append-only sequential writes to page cache:** The broker appends entire batches to segment files. The OS page cache handles durability. No per-message RocksDB operations, no LSM compaction. "When a broker receives data from a producer, it is immediately written to a persistent log on the filesystem... this does not mean it will be flushed to disk."
_Source: [How Kafka Is so Performant If It Writes to Disk?](https://andriymz.github.io/kafka/kafka-disk-write-performance/)_

4. **Zero-copy consume path:** `sendfile()` syscall transfers data from page cache to network socket without user-space copies.
_Source: [What is Zero Copy in Kafka?](https://www.nootcode.com/knowledge/en/kafka-zero-copy)_

**Key insight:** Kafka's broker does almost no per-message work. It validates the batch header, appends the blob, and responds. All per-message processing (serialization, partitioning, compression) happens on the client.

### How NATS Achieves 138K msg/s

NATS takes a different approach — protocol simplicity:

1. **Simple text protocol:** `PUB subject payload_length\r\n[payload]\r\n`. Zero allocation byte parser. No HTTP/2 framing, no protobuf, no HPACK compression.
_Source: [NATS Client Protocol](https://docs.nats.io/reference/reference-protocols/nats-protocol)_

2. **Pipelined publishes:** Client fires messages without waiting for per-message acks. JetStream uses async publish with batch acknowledgements.
_Source: [NATS JetStream](https://docs.nats.io/nats-concepts/jetstream)_

3. **Deferred fsync:** JetStream's `sync_interval` defaults to 2 minutes. "Forcing an fsync after each message will slow down the throughput to a few hundred msg/s."
_Source: [NATS FAQ](https://docs.nats.io/reference/faq)_

4. **Go goroutine multiplexing:** NATS server written in Go, uses goroutines for concurrent connection handling.
_Source: [NATS Server Architecture](https://github.com/nats-io/nats-general/blob/main/architecture/ARCHITECTURE.md)_

### How Redpanda Achieves >1 GB/s per Core

Redpanda represents the ceiling of what's achievable with aggressive systems engineering:

1. **Thread-per-core (Seastar):** "Intentionally allocates one thread to each CPU core and pins it to that core." Zero locking, NUMA-local memory, per-core I/O queues. Each core is a fully independent shard.
_Source: [What makes Redpanda fast?](https://www.redpanda.com/blog/what-makes-redpanda-fast)_

2. **io_uring + DMA + O_DIRECT:** Bypasses filesystem page cache entirely. Asynchronous disk I/O through io_uring. "It takes two cores to saturate a single NVMe disk."
_Source: [Redpanda Architecture](https://docs.redpanda.com/current/get-started/architecture/)_

3. **Kafka-compatible protocol:** Reuses Kafka's binary protocol (RecordBatch), getting the same client-side batching benefits without inventing a new protocol.
_Source: [Redpanda vs Apache Kafka](https://www.automq.com/blog/redpanda-vs-apache-kafka-event-streaming)_

### How TiKV Optimizes RocksDB Writes Under Concurrency

TiKV faces the same challenge as Fila — high-concurrency writes to RocksDB through a Raft consensus layer:

1. **Multi-batch write optimization:** RocksDB's default write coordination uses a leader-follower model where one writer holds the DB mutex and writes for the group. TiKV's optimization introduces a two-stage pipeline: "The leader writes the entire group to the WAL, after which threads are released from the WAL stage, and a separate set of memtable writers write the batches into memtables."
_Source: [How We Optimize RocksDB in TiKV — Write Batch Optimization](https://medium.com/@siddontang/how-we-optimize-rocksdb-in-tikv-write-batch-optimization-28751a4bdd8b)_

2. **Raft batch processing:** TiKV "traverses all the ready groups and uses a RocksDB WriteBatch to handle all appending data and persist the corresponding result at the same time." Multiple Raft groups coalesced into single writes.
_Source: [Deep Dive TiKV](https://tikv.github.io/deep-dive-tikv/print.html)_

3. **Results:** "P9999 write thread wait time dropped by over 90% (25ms → 2ms). P99 write latency dropped by more than 50% (2–4ms → 1ms)."
_Source: [How We Optimize RocksDB in TiKV — Smarter Flow Control](https://medium.com/@siddontang/how-we-optimize-rocksdb-in-tikv-smarter-flow-control-f6af95cbf87e)_

### gRPC/tonic Performance Characteristics

Tonic (Rust gRPC) is CPU-efficient but latency-sensitive:

- **Memory:** 16 MB under load — most efficient across all language implementations (vs 200 MB Java, 150 MB .NET).
- **CPU:** "First place in testing... most efficient implementation CPU-wise."
- **Latency caveat:** "Rust (tonic gRPC) showed the worst latency profile" in some benchmarks, likely due to h2 crate implementation.
_Source: [Comparing gRPC performance across different technologies](https://nexthink.com/blog/comparing-grpc-performance)_

**Streaming vs Unary:** "Unary requests require a new HTTP2 stream to be established for each request including additional header frames... once established, each new message sent on a streamed request only requires the data frame." This eliminates per-message stream establishment overhead.
_Source: [The Mysterious Gotcha of gRPC Stream Performance](https://ably.com/blog/grpc-stream-performance)_

**Future tonic optimizations:** A successor library is being developed with "Protobuf Arenas: borrowing a strategy from Google's C++ implementation... memory pooling to reuse messages, significantly reducing allocation overhead. Zero-Copy IO: ensuring there are no unnecessary user-space copies between the wire and the library."
_Source: [Safe, Fast, and Scalable: Why gRPC-Rust Should Be Your Next RPC Framework](https://tldrecap.tech/posts/2025/grpconf-india/grpc-rust-successor/)_

### io_uring and Thread-per-Core in Rust

Two mature Rust runtimes offer Seastar-like architecture:

1. **Glommio** (Datadog): "Cooperative Thread-per-Core crate for Rust & Linux based on io_uring." Per-thread ring sets operated locklessly. Requires Linux kernel ≥5.8.
_Source: [Introducing Glommio](https://www.datadoghq.com/blog/engineering/introducing-glommio/)_

2. **Monoio** (ByteDance): "Pure io_uring/epoll/kqueue Rust async runtime." Thread-per-core, no Send/Sync requirements. "Under 4 cores, Monoio's peak performance is about twice that of Tokio, and under 16 cores, it is close to 3 times."
_Source: [Monoio Benchmark](https://github.com/bytedance/monoio/blob/master/docs/en/benchmark.md)_

**Critical constraint:** Both require abandoning Tokio. Tonic depends on Tokio. A thread-per-core migration means replacing tonic with a custom protocol layer. This is a rewrite-tier change.

### Technology Adoption Trends

| Pattern | Who Uses It | Throughput Achieved |
|---------|------------|-------------------|
| Client-side batch + append-only log | Kafka, Redpanda, Pulsar | 400K–1M+ msg/s |
| Simple text protocol + pipelining | NATS | 130K–250K msg/s |
| Thread-per-core + io_uring | Redpanda, ScyllaDB | 1M+ msg/s |
| gRPC + RocksDB + Raft batching | TiKV, CockroachDB | 100K–300K ops/s |
| gRPC streaming + write coalescing | Fila (current) | 22K msg/s |

_Throughput numbers are approximate, workload-dependent, and based on published benchmarks under favorable conditions._

## Integration Patterns Analysis

### Pattern 1: End-to-End Batch Propagation (Kafka Model)

The dominant pattern for high-throughput message brokers is treating **batches as the unit of work at every layer** — not individual messages.

**Kafka's approach:**

```
Producer:
  messages → RecordAccumulator → RecordBatch (linger 5ms or batch.size 1MB)
      ↓
Wire: Produce request = [topic → [partition → RECORDS blob]]
      ↓
Broker: validate batch header → append blob to segment file (page cache)
      ↓
Consumer: Fetch response = same RECORDS blob → zero-copy sendfile
```

"The protocol is structured around a 'message set' abstraction that groups messages together... the server appends message chunks to its log in a single operation."
_Source: [Kafka Batch Processing for Efficiency](https://docs.confluent.io/kafka/design/efficient-design.html)_

The broker does **no per-message work** on the write path. The RecordBatch is an opaque byte sequence — "Represents a sequence of Kafka records as NULLABLE_BYTES." Validation is per-batch (count check), not per-message.
_Source: [Kafka Protocol](https://kafka.apache.org/41/design/protocol/)_

**Why this matters for Fila:** Fila's `enqueue_single_standalone()` does per-message work even in batch/stream paths: UUID generation, ACL check, Message struct construction, oneshot channel, scheduler round-trip. Adopting the Kafka model means the batch itself must be the unit that traverses the pipeline.

### Pattern 2: Pipelined Writes Without Per-Message Acks (NATS Model)

NATS decouples write acknowledgement from message sending:

1. Client publishes messages in rapid succession without waiting for per-message acks
2. JetStream batches acknowledgements asynchronously
3. Server-side fsync is deferred (`sync_interval` defaults to 2 minutes)

"Forcing an fsync after each message will slow down the throughput to a few hundred msg/s."
_Source: [NATS FAQ](https://docs.nats.io/reference/faq)_

**Applied to Fila:** The current design requires a full scheduler round-trip per message because each `enqueue_single()` creates a `oneshot::channel` and awaits the reply. A pipelined model would:
1. Client sends N messages on a stream without awaiting each
2. Server buffers and batches them server-side
3. Server sends periodic batch acks (e.g., "messages 1-1000 committed")

### Pattern 3: gRPC Streaming as Batch Transport

gRPC streaming amortizes HTTP/2 overhead but requires careful batch structuring:

**Current Fila StreamEnqueue:** Drains up to 256 messages from the inbound stream, but then processes them **one-at-a-time** with sequential awaits. The drain is wasted because the processing is serial.

**Optimized pattern (from gRPC best practices):** "Sending multiple documents at once reduces the number of round trips between the client and the server." Benchmark results show batch streaming achieves **1.64x throughput improvement** (1202 vs 732 ops/sec) with **57% fewer allocations**.
_Source: [gRPC Streaming: Best Practices and Performance Insights](https://dev.to/ramonberrutti/grpc-streaming-best-practices-and-performance-insights-219g)_

**Ably's finding on stream packet efficiency:** At 1,300 msg/s on gRPC streams, "27% of CPU time [was] spent on read/write syscalls, 17% on scheduling goroutines." The key finding was that desynchronized message publication caused "an order of magnitude more TCP packets" due to missed TCP Nagling opportunities.
_Source: [The Mysterious Gotcha of gRPC Stream Performance](https://ably.com/blog/grpc-stream-performance)_

**Implication:** gRPC streaming's throughput depends on batching messages within the stream, not just using the stream. Individual `StreamEnqueueRequest` per message still incurs per-message HTTP/2 DATA frame overhead. The optimal pattern is a wrapper message with `repeated` messages inside a single stream write.

### Pattern 4: RocksDB Group Commit and Pipelined Writes

Fila already uses `WriteBatch` but can exploit RocksDB's built-in group commit:

**Group commit:** "When different threads are writing to the same DB at the same time, all outstanding writes that qualify to be combined will be combined together and written to WAL once, with one fsync. Maximum group size is 1MB."
_Source: [RocksDB WAL Performance](https://github.com/facebook/rocksdb/wiki/WAL-Performance)_

**Pipelined writes:** "Once the previous writer finishes its WAL write, the next writer can start writing to the WAL while the previous writer still has its memtable write ongoing." Result: **20-30% throughput improvement** with concurrent writers.
_Source: [RocksDB Pipelined Write](https://github.com/facebook/rocksdb/wiki/Pipelined-Write)_

**Manual WAL flush:** "Entries remain buffered in RocksDB. Users must explicitly call DB::FlushWAL() to move buffered entries to the filesystem, reducing CPU latency." Fila already uses `manual_wal_flush` — this is correctly configured.
_Source: [RocksDB WAL Performance](https://github.com/facebook/rocksdb/wiki/WAL-Performance)_

**Current Fila configuration** (`crates/fila-core/src/storage/rocksdb.rs`): `pipelined_write` is configurable, `manual_wal_flush` is enabled. These are already set correctly. The bottleneck is NOT RocksDB write performance — it's that messages never arrive at RocksDB in batches because the gRPC handler serializes them.

### Pattern 5: Async/Sync Channel Bridge

Fila bridges tokio async (gRPC handlers) to a synchronous scheduler thread via `crossbeam_channel::bounded`:

```
tokio task (async) → crossbeam::bounded::send() [blocks tokio thread!] → scheduler thread (sync)
scheduler thread → oneshot::Sender::send() → tokio task (async)
```

**Problem 1:** `crossbeam_channel::send()` is a blocking call. If the channel is full (capacity: 100), the tokio worker thread blocks, potentially starving other tasks. This is documented as an anti-pattern: "Crossbeam channels are for use outside of asynchronous Rust and wait for messages by blocking the thread, which is not allowed in asynchronous code."
_Source: [Rust Channel Comparison](https://users.rust-lang.org/t/differences-between-channel-in-tokio-mpsc-and-crossbeam/92676)_

**Problem 2:** The oneshot-per-message pattern creates allocation overhead. Each message creates a oneshot channel pair. At 400K msg/s, that's 400K allocations/deallocations per second.

**Alternative approaches:**
- `tokio::sync::mpsc` bounded channel (async-native, no thread blocking)
- Batch command with single reply channel: `EnqueueBatch { messages: Vec<Message>, reply: oneshot::Sender<Vec<Result>> }`
- `crossfire` crate: "lockless spsc/mpsc/mpmc channels that support async contexts" with +30% MPSC performance
_Source: [Crossfire Crate](https://crates.io/crates/crossfire)_

### Pattern 6: Fsync Strategy — The Durability/Throughput Tradeoff

Every high-throughput broker makes a choice about when to fsync:

| Broker | Fsync Strategy | Write Throughput Impact |
|--------|---------------|----------------------|
| Kafka | Page cache only; OS-controlled flush | Highest throughput; relies on replication for durability |
| NATS JetStream | `sync_interval=2m` (configurable) | High throughput; crash window = sync_interval |
| RabbitMQ | fsync per publisher confirm (quorum) | Low throughput (~658 msg/s lifecycle) |
| Fila | `manual_wal_flush`; no per-write fsync | Good default; WAL flushed on shutdown |

"With default sync settings for lower producer batch sizes (1 KB and 10 KB), throughput is 3–5x higher than fsyncing every message."
_Source: [Kafka Fastest Messaging System](https://www.confluent.io/blog/kafka-fastest-messaging-system/)_

Fila's current fsync strategy is already optimal for throughput. The durability guarantee comes from RocksDB WAL buffering + `manual_wal_flush` + explicit flush on shutdown. No changes needed here.

### Integration Pattern Synthesis: What Fila Must Change

Mapping the patterns above to Fila's specific bottleneck:

| Current Pattern | Problem | Required Pattern |
|----------------|---------|-----------------|
| Sequential `enqueue_single().await` in batch/stream | 1 scheduler round-trip per message | Batch command: 1 round-trip per batch |
| Per-message oneshot channel | 400K allocations/s at target | Batch reply: 1 oneshot per batch |
| Protobuf serialize per-message in scheduler | CPU work per message | Batch serialization or store-as-received |
| crossbeam bounded channel (blocking) | Blocks tokio threads | tokio::sync::mpsc or spawn_blocking |
| Per-message DRR update after write | Interleaves delivery work into hot path | Batch DRR update after batch commit |
| gRPC stream sends individual requests | Per-message HTTP/2 DATA frames | Wrapper message with repeated field |

**Confidence level: HIGH** — The sequential processing is a measured bottleneck (328 msg/s = exactly 1000/3ms round-trip). The fix is structural, not speculative.

## Architectural Patterns and Design

### Architecture Pattern 1: Single Writer with Batch Draining (LMAX Model)

The LMAX Disruptor architecture achieves **6 million TPS on a single thread** by combining three principles: single-writer, in-memory state, and batch processing.

"Only one core writes to any memory location. Each producer/consumer reads others' counters without locks." The ring buffer allows "consumers to read data from slots in one batch to catch up" when lagging.
_Source: [The LMAX Architecture](https://martinfowler.com/articles/lmax.html)_

**Fila's scheduler already follows this pattern.** Each scheduler shard is a single-writer thread that owns all state for its queues. The `drain_and_coalesce()` loop is a batch-draining consumer. The problem is not the scheduler design — it's that the **gRPC handler starves the batch drainer by feeding messages one-at-a-time**.

The fix is to make the gRPC handler behave like a proper LMAX input disruptor: buffer messages, then submit the entire batch to the ring (channel) in one operation. The scheduler's existing batch drain handles the rest.

**Architecture decision:** Keep the single-writer scheduler. Fix the input path.

### Architecture Pattern 2: Thread-per-Core vs Work-Stealing

Two competing models for high-throughput servers:

**Work-stealing (Tokio):** Threads steal tasks from each other when idle. Better CPU utilization under uneven loads. Requires `Send + Sync` bounds. Cache misses when tasks migrate between cores.

**Thread-per-core (Glommio/Monoio):** One thread pinned to one core. Zero locking, NUMA-local memory, cache-friendly. "Substantial improvements in tail latency" but demands careful state partitioning.
_Source: [Thread-per-core](https://without.boats/blog/thread-per-core/)_

**The fundamental tension:** "It may be one or the other, but not both — you choose performance optimization or implementation simplicity." Work-stealing costs synchronization overhead but handles load imbalance. Thread-per-core eliminates overhead but becomes brittle with hot keys or uneven partitions.
_Source: [Thread-per-core](https://without.boats/blog/thread-per-core/)_

**Fila's position:**
- Fila's scheduler is already thread-per-core (one thread per shard, owns all state for assigned queues)
- Fila's gRPC layer runs on Tokio (work-stealing) — this is correct for I/O-bound connection handling
- The hybrid model (Tokio for I/O, dedicated threads for processing) is sound

**Architecture decision:** Do NOT migrate to Glommio/Monoio. The scheduler is already single-threaded-per-shard. The I/O layer benefits from Tokio's work-stealing for connection handling. A full runtime migration would be a rewrite with uncertain payoff. Monoio's 2-3x advantage over Tokio is for raw I/O throughput — Fila's bottleneck is the handler→scheduler bridge, not I/O syscalls.

### Architecture Pattern 3: Append-Only Log vs LSM Tree

Kafka uses append-only segment files. Fila uses RocksDB (LSM tree). Which is right?

"Message queues when ordered usually just need some kind of append-only log file" — simpler, zero write amplification, sequential I/O only.
_Source: [HN Discussion: LSM Trees for Message Queues](https://news.ycombinator.com/item?id=10407291)_

LSM trees add compaction overhead (write amplification) but provide "sophisticated indexing and query optimization" for non-sequential access patterns.
_Source: [LSM Tree Explained](https://aerospike.com/blog/log-structured-merge-tree-explained/)_

**Fila's tradeoffs:**
- Fila needs random-access reads for consume (DRR selects specific fairness keys, not sequential scan)
- Fila needs efficient deletes (ack removes specific messages)
- Fila's fairness scheduling requires indexed access by `(queue_id, fairness_key)` prefix
- These are NOT append-only-log access patterns — LSM tree is correct for Fila's semantics

**However:** RocksDB has 160K ops/s at 1KB, 20x above current throughput. Write amplification from compaction is not the bottleneck and won't be until throughput exceeds ~50K msg/s with large payloads.

**Architecture decision:** Keep RocksDB. An append-only log would require rebuilding the fairness scheduling index, DRR state, and consume-by-key semantics. Cost is enormous, benefit is negligible at current throughput gap. Re-evaluate only after transport+batching fixes push throughput past 100K msg/s.

### Architecture Pattern 4: Scheduler Sharding

Fila already supports multiple scheduler shards (`crates/fila-core/src/broker/mod.rs:67-116`). Each shard is a separate thread with its own crossbeam channel. Messages route to shards by queue_id hash.

**Current default: 1 shard.** This means ALL queues funnel through one scheduler thread.

**RabbitMQ's sharding pattern:** "Sharding shows one queue to the consumer, but in reality it consists of many queues running in the background, giving you a centralized place where you can send your messages, plus load balancing across many nodes."
_Source: [RabbitMQ Sharding](https://github.com/rabbitmq/rabbitmq-sharding)_

**For multi-queue workloads:** Increasing `shard_count` to match CPU core count provides linear throughput scaling. Each shard handles a subset of queues independently.

**For single-queue workloads:** Sharding by queue_id doesn't help — all messages for one queue go to one shard. Sharding by fairness_key within a queue would require significant refactoring of the DRR scheduler.

**Architecture decision:** Increase default `shard_count` to match available cores. This is a config change, not an architecture change. For single-queue scaling, the batch processing fix (Pattern 1) is far more impactful.

### Architecture Pattern 5: Tiered Optimization Strategy

Based on profiling data and reference architectures, the path to 400K msg/s breaks into tiers with decreasing certainty:

**Tier 0: Fix the Bug (the sequential handler)**
- Make BatchEnqueue/StreamEnqueue submit entire batches to the scheduler as one command
- One channel send, one oneshot await, one WriteBatch for N messages
- **Expected: 10-50x** improvement on batch paths (eliminates 3ms × N round-trip)
- **Confidence: VERY HIGH** — this is a measured bottleneck with a clear fix
- **Effort: Small-Medium** — new `SchedulerCommand::EnqueueBatch` variant + handler refactor

**Tier 1: Batch-aware gRPC protocol**
- New proto message: `BatchStreamRequest { repeated EnqueueRequest messages = 1; }` over bidirectional stream
- Client SDK accumulates messages (like Kafka's RecordAccumulator) and flushes on linger timeout or batch size
- Server receives pre-batched messages, submits directly to scheduler
- Pipelined acks: server responds with batch sequence numbers, not per-message
- **Expected: 2-5x** on top of Tier 0 (eliminates per-message HTTP/2 DATA frame overhead)
- **Confidence: HIGH** — well-understood pattern from Kafka/NATS
- **Effort: Medium** — proto changes, SDK changes, backward-compatible

**Tier 2: Server-side processing optimization**
- Skip per-message protobuf re-serialization: store the batch blob as-received (like Kafka)
- Batch Lua hook execution: process Vec<Message> in one Lua call instead of N calls
- Batch DRR updates: one `drr_deliver()` call per batch, not per message
- **Expected: 2-3x** on top of Tier 1 (eliminates remaining per-message CPU work)
- **Confidence: MEDIUM** — depends on Lua hook semantics and DRR batch-ability
- **Effort: Medium-Large** — storage format changes, Lua API changes

**Tier 3: Horizontal scaling**
- Increase default `shard_count` to CPU core count
- Benchmark to find optimal shard count for single-queue vs multi-queue
- **Expected: 2-4x** for multi-queue workloads (linear with cores)
- **Confidence: HIGH** for multi-queue, LOW for single-queue
- **Effort: Small** — config change

**Tier 4: Advanced (only if Tiers 0-3 insufficient)**
- Custom binary protocol (eliminate HTTP/2 entirely)
- io_uring for disk I/O (bypass filesystem cache for large payloads)
- Zero-copy consume path (sendfile-style)
- Thread-per-core I/O (Monoio/Glommio migration)
- **Expected: 2-5x** on top of Tiers 0-3
- **Confidence: LOW** — requires profiling after Tiers 0-3 to know if needed
- **Effort: Very Large** — near-rewrite of transport layer

### Projected Throughput by Tier

| Tier | Change | Single-Client Batch | Multi-Producer (4) |
|------|--------|--------------------|--------------------|
| Current | Sequential processing | 328 msg/s | 22K msg/s |
| Tier 0 | Batch commands to scheduler | ~30K msg/s | ~100K msg/s |
| Tier 1 | Batch-aware gRPC stream | ~100K msg/s | ~300K msg/s |
| Tier 2 | Skip per-message processing | ~200K msg/s | ~500K msg/s |
| Tier 3 | Multi-shard (4 cores) | ~200K msg/s (1Q) | ~800K msg/s (4Q) |

_Projections based on: RocksDB 160K ops/s headroom, protobuf 83ns/msg, DRR 122ns/selection, current 3ms per-message round-trip. These are estimates — each tier must be profiled after the previous tier is implemented._

### Critical Architecture Constraints

1. **Fila is NOT Kafka.** Fila has fairness scheduling (DRR), Lua hooks, per-message routing, and weighted delivery. Kafka has none of these — it appends blobs and reads sequentially. Some per-message work is inherent to Fila's value proposition. The goal is to batch that work, not eliminate it.

2. **Raft clustering adds a multiplier.** In cluster mode, each enqueue goes through Raft consensus before storage. Batch Raft proposals (like TiKV) are essential for cluster-mode throughput. This is a separate concern from the single-node bottleneck.

3. **Backward compatibility matters.** The existing unary `Enqueue` RPC must continue to work. Batch optimization is additive — new RPCs and SDK features, not breaking changes.

4. **Profile after each tier.** The CLAUDE.md rule applies: "Performance epics must begin with profiling to identify actual bottlenecks." Each tier changes the bottleneck. Tier 0 will likely move the bottleneck from handler→scheduler bridge to gRPC framing. Tier 1 will likely move it to per-message CPU work. Don't implement Tier 2 until profiling confirms Tier 1's bottleneck.

## Implementation Approaches and Technology Adoption

### Tier 0 Implementation: Batch Commands to Scheduler

**Feasibility assessment:** The codebase is already batch-ready. `flush_coalesced_enqueues()` implements the complete batch pattern — prepare per-message, collect mutations, single `apply_mutations()`, finalize per-message, deliver per-queue. The missing piece is getting batches INTO the scheduler from the gRPC handler.

#### Step 1: Add SchedulerCommand::EnqueueBatch

**File:** `crates/fila-core/src/broker/command.rs`

Add new variant:
```rust
EnqueueBatch {
    messages: Vec<crate::message::Message>,
    reply: tokio::sync::oneshot::Sender<Vec<Result<uuid::Uuid, crate::error::EnqueueError>>>,
},
```

**Design decision:** Single reply channel with Vec of per-message results (not one oneshot per message). This eliminates N channel allocations.

#### Step 2: Handle EnqueueBatch in Scheduler

**File:** `crates/fila-core/src/broker/scheduler/mod.rs`

In `handle_command()`, add match arm that reuses the core batch logic from `flush_coalesced_enqueues()`:
1. Call `prepare_enqueue()` for each message, collect successes/failures
2. Call `apply_mutations()` once for all successful preparations
3. Call `finalize_enqueue()` for each success
4. Call `drr_deliver_queue()` once per unique queue
5. Send `Vec<Result<Uuid, EnqueueError>>` through reply channel

**Code reuse:** Extract the core logic from `flush_coalesced_enqueues()` (lines 220-299) into a shared `process_enqueue_batch()` method. Both `flush_coalesced_enqueues` and the new `EnqueueBatch` handler call it.

#### Step 3: Route EnqueueBatch in Broker

**File:** `crates/fila-core/src/broker/mod.rs`

**Complication:** Messages in a batch can target different queues, which route to different shards. Options:

**Option A (Recommended):** Group-by-shard in the broker's `send_command()`:
- Group messages by shard (using queue_id hash)
- Send one `EnqueueBatch` per shard
- Collect results from all shards, reorder to match input order

**Option B (Simpler):** Require all messages in a batch to target the same queue (add a `queue` field to `BatchEnqueueRequest`). This is a proto change but simplifies routing.

**Option C (Simplest for Tier 0):** Group-by-queue in the gRPC handler (service.rs), send one `EnqueueBatch` per queue. Each batch routes cleanly to one shard.

#### Step 4: Update gRPC Handler

**File:** `crates/fila-server/src/service.rs`

Replace the sequential loop in `batch_enqueue()` (line 224) with:
```rust
// Group messages by queue for routing
let mut by_queue: HashMap<String, Vec<(usize, Message)>> = HashMap::new();
for (idx, req) in req.messages.into_iter().enumerate() {
    let msg = construct_message(req); // extract from enqueue_single_standalone
    by_queue.entry(msg.queue_id.clone()).or_default().push((idx, msg));
}

// Send one batch command per queue
let mut result_slots = vec![None; total_count];
for (queue_id, msgs) in by_queue {
    let (reply_tx, reply_rx) = oneshot::channel();
    let indices: Vec<usize> = msgs.iter().map(|(i, _)| *i).collect();
    let messages: Vec<Message> = msgs.into_iter().map(|(_, m)| m).collect();
    broker.send_command(SchedulerCommand::EnqueueBatch { messages, reply: reply_tx })?;
    let results = reply_rx.await?;
    for (i, result) in indices.into_iter().zip(results) {
        result_slots[i] = Some(result);
    }
}
```

Similarly update `stream_enqueue()` (line 303) to batch the drained messages.

#### Step 5: Update StreamEnqueue

The stream handler already drains up to 256 messages. Change from:
```rust
for req in batch {
    let result = enqueue_single_standalone(...).await; // sequential
}
```
To the same group-by-queue + batch command pattern.

#### Effort Assessment

| Component | Change | Lines | Risk |
|-----------|--------|-------|------|
| SchedulerCommand enum | Add variant | ~5 | Low |
| Scheduler batch handler | Extract from flush_coalesced_enqueues | ~50 | Low — reuse existing tested code |
| Broker routing | Add EnqueueBatch to routing + group-by-shard | ~30 | Medium — new routing logic |
| service.rs batch_enqueue | Replace sequential loop | ~40 | Medium — message construction refactor |
| service.rs stream_enqueue | Replace sequential loop | ~30 | Medium — same pattern |
| Tests | Unit + integration tests for batch path | ~100 | Low |
| **Total** | | **~255 lines** | **Medium** |

**No changes needed:**
- `prepare_enqueue()` — already per-message, no state mutations
- `finalize_enqueue()` — already per-message, keyed state updates
- `apply_mutations()` — already batches atomically via `Vec<Mutation>`
- DRR delivery — already batched per-queue in `flush_coalesced_enqueues()`
- Storage layer — fully batch-ready
- Proto definitions — `BatchEnqueueRequest` already has `repeated` messages

### Tier 1 Implementation: Batch-Aware gRPC Protocol

#### Proto Changes

New streaming RPC with batch-within-stream:
```protobuf
service Fila {
    // Existing
    rpc StreamEnqueue(stream StreamEnqueueRequest) returns (stream StreamEnqueueResponse);

    // New: batch-within-stream
    rpc StreamBatchEnqueue(stream StreamBatchEnqueueRequest) returns (stream StreamBatchEnqueueResponse);
}

message StreamBatchEnqueueRequest {
    uint64 sequence_number = 1;
    repeated EnqueueRequest messages = 2;  // Multiple messages per stream write
}

message StreamBatchEnqueueResponse {
    uint64 sequence_number = 1;
    repeated BatchEnqueueResult results = 2;
}
```

"The repeated field approach is akin to batching... the entire set of messages is received before processing." Starting from protobuf v2.1.0, packed encoding is default for repeated fields, reducing wire size.
_Source: [gRPC streaming services vs. repeated fields](https://learn.microsoft.com/en-us/dotnet/architecture/grpc-for-wcf-developers/streaming-versus-repeated)_

#### SDK Changes — Client-Side Batching

**Kafka-style RecordAccumulator for Fila SDK:**

```rust
pub struct BatchConfig {
    pub max_batch_size: usize,     // e.g., 1000 messages
    pub linger: Duration,           // e.g., 5ms
    pub max_batch_bytes: usize,     // e.g., 1MB
}
```

SDK accumulates messages in a buffer. Flushes when:
1. Buffer reaches `max_batch_size` messages, OR
2. Buffer reaches `max_batch_bytes` total payload, OR
3. `linger` duration expires since first message in buffer

Client gets a `Future<Result<Uuid>>` per message that resolves when the batch ack returns.

**Pipelined acks:** Client sends batches on the stream without awaiting each response. Responses arrive asynchronously, keyed by sequence_number. Multiple in-flight batches at once.

#### RocksDB WriteBatch Sizing

"Increasing the size of the batch minimizes the substantial overhead of per-batch operations." Optimal micro-batch size is ~100KB. For 1KB messages, that's ~100 messages per WriteBatch — Fila's `write_coalesce_max_batch: 100` default is already in the right range.
_Source: [Optimizing Bulk Load in RocksDB](https://rockset.com/blog/optimizing-bulk-load-in-rocksdb/)_

For larger throughput targets, increasing `write_coalesce_max_batch` to 1000 with 1KB messages (1MB batch) aligns with RocksDB's 1MB group commit maximum.
_Source: [RocksDB WAL Performance](https://github.com/facebook/rocksdb/wiki/WAL-Performance)_

### Testing Strategy

#### Benchmark-Driven Development

Each tier must be validated by the existing benchmark suite before proceeding:

1. **fila-bench self-benchmarks:**
   - `enqueue_throughput_1kb` — primary metric
   - `multi_producer_throughput` — parallel scaling
   - `e2e_latency_p50/p95/p99` — latency regression check

2. **Subsystem benchmarks** (`FILA_BENCH_SUBSYSTEM=1`):
   - `grpc_overhead` — transport layer isolation
   - `rocksdb_write` — storage isolation (should remain constant)

3. **New benchmarks needed:**
   - `batch_enqueue_throughput(batch_size)` — vary batch size 1/10/100/1000
   - `stream_batch_throughput` — pipelined streaming with batches
   - `single_client_batch_vs_multi_client` — verify batch path matches parallel scaling

4. **Competitive benchmarks** (`bench/competitive/`):
   - Re-run after each tier with identical parameters
   - Track Fila-to-Kafka ratio as the primary success metric

#### Regression Tests

Each tier adds tests for:
- Batch with 1 message (degenerate case = existing behavior)
- Batch with mixed success/failure (some queues exist, some don't)
- Batch with messages for different queues (routing correctness)
- Batch under concurrent load (coalescing + batching interaction)
- Stream batch with client disconnect mid-batch (error handling)

### Risk Assessment and Mitigation

| Risk | Impact | Likelihood | Mitigation |
|------|--------|-----------|------------|
| Tier 0 doesn't improve throughput | High | Low | Sequential processing is measured at 328 msg/s = 3ms × N. Eliminating N round-trips is mathematically sound. |
| Batch routing breaks queue ordering | High | Medium | All messages for one queue go to one shard. Within-shard, enqueue order = insertion order in Vec. Add ordering tests. |
| crossbeam channel blocking under batch load | Medium | Medium | Monitor tokio thread starvation. Consider `spawn_blocking` for channel sends, or migrate to `tokio::sync::mpsc`. |
| Protobuf batch message too large | Low | Low | Cap batch size at proto level (e.g., 10K messages or 10MB). Return error for oversized batches. |
| Raft cluster mode incompatible with batch | Medium | Low | `ClusterRequest::Enqueue` already wraps a single message. Add `ClusterRequest::EnqueueBatch` for cluster-mode batching. Separate concern from Tier 0. |
| Lua hooks don't compose with batching | Medium | Medium | Tier 0 calls Lua per-message (no change). Tier 2 adds batch Lua API — test Lua scripts with batch input before deploying. |

### Implementation Roadmap

| Phase | Scope | Validation | Duration Estimate |
|-------|-------|-----------|-------------------|
| **Tier 0a** | `SchedulerCommand::EnqueueBatch` + scheduler handler | Unit tests, existing bench suite | 1 story |
| **Tier 0b** | service.rs `batch_enqueue()` + `stream_enqueue()` refactor | Integration tests, batch bench | 1 story |
| **Profile** | Flamegraph + subsystem benchmarks post-Tier 0 | Identify new bottleneck | 1 story |
| **Tier 1a** | Proto `StreamBatchEnqueue` + server handler | Stream batch bench | 1 story |
| **Tier 1b** | SDK `BatchConfig` + client-side accumulator | SDK integration tests | 1 story |
| **Profile** | Flamegraph + competitive benchmarks post-Tier 1 | Identify new bottleneck | 1 story |
| **Tier 2** | Store-as-received, batch Lua, batch DRR | Full bench suite | 1-2 stories |
| **Tier 3** | `shard_count` auto-config + bench tuning | Multi-queue benchmarks | 1 story |

**Total: 7-9 stories across 2-3 epics.** Profile checkpoints between tiers enforce the "profile-first" rule.

### Success Metrics and KPIs

| Metric | Current | Tier 0 Target | Tier 1 Target | Kafka Parity |
|--------|---------|---------------|---------------|-------------|
| Single-client batch 1KB | 328 msg/s | 30K+ msg/s | 100K+ msg/s | N/A |
| Multi-producer 4x (1KB) | 22K msg/s | 100K+ msg/s | 300K+ msg/s | 422K msg/s |
| Competitive benchmark ratio | 0.05x Kafka | 0.24x | 0.71x | 1.0x |
| e2e latency p99 (light) | 0.23 ms | ≤0.5 ms | ≤1.0 ms | — |
| Batch enqueue throughput | N/A | 30K+ msg/s | 200K+ msg/s | — |

_Targets are estimates based on profiling data. Profile after each tier to set realistic next-tier targets._

## Research Synthesis

### Executive Summary

Fila's 19x throughput gap to Kafka (22K vs 422K msg/s) has a single root cause: **the gRPC handlers process messages sequentially**, awaiting a full scheduler round-trip (~3ms) per message even in batch and streaming paths. The scheduler's write coalescing infrastructure (`drain_and_coalesce()`, `WriteBatch`, batch DRR delivery) already exists but is starved because handlers feed messages one-at-a-time. This is not a storage problem (RocksDB has 20x headroom at 160K ops/s), not a serialization problem (protobuf at 12,335 MB/s), and not fundamentally a gRPC problem — it's a handler-to-scheduler bridge problem.

The fix is a tiered strategy: (Tier 0) batch commands to scheduler, (Tier 1) batch-aware gRPC protocol with client-side accumulation, (Tier 2) eliminate per-message CPU work, (Tier 3) horizontal sharding. Tier 0 alone — ~255 lines of additive code — is projected to yield 10-50x improvement on batch paths. The codebase's existing batch infrastructure means most core methods (`prepare_enqueue`, `apply_mutations`, `finalize_enqueue`, DRR delivery) require zero changes.

**Key Technical Findings:**

1. **Sequential processing is the measured bottleneck.** `BatchEnqueue` processes 1000 messages in 1000 × 3ms = 3 seconds = 328 msg/s, matching the measured 328 msg/s exactly. `StreamEnqueue` drains 256 messages from the stream but then processes them one-at-a-time with sequential awaits.

2. **The scheduler is already batch-optimized.** `flush_coalesced_enqueues()` collects up to 100 enqueues, prepares per-message, commits one `WriteBatch`, finalizes per-message, delivers per-queue. It just never receives batches from the gRPC layer.

3. **Kafka's advantage is end-to-end batch propagation, not a better storage engine.** The broker writes RecordBatch blobs as-is — no per-message deserialization, no per-message processing. Fila can't fully adopt this pattern (fairness scheduling requires per-message routing), but can batch the scheduler intake and amortize the round-trip.

4. **gRPC streaming is not a silver bullet without batch-within-stream.** Individual `StreamEnqueueRequest` messages still incur per-message HTTP/2 DATA frame overhead. The optimized pattern uses `repeated` fields inside stream messages.

5. **Thread-per-core and io_uring are NOT required for 400K msg/s.** The scheduler is already single-threaded-per-shard (LMAX pattern). Tokio is correct for the I/O layer. The bottleneck is the handler→scheduler bridge, not syscall overhead.

6. **RocksDB and the storage layer are not bottlenecks.** 160K ops/s at 1KB with 6.2µs per write. Append-only log migration would require rebuilding fairness scheduling, DRR state, and consume-by-key semantics for negligible gain.

**Top Recommendations:**

1. **Implement Tier 0 immediately** — `SchedulerCommand::EnqueueBatch` + handler refactor. Highest confidence, highest impact, lowest effort.
2. **Profile after Tier 0** before designing Tier 1. The new bottleneck will be visible and may differ from predictions.
3. **Increase default `shard_count`** to CPU core count for multi-queue workloads. Free scaling, config-only change.
4. **Do NOT pursue** custom binary protocol, io_uring migration, or append-only storage at this stage. Re-evaluate only after Tiers 0-2 are profiled.

### Table of Contents

1. [Technical Research Scope Confirmation](#technical-research-scope-confirmation)
2. [Technology Stack Analysis](#technology-stack-analysis)
   - The Root Cause: Sequential Message Processing
   - Fila's Server Architecture
   - How Kafka Achieves 422K msg/s
   - How NATS Achieves 138K msg/s
   - How Redpanda Achieves >1 GB/s per Core
   - How TiKV Optimizes RocksDB Writes
   - gRPC/tonic Performance Characteristics
   - io_uring and Thread-per-Core in Rust
3. [Integration Patterns Analysis](#integration-patterns-analysis)
   - End-to-End Batch Propagation (Kafka Model)
   - Pipelined Writes Without Per-Message Acks (NATS Model)
   - gRPC Streaming as Batch Transport
   - RocksDB Group Commit and Pipelined Writes
   - Async/Sync Channel Bridge
   - Fsync Strategy
4. [Architectural Patterns and Design](#architectural-patterns-and-design)
   - Single Writer with Batch Draining (LMAX Model)
   - Thread-per-Core vs Work-Stealing
   - Append-Only Log vs LSM Tree
   - Scheduler Sharding
   - Tiered Optimization Strategy
5. [Implementation Approaches](#implementation-approaches-and-technology-adoption)
   - Tier 0: Batch Commands to Scheduler
   - Tier 1: Batch-Aware gRPC Protocol
   - Testing Strategy
   - Risk Assessment
   - Implementation Roadmap
   - Success Metrics and KPIs

### Source Documentation

**Primary Sources — Codebase Analysis:**
- `crates/fila-server/src/service.rs` — gRPC handlers (enqueue, batch_enqueue, stream_enqueue)
- `crates/fila-core/src/broker/scheduler/mod.rs` — scheduler event loop, drain_and_coalesce
- `crates/fila-core/src/broker/scheduler/handlers.rs` — prepare_enqueue, finalize_enqueue, flush_coalesced_enqueues
- `crates/fila-core/src/broker/mod.rs` — broker routing, shard management
- `crates/fila-core/src/storage/rocksdb.rs` — apply_mutations, WriteBatch
- `crates/fila-core/src/broker/command.rs` — SchedulerCommand enum

**Primary Sources — Profiling Data:**
- `_bmad-output/planning-artifacts/research/post-optimization-profiling-analysis-2026-03-24.md` — flamegraph analysis, subsystem benchmarks, competitive comparison

**External Sources:**
- [Kafka Batch Processing for Efficiency](https://docs.confluent.io/kafka/design/efficient-design.html)
- [Kafka Protocol](https://kafka.apache.org/41/design/protocol/)
- [Kafka Performance Tuning: linger.ms and batch.size](https://www.automq.com/blog/kafka-performance-tuning-linger-ms-batch-size)
- [How Kafka Is so Performant If It Writes to Disk?](https://andriymz.github.io/kafka/kafka-disk-write-performance/)
- [What is Zero Copy in Kafka?](https://www.nootcode.com/knowledge/en/kafka-zero-copy)
- [NATS Client Protocol](https://docs.nats.io/reference/reference-protocols/nats-protocol)
- [NATS JetStream](https://docs.nats.io/nats-concepts/jetstream)
- [NATS FAQ](https://docs.nats.io/reference/faq)
- [NATS Server Architecture](https://github.com/nats-io/nats-general/blob/main/architecture/ARCHITECTURE.md)
- [What makes Redpanda fast?](https://www.redpanda.com/blog/what-makes-redpanda-fast)
- [Redpanda Architecture](https://docs.redpanda.com/current/get-started/architecture/)
- [Redpanda vs Apache Kafka](https://www.automq.com/blog/redpanda-vs-apache-kafka-event-streaming)
- [TiKV gRPC](https://tikv.org/deep-dive/rpc/grpc/)
- [Deep Dive TiKV](https://tikv.github.io/deep-dive-tikv/print.html)
- [TiKV RocksDB Write Batch Optimization](https://medium.com/@siddontang/how-we-optimize-rocksdb-in-tikv-write-batch-optimization-28751a4bdd8b)
- [TiKV Smarter Flow Control](https://medium.com/@siddontang/how-we-optimize-rocksdb-in-tikv-smarter-flow-control-f6af95cbf87e)
- [Comparing gRPC performance across different technologies](https://nexthink.com/blog/comparing-grpc-performance)
- [The Mysterious Gotcha of gRPC Stream Performance](https://ably.com/blog/grpc-stream-performance)
- [gRPC-Rust Successor](https://tldrecap.tech/posts/2025/grpconf-india/grpc-rust-successor/)
- [gRPC Streaming Best Practices](https://dev.to/ramonberrutti/grpc-streaming-best-practices-and-performance-insights-219g)
- [gRPC streaming vs repeated fields](https://learn.microsoft.com/en-us/dotnet/architecture/grpc-for-wcf-developers/streaming-versus-repeated)
- [Introducing Glommio](https://www.datadoghq.com/blog/engineering/introducing-glommio/)
- [Monoio Benchmark](https://github.com/bytedance/monoio/blob/master/docs/en/benchmark.md)
- [RocksDB Pipelined Write](https://github.com/facebook/rocksdb/wiki/Pipelined-Write)
- [RocksDB WAL Performance](https://github.com/facebook/rocksdb/wiki/WAL-Performance)
- [Optimizing Bulk Load in RocksDB](https://rockset.com/blog/optimizing-bulk-load-in-rocksdb/)
- [The LMAX Architecture](https://martinfowler.com/articles/lmax.html)
- [Thread-per-core](https://without.boats/blog/thread-per-core/)
- [LSM Tree Explained](https://aerospike.com/blog/log-structured-merge-tree-explained/)
- [RabbitMQ Sharding](https://github.com/rabbitmq/rabbitmq-sharding)
- [Kafka Fastest Messaging System](https://www.confluent.io/blog/kafka-fastest-messaging-system/)
- [Crossfire Crate](https://crates.io/crates/crossfire)
- [Rust Channel Comparison](https://users.rust-lang.org/t/differences-between-channel-in-tokio-mpsc-and-crossbeam/92676)

### Confidence Assessment

| Finding | Confidence | Basis |
|---------|-----------|-------|
| Sequential processing is the root cause | **VERY HIGH** | Measured: 328 msg/s = 1000/3ms. Code trace confirms sequential `.await` per message. |
| Tier 0 will improve batch throughput 10-50x | **HIGH** | Mathematical: eliminates N × 3ms round-trips. Scheduler batch infra already works. |
| RocksDB is not the bottleneck | **VERY HIGH** | Measured: 160K ops/s, 6.2µs/write. 20x above current throughput. |
| gRPC streaming + batch can reach 300K+ msg/s | **MEDIUM** | Based on: h2 50% CPU at 7.8K msg/s with unary. Streaming eliminates per-msg stream setup. Extrapolation. |
| Thread-per-core is not required for 400K msg/s | **MEDIUM-HIGH** | Based on: bottleneck is handler→scheduler bridge, not I/O. TiKV achieves 100K+ ops/s with tokio+gRPC. |
| Append-only log is not justified | **HIGH** | Based on: Fila's fairness scheduling requires indexed access by (queue_id, fairness_key). RocksDB has 20x headroom. |
| Full path to 400K+ msg/s via Tiers 0-3 | **MEDIUM** | Tier 0 is high confidence. Each subsequent tier depends on profiling the previous one. Projected numbers are estimates. |

### Limitations

1. **Throughput projections are estimates.** Each tier changes the bottleneck. Actual numbers require profiling after implementation. The 10-50x Tier 0 projection is derived from round-trip elimination, but real-world behavior includes GC pressure, tokio scheduling, and crossbeam contention that are not modeled.

2. **Single-queue scaling is unaddressed.** Scheduler sharding helps multi-queue workloads but not single-queue. Single-queue throughput at 400K+ msg/s may require intra-queue partitioning (by fairness_key), which is architecturally complex.

3. **Cluster mode is out of scope.** Raft consensus adds latency per write. Batch Raft proposals (like TiKV) are needed but are a separate concern. This research focuses on single-node bottlenecks.

4. **Competitive benchmark methodology.** Kafka's 422K msg/s uses `linger.ms=5, batch.size=1MB` (client-side batching). Fila's 22K uses independent producers with no client-side batching. A fair Fila-with-batching comparison requires Tier 1 SDK changes.

5. **External SDK impact.** Tier 1 requires changes to all 6 SDKs (Rust, Go, Python, JS, Ruby, Java) for client-side batching. This multiplies implementation effort.

---

**Technical Research Completion Date:** 2026-03-24
**Research Period:** Single-session comprehensive technical analysis
**Source Verification:** All technical claims cited with current sources
**Technical Confidence Level:** High — based on measured profiling data, codebase analysis, and multiple authoritative external sources

_This research document serves as the technical foundation for epic planning to close the throughput gap to Kafka parity. The primary recommendation — implement `SchedulerCommand::EnqueueBatch` (Tier 0) — is high-confidence, low-risk, and should be the first story in the performance epic._
