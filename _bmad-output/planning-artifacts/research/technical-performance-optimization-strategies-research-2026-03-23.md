---
stepsCompleted: [1, 2, 3, 4, 5, 6]
inputDocuments:
  - docs/benchmarks.md
  - bench/competitive/results/bench-*.json
  - bench/competitive/METHODOLOGY.md
  - crates/fila-core/src/storage/rocksdb.rs
  - crates/fila-core/src/broker/scheduler/
  - _bmad-output/planning-artifacts/research/technical-benchmarking-methodology-research-2026-03-23.md
workflowType: 'research'
lastStep: 6
research_type: 'technical'
research_topic: 'performance optimization strategies for Fila'
research_goals: 'identify bottlenecks from benchmark data, evaluate optimization strategies, prioritize by impact, assess FR66 (purpose-built storage engine)'
user_name: 'Lucas'
date: '2026-03-23'
web_research_enabled: true
source_verification: true
---

# Research Report: Performance Optimization Strategies for Fila

**Date:** 2026-03-23
**Author:** Lucas
**Research Type:** Technical

---

## Executive Summary

Fila's single-node throughput sits at **2.7K msg/s** (64B) and **2.3K msg/s** (1KB) — roughly **500x behind Kafka** and **140x behind NATS** at 64B, and **60x behind Kafka** and **60x behind NATS** at 1KB. However, Fila's **latency is excellent**: p50 of 0.40ms and p99 of 0.51ms, beating Kafka by 250x and RabbitMQ by 3x, trailing only NATS (by ~1.4x). On lifecycle throughput (the full enqueue→consume→ack cycle), Fila places second at 2,393 msg/s — 6.7x ahead of RabbitMQ and 6.7x ahead of Kafka, behind only NATS (10.7x faster).

**The throughput gap is the dominant problem.** Fila's architecture creates per-message overhead that doesn't amortize: one gRPC call → one protobuf decode → one RocksDB key-value write → one protobuf encode to storage. Competitors amortize this via batching (Kafka: `linger.ms` + `batch.size`), append-only sequential I/O (Kafka/NATS: no LSM overhead), and zero-copy transfers (Kafka: `sendfile(2)`).

This report identifies **12 optimization opportunities**, prioritized into 4 tiers. The top 3 interventions — client-side batching, server-side write coalescing, and RocksDB tuning — could realistically take Fila from 2.7K to **30K–100K msg/s** without architectural changes. Closing the remaining gap to NATS/Kafka territory (400K–1.4M) would require replacing RocksDB with an append-only storage engine (FR66), which is a major undertaking better deferred until tuning headroom is exhausted.

---

## Table of Contents

1. [Current Performance Profile](#1-current-performance-profile)
2. [Bottleneck Analysis](#2-bottleneck-analysis)
3. [Optimization Strategies](#3-optimization-strategies)
4. [Competitor Architecture Comparison](#4-competitor-architecture-comparison)
5. [FR66: Purpose-Built Storage Engine Assessment](#5-fr66-purpose-built-storage-engine-assessment)
6. [Prioritized Optimization Roadmap](#6-prioritized-optimization-roadmap)
7. [Sources](#7-sources)

---

## 1. Current Performance Profile

### 1.1 Self-Benchmark Data (CI, commit `d2cb526`)

Data from the latest successful `bench-regression` CI run (3-run median aggregation):

| Metric | Value | Unit | Notes |
|--------|------:|------|-------|
| **Enqueue throughput (1KB)** | 2,724 | msg/s | Single producer, 3s window |
| **Enqueue throughput (1KB)** | 2.66 | MB/s | |
| **E2E latency p50** | 0.40 | ms | Light load (1 producer) |
| **E2E latency p95** | 0.45 | ms | |
| **E2E latency p99** | 0.51 | ms | |
| **E2E latency p99.99** | 0.86 | ms | |
| **FIFO throughput** | 1,433 | msg/s | Baseline without DRR |
| **Fair scheduling throughput** | 1,385 | msg/s | DRR enabled |
| **Fairness overhead** | 3.3% | | DRR vs FIFO delta |
| **Lua overhead** | 26.1 | us/msg | on_enqueue hook |
| **Lua throughput** | 1,148 | msg/s | With hook enabled |
| **Key cardinality 10** | 1,663 | msg/s | |
| **Key cardinality 1K** | 848 | msg/s | 49% drop from 10 keys |
| **Key cardinality 10K** | 506 | msg/s | 70% drop from 10 keys |
| **Consumer concurrency 1** | 320 | msg/s | |
| **Consumer concurrency 10** | 1,928 | msg/s | 6x scaling |
| **Consumer concurrency 100** | 1,874 | msg/s | Plateaus at ~10 consumers |
| **Memory RSS idle** | 268 | MB | RocksDB buffer pool dominated |
| **Memory per message** | 50.8 | bytes/msg | |
| **Compaction p99 delta** | 0.21 | ms | Active vs idle compaction |

### 1.2 Competitive Comparison Data (commit `7dfa7b4`)

#### Throughput (msg/s)

| Payload | Fila | Kafka | RabbitMQ | NATS | Fila vs Kafka | Fila vs NATS |
|---------|-----:|------:|---------:|-----:|--------------|-------------|
| 64B | 2,758 | 1,473,379 | 36,141 | 394,950 | 534x behind | 143x behind |
| 1KB | 2,326 | 143,278 | 38,321 | 137,748 | 62x behind | 59x behind |
| 64KB | 296 | 2,335 | 2,379 | 2,426 | 8x behind | 8x behind |

**Key observation:** The throughput gap narrows dramatically at 64KB (only 8x). This proves the bottleneck is **per-message overhead**, not raw I/O bandwidth. At 64KB, the I/O cost dominates and all brokers converge.

#### Latency (1KB, ms)

| Percentile | Fila | Kafka | RabbitMQ | NATS |
|-----------|-----:|------:|---------:|-----:|
| p50 | 0.46 | 101.62 | 1.46 | 0.29 |
| p95 | 0.59 | 105.07 | 3.32 | 0.42 |
| p99 | 1.02 | 105.30 | 5.59 | 0.79 |

**Fila wins on latency.** 220x better than Kafka (which batches — latency is the cost of `linger.ms`). 3x better than RabbitMQ. Only 1.6x behind NATS at p50.

#### Lifecycle Throughput (enqueue + consume + ack, 1KB)

| Broker | msg/s | vs Fila |
|--------|------:|---------|
| NATS | 25,763 | 10.8x faster |
| **Fila** | **2,393** | baseline |
| RabbitMQ | 658 | 3.6x slower |
| Kafka | 356 | 6.7x slower |

**Fila shines on lifecycle.** The full queue cycle (produce, consume, ack) is Fila's use case. Kafka's high producer throughput comes from batching, which doesn't help lifecycle. NATS remains the benchmark to chase.

#### Resource Usage

| Broker | CPU | Memory |
|--------|----:|-------:|
| NATS | 1.3% | 12 MB |
| Kafka | 2.1% | 1,276 MB |
| **Fila** | **23.1%** | **107 MB** |
| RabbitMQ | 56.8% | 654 MB |

**Fila's CPU usage is high relative to throughput.** At 2.7K msg/s consuming 23% CPU, Fila processes ~117 msg/s per CPU percent. NATS processes ~303K msg/s at 1.3% CPU — about 2,600x more efficient per CPU unit. This points to per-message CPU overhead (protobuf ser/deser, key construction, DRR scheduling) rather than I/O being the bottleneck.

### 1.3 Notable Patterns

**Consumer concurrency plateaus at ~10 consumers.** Going from 1→10 consumers gives 6x throughput gain (320→1,928 msg/s). Going from 10→100 gives no gain (1,928→1,874). The bottleneck shifts from consumer-side to scheduler-side at 10 consumers.

**Key cardinality degrades linearly.** 10→1K keys drops throughput 49%. 10→10K drops 70%. The DRR scheduler's `next_key()` is O(# active keys), scanning linearly for the first key with positive deficit. At 10K keys this scan becomes the dominant cost.

**Lua overhead is 26μs/msg** — within the 50μs NFR target but non-trivial. At 1,148 msg/s with Lua vs 1,433 without, Lua reduces throughput by 20%.

---

## 2. Bottleneck Analysis

### 2.1 Architecture of the Hot Path

```
gRPC request (tonic HTTP/2)
  → protobuf decode (prost)
  → crossbeam channel send → scheduler thread
  → UUID v7 generation
  → Lua on_enqueue (if configured)
  → QueueRouter key assignment
  → protobuf encode → RocksDB put_cf (messages CF)
  → msg_index put_cf
  → DRR add_key (in-memory HashMap)
  → pending index push (VecDeque)
  → response via crossbeam channel
```

For **delivery** (consume):
```
DRR next_key() → O(# active keys) scan
  → pending index peek/pop (VecDeque)
  → RocksDB get_cf (messages CF) → protobuf decode
  → RocksDB WriteBatch: put lease + put lease_expiry
  → tokio mpsc send → gRPC stream response
```

### 2.2 Identified Bottlenecks (Ranked by Impact)

#### B1: No Request Batching (Critical — ~10-50x potential)

Every gRPC call processes exactly one message. The per-message overhead stack:
- 1 HTTP/2 frame decode
- 1 protobuf deserialize
- 1 crossbeam channel hop (gRPC thread → scheduler thread)
- 1 UUID generation
- 1 protobuf serialize (to storage)
- 1 RocksDB `put_cf` (messages CF)
- 1 RocksDB `put_cf` (msg_index CF)
- 1 crossbeam channel hop back (scheduler → gRPC thread)
- 1 protobuf serialize (response)
- 1 HTTP/2 frame encode

Kafka amortizes this: `linger.ms=5` + `batch.size=16KB` means one network round-trip carries ~16 messages (at 1KB). One disk write carries the entire batch. **The cost per message drops by the batch size factor.**

**Evidence:** The 534x gap at 64B (where per-message overhead dominates) vs 8x gap at 64KB (where I/O dominates) proves batching is the primary throughput lever.

#### B2: RocksDB Default Configuration (High — ~3-10x potential)

The current `RocksDbEngine::open()` uses entirely stock defaults:
- **No block cache sizing** — RocksDB default is ~8MB shared LRU
- **No bloom filters** — every point lookup (ack, nack, get_message) potentially checks multiple SST levels
- **No write buffer tuning** — default 64MB, 2 buffers → frequent flushes
- **No compression strategy** — snappy everywhere, including L0 where CPU matters
- **No compaction tuning** — no `CompactOnDeletionCollector` despite queue's delete-heavy pattern
- **No pipelined writes** — sequential WAL + memtable writes

This is the lowest-effort, highest-certainty optimization. RocksDB's own wiki has a page titled ["Implement Queue Service Using RocksDB"](https://github.com/facebook/rocksdb/wiki/Implement-Queue-Service-Using-RocksDB) that directly addresses Fila's access pattern.

#### B3: Single-Threaded Scheduler (High — ~2-5x potential at high concurrency)

The scheduler runs on a single dedicated OS thread (`thread::Builder::new()` in `broker/mod.rs`). All queue operations — enqueue, deliver, ack, nack, admin — serialize through one `crossbeam_channel::bounded`. At 10+ consumers, the scheduler becomes the bottleneck (evidenced by consumer concurrency plateauing at 10).

The DRR loop, Lua execution, RocksDB writes, and response dispatching all compete for this single thread. A per-queue or sharded scheduler would allow independent queues to process in parallel.

#### B4: DRR Key Scan at High Cardinality (Medium — relevant at >1K keys)

`next_key()` in `drr.rs` does a linear scan of `active_keys: VecDeque<String>` to find the first key with positive deficit. At 10K keys, this O(n) scan per delivery dominates scheduling cost (throughput drops from 1,663 to 506 msg/s — a 3.3x degradation).

Replacing the linear scan with a data structure that tracks "which keys have positive deficit" (e.g., a priority queue or a bitmap of eligible keys) would make this O(1) or O(log n).

#### B5: Per-Message Protobuf Serialization (Medium — ~1.5-2x potential)

Every enqueue:
1. Deserializes the incoming protobuf request
2. Constructs a domain `Message`
3. Re-serializes to protobuf for RocksDB storage

Every delivery:
1. Reads raw bytes from RocksDB
2. Deserializes protobuf to domain `Message`
3. Constructs a gRPC response (re-serializes to protobuf)

If the stored format matched the wire format, the enqueue path could skip step 2-3 and store the already-serialized bytes directly. Delivery could skip step 2-3 and forward the raw bytes.

#### B6: gRPC/HTTP/2 Default Flow Control (Low-Medium — ~1.5-2x for streaming)

Tonic uses default HTTP/2 flow control settings:
- `initial_stream_window_size`: 64KB (default)
- `initial_connection_window_size`: 64KB (default)

For high-throughput consumer streams, the 64KB window creates unnecessary `WINDOW_UPDATE` round-trips. Setting these to 1-2MB would reduce flow control overhead for streaming consumers.

#### B7: No Delivery Batching (Low-Medium — ~1.5-3x for consumers)

Each consumed message is sent as a separate gRPC stream response. Batching multiple messages per response frame would amortize HTTP/2 framing and protobuf encoding overhead.

#### B8: CPU Overhead from Key Construction (Low — but constant)

Storage keys are constructed by string formatting with length-prefixed components (`keys.rs`). At 2.7K msg/s this is negligible, but at target throughputs (50K+), string allocation and formatting for every key becomes measurable.

---

## 3. Optimization Strategies

### 3.1 Client-Side Batching (SDK Layer)

**What:** Add `linger_ms` and `batch_size` parameters to the Fila SDK. The SDK accumulates messages in a per-queue buffer and sends them as a batch when either threshold is reached.

**API change:** Add a `BatchEnqueue` RPC that accepts `repeated EnqueueRequest` and returns `repeated EnqueueResponse`. The SDK transparently uses this when batching is configured.

**Expected impact:** 10-50x throughput improvement for small messages (64B-1KB). The improvement comes from:
- N messages per gRPC round-trip instead of 1
- N messages per RocksDB WriteBatch instead of individual puts
- Amortized protobuf framing, HTTP/2 frames, and crossbeam channel hops

**Kafka's model:** `batch.size=16KB` (byte limit), `linger.ms=5` (time limit). Whichever triggers first flushes the batch. This is the gold standard.

**Effort:** Medium (2-3 weeks). Proto changes, server-side batch handler, SDK accumulator, tests.

**Risk:** Low. Additive change. Single-message path unchanged. Latency increases by `linger_ms` for batched path.

### 3.2 Server-Side Write Coalescing

**What:** Buffer concurrent enqueue requests in the scheduler for a brief window (e.g., 1ms or 100 requests, whichever comes first) and combine them into a single `apply_mutations` call.

**Current state:** Each enqueue triggers its own `apply_mutations` → `WriteBatch` → RocksDB commit. While `WriteBatch` is atomic, each batch is a single message. RocksDB internally does write leader election (the first concurrent writer becomes leader and coalesces followers), but this only works when writes arrive simultaneously.

**How:** Replace the current "process one command, respond, process next" loop with a "drain N commands from channel, process batch, respond to all N" loop. The channel already exists (`crossbeam_channel::bounded`).

**Expected impact:** 3-5x throughput improvement. Reduces RocksDB WAL syncs by the batch factor. Combined with `enable_pipelined_write=true`, could push RocksDB writes to ~30K-50K/s.

**Effort:** Medium (1-2 weeks). Scheduler loop refactor, batched response dispatch.

**Risk:** Medium. Changes the scheduler's event loop. Must handle partial batch failures correctly. Increases latency by the coalescing window.

### 3.3 RocksDB Configuration Tuning

**What:** Apply queue-optimized RocksDB settings. Currently every option is default.

**Recommended settings by column family:**

#### DB-Wide

| Setting | Current | Recommended | Rationale |
|---------|---------|-------------|-----------|
| Block cache | ~8MB (default) | 256MB shared LRU | Cache uncompressed data blocks. Use `LRUCache` with 6 shard bits to reduce lock contention. |
| `cache_index_and_filter_blocks` | false | true | Keep bloom filters and index blocks in managed cache (predictable memory) |
| `pin_l0_filter_and_index_blocks` | false | true | L0 files are read most frequently; pinning avoids eviction |
| `enable_pipelined_write` | false | true | WAL and memtable writes run in parallel. **Measured 30% throughput improvement** in RocksDB benchmarks. |
| `manual_wal_flush` | false | true | Buffer WAL in memory, flush periodically. MyRocks measured **40% throughput improvement**. Safe when Raft provides durability. |
| `wal_bytes_per_sync` | 0 | 512KB | Periodic WAL sync to avoid burst I/O |
| `max_total_wal_size` | 0 (unlimited) | 256MB | Limits WAL accumulation |

#### Messages CF (write-heavy, read-once, delete-heavy)

| Setting | Current | Recommended | Rationale |
|---------|---------|-------------|-----------|
| `write_buffer_size` | 64MB | 128MB | Larger buffers → fewer flushes |
| `max_write_buffer_number` | 2 | 4 | Queue 3 buffers for flush while 1 is active |
| `min_write_buffer_number_to_merge` | 1 | 2 | Merge memtables before flush, reducing L0 duplicates |
| Bloom filter | none | 10 bits/key, full bloom | Speeds up point lookups for ack/nack |
| `memtable_prefix_bloom_size_ratio` | 0 | 0.1 | 10% of write buffer for memtable bloom |
| Compression L0-L1 | snappy | none | Hot data, recently written — CPU saved > space cost |
| Compression L2+ | snappy | LZ4 | Better speed/ratio than snappy |
| `CompactOnDeletionCollector` | disabled | enabled, ratio 0.5 | **Critical for queue pattern.** Prioritizes compaction of tombstone-heavy SST ranges. RocksDB wiki explicitly recommends this for queue workloads. |
| `iterate_upper_bound` | not set | set on all prefix scans | Prevents iterator from walking past tombstones into unrelated key ranges |

#### Leases CF (short-lived, high churn)

Same as messages but with 64MB write buffer (smaller scale).

#### Lease Expiry CF (scan-heavy)

| Setting | Recommended | Rationale |
|---------|-------------|-----------|
| Bloom filter | **disabled** | Range scans don't use bloom filters |
| Compression | none | Small volume, fast access needed |

#### Raft Log CF (append-only, periodic truncation)

| Setting | Recommended | Rationale |
|---------|-------------|-----------|
| `CompactOnDeletionCollector` | enabled | Log truncation creates tombstone-heavy regions |
| Write buffer | 64MB | Moderate write volume |

**Expected impact:** 3-10x aggregate throughput improvement from the combination of pipelined writes (1.3x), manual WAL flush (1.4x), proper bloom filters (fewer disk reads for ack/nack), CompactOnDeletionCollector (reduced tombstone scanning), and larger write buffers (fewer flush stalls).

**Effort:** Low (2-3 days). Configuration-only change to `RocksDbEngine::open()`. No API changes.

**Risk:** Very low. All settings are well-documented RocksDB options used in production at scale (TiKV, CockroachDB, EMQX).

### 3.4 Delivery Batching (Consumer-Side)

**What:** Send multiple messages per gRPC streaming response instead of one.

**Current state:** Each consumed message is sent as a separate `StreamConsumeResponse` on the gRPC stream. This means one HTTP/2 DATA frame + one protobuf-encoded response per message.

**How:** Buffer delivered messages in the consumer stream handler and flush when either a count threshold (e.g., 10 messages) or a time threshold (e.g., 1ms) is reached. Proto change: add `repeated Message messages` to the response.

**Expected impact:** 1.5-3x consumer throughput improvement. Amortizes HTTP/2 framing and tokio mpsc channel overhead.

**Effort:** Low-Medium (1 week). Proto change, stream handler buffering, SDK consumer batch processing.

### 3.5 gRPC Tuning

**What:** Configure tonic's HTTP/2 flow control and keepalive settings.

```rust
Server::builder()
    .initial_stream_window_size(2 * 1024 * 1024)      // 2MB (default: 64KB)
    .initial_connection_window_size(4 * 1024 * 1024)   // 4MB (default: 64KB)
    .http2_keepalive_interval(Some(Duration::from_secs(15)))
    .http2_keepalive_timeout(Some(Duration::from_secs(10)))
    .tcp_nodelay(true)
```

**Expected impact:** 1.5-2x for streaming consumers. Reduces `WINDOW_UPDATE` back-and-forth. Keepalive prevents idle connection drops through proxies/load balancers.

**Effort:** Low (hours). Add builder methods to server setup.

**Risk:** Very low. Standard production settings.

### 3.6 Zero-Copy Protobuf Passthrough

**What:** Use `bytes::Bytes` for message payload fields instead of `Vec<u8>`. Store already-serialized protobuf bytes in RocksDB instead of deserializing and re-serializing.

**How:**
1. Configure prost to use `Bytes` for payload fields (`bytes = "bytes"` in build.rs)
2. For the enqueue path: skip domain type conversion when the message body is opaque — store the raw protobuf bytes from the gRPC request directly
3. For the delivery path: read raw bytes from RocksDB and forward to gRPC response without full deserialization — only parse the fields needed for DRR (fairness key, headers) using partial protobuf parsing

**Expected impact:** 1.2-1.5x throughput. Eliminates 2 full protobuf encode/decode cycles per message for opaque payloads. More impactful for larger messages.

**Effort:** Medium (1-2 weeks). Requires careful refactoring of the `Message` domain type and storage layer.

### 3.7 DRR Scheduler Data Structure Optimization

**What:** Replace the O(# active keys) linear scan in `next_key()` with an O(1) or O(log n) structure.

**Options:**
1. **Eligible key bitmap:** Maintain a secondary set of keys with positive deficit. `next_key()` pops from this set. `consume_deficit()` removes keys when deficit reaches zero. `replenish_deficits()` rebuilds the set. O(1) per delivery.
2. **Priority queue (BinaryHeap):** Order keys by deficit. `next_key()` peeks the max-deficit key. O(log n) per delivery.
3. **Tiered VecDeque:** Split active keys into "has deficit" and "exhausted" queues. O(1) per delivery, O(n) per round.

**Expected impact:** Throughput at 10K keys would improve from 506 to ~1,500 msg/s (matching 10-key performance). Negligible impact at low key counts.

**Effort:** Low (1-2 days). Data structure swap within `drr.rs`.

**Risk:** Low. Well-understood data structure change with existing property tests for validation.

### 3.8 Scheduler Sharding

**What:** Run multiple scheduler threads, each responsible for a subset of queues.

**Current state:** All queues share one scheduler thread. At 10+ concurrent consumers across multiple queues, the single thread saturates.

**How:** Hash queue names to scheduler shards. Each shard has its own crossbeam channel, DRR state, and RocksDB write path. gRPC handlers route requests to the correct shard.

**Expected impact:** Near-linear scaling with shard count for independent queues. A 4-shard scheduler could provide ~4x throughput for multi-queue workloads.

**Effort:** High (3-4 weeks). Fundamental scheduler architecture change. Must handle cross-queue operations (admin commands, stats aggregation).

**Risk:** Medium. Increases complexity. Must ensure per-queue ordering guarantees are preserved.

### 3.9 io_uring Storage I/O

**What:** Replace RocksDB's internal pread/pwrite with io_uring submission queues for storage I/O.

**Current state of Rust ecosystem:**
- **tokio-uring:** Stalled development, single-threaded, `!Sync` types. Not production-ready.
- **monoio** (ByteDance): Active, thread-per-core model, 2-3x tokio throughput at 16 cores. Requires full runtime replacement.
- **compio:** Used by Apache Iggy's thread-per-core rewrite. Achieved >5 GB/s throughput. Emerging but promising.

**Critical limitation:** io_uring is irrelevant while RocksDB is the storage engine. RocksDB manages its own I/O internally. io_uring only helps with a custom storage engine (see FR66 assessment below).

**Expected impact:** 2-3x on top of a custom storage engine. Not applicable to current architecture.

**Effort:** Very high. Requires custom storage engine + runtime migration.

**Risk:** High. Rust io_uring ecosystem is immature. Thread-per-core model (monoio/compio) is incompatible with tokio's multi-threaded runtime that tonic depends on.

### 3.10 Bytes Representation and Key Encoding

**What:** Reduce allocation overhead in the storage key construction path.

**Current state:** `keys.rs` constructs storage keys using `Vec<u8>` with string formatting and length prefixes. Each key construction allocates a new `Vec<u8>`.

**How:** Pre-allocate key buffers with `Vec::with_capacity()` based on known maximum key sizes. Use stack-allocated arrays (`[u8; MAX_KEY_LEN]`) where possible. Reuse key buffers across operations in the batch path.

**Expected impact:** Minimal at current throughput (2.7K msg/s). Becomes measurable at 50K+ msg/s where allocation overhead adds up.

**Effort:** Low (1 day). Micro-optimization.

### 3.11 Tokio Runtime Tuning

**What:** Explicitly configure tokio's multi-threaded runtime instead of using `#[tokio::main]` defaults.

**Options:**
- Set worker thread count explicitly (currently: CPU core count)
- Configure blocking thread pool size (for any `spawn_blocking` calls)
- Enable I/O and time drivers explicitly

**Expected impact:** Minimal. The scheduler already runs on a dedicated OS thread. gRPC handlers are lightweight async tasks. Tokio's defaults are already well-tuned for this workload.

**Effort:** Low (hours).

### 3.12 Connection Handling Optimization

**What:** Tune tonic's connection lifecycle for high-concurrency workloads.

**Options:**
- `tcp_nodelay(true)` — disable Nagle's algorithm (reduce small-message latency)
- Connection pooling in SDK (reuse HTTP/2 connections across queues)
- Max concurrent streams per connection

**Expected impact:** Low. Already bounded by scheduler throughput, not connection handling.

**Effort:** Low (hours).

---

## 4. Competitor Architecture Comparison

### 4.1 What Competitors Do That Fila Doesn't

| Architectural Pattern | Kafka | NATS | RabbitMQ | Fila |
|----------------------|-------|------|----------|------|
| **Append-only storage** | Yes (log segments) | Yes (flat files) | Yes (Raft log for quorum queues) | No (LSM tree / RocksDB) |
| **OS page cache reliance** | Primary cache | Yes (mmap) | No | No (RocksDB block cache) |
| **Zero-copy transfer** | Yes (`sendfile(2)`) | Partial | No | No |
| **Client-side batching** | Yes (`linger.ms`, `batch.size`) | Yes (implicit at protocol level) | Yes (publisher confirms with batching) | No |
| **Server-side write batching** | Yes (segment append is naturally batched) | Yes | Yes | Per-message only |
| **Custom wire protocol** | Kafka binary protocol | NATS text protocol | AMQP 0-9-1 | gRPC/HTTP/2 (protobuf) |
| **Multi-partition parallelism** | Yes (topic partitions) | Yes (subjects → streams) | No (queue-level) | No (queue-level) |
| **Thread-per-core / io_uring** | No (JVM) | No (Go) | No (Erlang) | No (tokio multi-threaded) |

### 4.2 Key Architectural Differences Explained

**Why Kafka is 534x faster at 64B throughput:**
1. **Batching** (~50x factor): `linger.ms=5` with `batch.num.messages=1000` means one network call carries up to 1000 messages. One disk write carries the entire batch.
2. **Sequential I/O** (~5x factor): Append-only log segments. No LSM compaction, no write amplification. One disk write per batch, sequentially.
3. **Zero-copy** (~2x factor): `sendfile(2)` for consumer reads. Data goes from page cache → NIC without entering JVM heap.
4. **Page cache** (latency factor): Recently-written messages are already in the OS page cache. Consumer reads hit cache 100% when caught up.

**Why NATS is 143x faster at 64B throughput:**
1. **Custom protocol** (~3-5x factor): NATS text protocol has minimal per-message framing overhead compared to gRPC/HTTP/2 + protobuf.
2. **Append-only file storage** (~5-10x factor): Similar to Kafka — sequential writes, no LSM overhead.
3. **Implicit batching** (~5-10x factor): NATS connection-level buffering coalesces writes automatically.
4. **Minimal per-message processing** (~2-3x factor): No DRR scheduling, no Lua hooks, no fairness key indexing.

**Why the gap narrows at 64KB (only 8x):**
At 64KB, I/O bandwidth dominates over per-message overhead. All brokers are bottlenecked by the same thing: disk write bandwidth and network throughput. The per-message overhead (protocol framing, serialization, scheduling) becomes negligible relative to the 64KB payload I/O cost.

### 4.3 What Fila Does That Competitors Don't

Fila has features that add per-message overhead but deliver unique value:
- **DRR fair scheduling** — weighted fairness across tenants (Kafka/NATS have no built-in fairness)
- **Lua scripting hooks** — programmable message routing and transformation at the broker level
- **Per-queue Raft consensus** — independent queue availability (vs Kafka's partition-level replication)
- **Fine-grained ACLs** — per-queue, per-operation access control with glob patterns

These features are load-bearing. Optimization should reduce their overhead, not remove them.

---

## 5. FR66: Purpose-Built Storage Engine Assessment

### 5.1 The Case For

Every major production message broker converged on append-only log segments:
- **Kafka:** Append-only log segments per partition, OS page cache, `sendfile(2)`
- **Pulsar/BookKeeper:** Append-only ledgers, separate WAL journal
- **RedPanda:** Custom C++ storage on Seastar, Raft + segments
- **NATS JetStream:** File-based segments with mmap

None use LSM trees for message storage. The mismatch between RocksDB's LSM design and queue access patterns creates:
- **Write amplification:** RocksDB level compaction produces 20-30x write amplification. A message written once and deleted once amplifies to dozens of disk writes across compaction levels.
- **Tombstone accumulation:** Deleted messages persist as tombstones until compaction. RocksDB's own wiki acknowledges "queries having to iterate over millions of tombstones" in queue workloads.
- **Unnecessary read amplification:** Messages consumed soon after writing should be in page cache. With RocksDB, reads potentially check memtable + multiple SST levels.
- **No segment-level GC:** An append-only log drops entire files when all messages are consumed. RocksDB must compact individual keys across levels.

### 5.2 The Case Against (Now)

**Fila's RocksDB has zero tuning applied.** Before concluding the engine is the bottleneck, the tuning headroom must be exhausted. Evidence that tuning alone can reach high throughput:
- **EMQX** (MQTT broker): 1M msg/s with RocksDB-backed persistence
- **TiKV:** Millions of operations/sec with heavily tuned RocksDB
- **Kafka Streams:** Uses RocksDB for state stores (not message storage) at high throughput

The `StorageEngine` trait already abstracts the storage layer cleanly (introduced in Epic 13). When/if a custom engine becomes necessary, swapping the implementation is structurally straightforward.

**Effort estimate for a minimal custom engine:**
- Segment file format + append path: ~2 weeks
- WAL for crash recovery: ~2 weeks
- In-memory index for point lookups: ~1 week
- Segment-level GC: ~1 week
- Fairness key index: ~1 week
- Crash recovery / startup replay: ~2 weeks
- Testing, fuzzing, edge cases: ~4 weeks
- Raft integration adjustments: ~2 weeks

**Total: 3-6 months for one experienced engineer.** This is substantially less than RedPanda (team, years) or TigerBeetle (startup, 3.5 years) because Fila's access pattern is constrained (append, read-head, delete-by-ID) and Raft already handles replication.

### 5.3 Intermediate Options

| Option | Pros | Cons | Effort |
|--------|------|------|--------|
| **RocksDB + tuning** | Immediate, low-risk, 3-10x potential | Still an LSM tree, tombstone problem remains | Days |
| **RocksDB + BlobDB** | 88-90% lower write amplification | Config only, but still LSM overhead | Days |
| **RocksDB + FIFO compaction** | Drops oldest SSTs (queue-like) | Poor fit for per-queue deletion semantics | Days |
| **LMDB** | Faster writes in benchmarks, B+tree | 2GB default map, memory-mapped (OOM risk) | 1-2 weeks |
| **redb** (pure Rust, B+tree) | No C++ dependency, 1.0 stable | B+tree write amp, not append-only | 1-2 weeks |
| **Custom WAL + segments** | Tailored to Fila's pattern | Must handle crash safety | 3-6 months |

**Swapping one KV store for another (redb, LMDB) doesn't solve the fundamental problem.** The tombstone/write-amplification issues are inherent to any key-value store used as a queue backend. The real win comes from moving to an append-only architecture.

### 5.4 Recommendation

**Tune RocksDB now. Defer FR66.**

1. Apply the configuration changes in section 3.3 (effort: days, impact: 3-10x)
2. Implement batching (sections 3.1-3.2) (effort: weeks, impact: 10-50x)
3. Profile and benchmark after both
4. If storage is still the bottleneck after tuning + batching, FR66 becomes justified

**FR66 becomes worthwhile when:**
- RocksDB tuning is exhausted and profiling shows storage is the remaining bottleneck
- Tombstone accumulation causes measurable latency spikes in production
- Segment-level GC advantage matters because disk space is a constraint
- Fila needs to compete on raw throughput with Kafka/NATS (>100K msg/s)

---

## 6. Prioritized Optimization Roadmap

### Tier 1: High Impact, Low Effort (Do First)

| # | Optimization | Expected Impact | Effort | Risk |
|---|-------------|----------------|--------|------|
| 1 | **RocksDB tuning** (bloom, cache, pipelined writes, CompactOnDeletion) | 3-10x throughput | 2-3 days | Very low |
| 2 | **gRPC HTTP/2 tuning** (window sizes, keepalive, tcp_nodelay) | 1.5-2x streaming | Hours | Very low |
| 3 | **DRR data structure** (eliminate O(n) key scan) | 3x at 10K keys | 1-2 days | Low |

**Combined Tier 1 impact: ~5-15x throughput improvement → 10K-40K msg/s**

### Tier 2: High Impact, Medium Effort (Do Next)

| # | Optimization | Expected Impact | Effort | Risk |
|---|-------------|----------------|--------|------|
| 4 | **Client-side batching** (SDK linger_ms + batch_size) | 10-50x for small msgs | 2-3 weeks | Low |
| 5 | **Server-side write coalescing** (batch concurrent enqueues) | 3-5x | 1-2 weeks | Medium |
| 6 | **Delivery batching** (multiple msgs per stream response) | 1.5-3x for consumers | 1 week | Low |

**Combined Tier 2 impact: ~15-75x on top of Tier 1 → 30K-100K msg/s**

### Tier 3: Medium Impact, Medium Effort (Diminishing Returns)

| # | Optimization | Expected Impact | Effort | Risk |
|---|-------------|----------------|--------|------|
| 7 | **Zero-copy protobuf passthrough** (Bytes, skip re-serialization) | 1.2-1.5x | 1-2 weeks | Medium |
| 8 | **Scheduler sharding** (per-queue or hash-sharded) | ~Nx for N shards (multi-queue) | 3-4 weeks | Medium |
| 9 | **Key encoding optimization** (pre-alloc, stack buffers) | Minimal until 50K+ msg/s | 1 day | Very low |

### Tier 4: Architectural Changes (Defer)

| # | Optimization | Expected Impact | Effort | Risk |
|---|-------------|----------------|--------|------|
| 10 | **Purpose-built storage engine (FR66)** | 10-100x on top of batching | 3-6 months | High |
| 11 | **io_uring integration** (requires FR66) | 2-3x on top of FR66 | Months + runtime migration | High |
| 12 | **Thread-per-core architecture** (requires FR66 + io_uring) | 2-5x on top of io_uring | Full rewrite | Very high |

### Realistic Target Envelope

| Phase | Target Throughput (1KB) | Strategies | Timeline |
|-------|------------------------:|-----------|----------|
| **Current** | 2.7K msg/s | — | Now |
| **After Tier 1** | 10K-15K msg/s | RocksDB tuning, gRPC tuning, DRR fix | 1 week |
| **After Tier 2** | 30K-100K msg/s | Batching (client + server + delivery) | 1-2 months |
| **After Tier 3** | 50K-150K msg/s | Zero-copy, scheduler sharding | 1-2 months |
| **After FR66** | 200K-500K msg/s | Purpose-built storage | 3-6 months |
| **After io_uring** | 500K-1M msg/s | io_uring + thread-per-core | 6-12 months |

**Note:** These are single-node numbers. Clustering adds overhead (Raft consensus) but also adds horizontal scaling. The competitive comparison context (Kafka at 1.4M) includes Kafka's batching — Fila with Tier 2 batching would be a fairer comparison.

---

## 7. Sources

### RocksDB Tuning
- [RocksDB Tuning Guide](https://github.com/facebook/rocksdb/wiki/RocksDB-Tuning-Guide)
- [Implement Queue Service Using RocksDB](https://github.com/facebook/rocksdb/wiki/Implement-Queue-Service-Using-RocksDB) — directly addresses Fila's access pattern
- [RocksDB Setup Options and Basic Tuning](https://github.com/facebook/rocksdb/wiki/Setup-Options-and-Basic-Tuning)
- [RocksDB Block Cache](https://github.com/facebook/rocksdb/wiki/Block-Cache)
- [RocksDB Bloom Filter](https://github.com/facebook/rocksdb/wiki/RocksDB-Bloom-Filter)
- [RocksDB Pipelined Write](https://github.com/facebook/rocksdb/wiki/Pipelined-Write) — 30% throughput improvement measured
- [FlushWAL; less fwrite, faster writes (MyRocks)](https://rocksdb.org/blog/2017/08/25/flushwal.html) — 40% improvement for `manual_wal_flush`
- [RocksDB CompactOnDeletionCollector](https://github.com/facebook/rocksdb/wiki/Implement-Queue-Service-Using-RocksDB#compaction) — queue-specific recommendation
- [5 RocksDB Tweaks That Tame Write Amplification](https://medium.com/@connect.hashblock/5-rocksdb-tweaks-that-tame-write-amplification-f31685910d6e)
- [Integrated BlobDB | RocksDB](https://rocksdb.org/blog/2021/05/26/integrated-blob-db.html) — 88-90% write amp reduction
- [Performance Tuning RocksDB for Kafka Streams](https://www.confluent.io/blog/how-to-tune-rocksdb-kafka-streams-state-stores-performance/)
- [EMQX: 1M msg/s with RocksDB](https://www.emqx.com/en/blog/embedded-mqtt-message-storage-using-rocksdb-for-emqx-broker) — evidence of high-throughput RocksDB queue

### Competitor Architecture
- [How Kafka Achieves High Throughput](https://dev.to/konstantinas_mamonas/how-kafka-achieves-high-throughput-a-breakdown-of-its-log-centric-architecture-3i7k)
- [Why Kafka Is So Fast (AutoMQ)](https://www.automq.com/blog/why-kafka-high-throughput)
- [The Zero Copy Optimization in Apache Kafka](https://blog.2minutestreaming.com/p/apache-kafka-zero-copy-operating-system-optimization)
- [Kafka Performance Tuning: linger.ms and batch.size](https://www.automq.com/blog/kafka-performance-tuning-linger-ms-batch-size)
- [NATS and Kafka Compared (Synadia)](https://www.synadia.com/blog/nats-and-kafka-compared)
- [JetStream Anti-Patterns (Synadia)](https://www.synadia.com/blog/jetstream-design-patterns-for-scale)
- [Engineering Redpanda for Multi-Core Hardware](https://www.redpanda.com/blog/engineering-redpanda-multi-core-hardware)

### io_uring and Async I/O
- [Apache Iggy thread-per-core with io_uring](https://iggy.apache.org/blogs/2026/02/27/thread-per-core-io_uring/) — >5 GB/s throughput after migration
- [Monoio: High-Performance Rust Runtime (ByteDance)](https://chesedo.me/blog/monoio-introduction/) — 2-3x tokio at 16 cores
- [tokio-uring status](https://github.com/tokio-rs/tokio-uring) — stalled development
- [Status of tokio_uring? (Rust forum)](https://users.rust-lang.org/t/status-of-tokio-uring/114481)

### gRPC Performance
- [gRPC Performance Best Practices](https://grpc.io/docs/guides/performance/)
- [The Mysterious Gotcha of gRPC Stream Performance (Ably)](https://ably.com/blog/grpc-stream-performance) — flow control window issues
- [Performance best practices with gRPC (Microsoft)](https://learn.microsoft.com/en-us/aspnet/core/grpc/performance)
- [gRPC Flow Control](https://grpc.io/docs/guides/flow-control/)

### Storage Engine Design
- [Kafka Storage Internals: Segments, Indexing, and Retention](https://www.conduktor.io/blog/understanding-kafka-s-internal-storage-and-log-retention)
- [Understanding How Apache Pulsar Works](https://jack-vanlightly.com/blog/2018/10/2/understanding-how-apache-pulsar-works) — BookKeeper architecture
- [RedPanda Architecture](https://docs.redpanda.com/current/get-started/architecture/)
- [TigerBeetle Design](https://www.amplifypartners.com/blog-posts/why-tigerbeetle-is-the-most-interesting-database-in-the-world) — purpose-built storage for financial transactions
- [Why LSM for a Message Queue? (HN discussion)](https://news.ycombinator.com/item?id=10407291) — arguments against

### Benchmark Methodology
- [Fila Benchmark Methodology Research (2026-03-23)](/Users/lucas/code/faisca/fila/_bmad-output/planning-artifacts/research/technical-benchmarking-methodology-research-2026-03-23.md) — companion report
- [bench/competitive/METHODOLOGY.md](../bench/competitive/METHODOLOGY.md)
