# Custom Transport Protocol Design for Kafka Throughput Parity

**Date:** 2026-03-25
**Author:** Lucas
**Research Type:** Technical Architecture

---

## Research Overview

This research investigates the feasibility and design of a custom transport protocol for Fila to close the 37x throughput gap with Kafka (9,500 vs ~400,000 msg/s). The starting point is profiling data showing 62% of server CPU spent in HTTP/2 transport (h2/hyper/tonic), 11% in application logic, and ~27% in storage/runtime.

**Key constraint:** Fila's differentiating features (DRR fair scheduling, per-message Lua hooks, per-message ack/nack, token-bucket throttling) must be preserved. The question is not "how to be Kafka" but "how fast can Fila be while remaining Fila."

**Methodology:** Parallel deep research across 4 domains: Kafka wire protocol internals, custom binary protocol design patterns, storage engines & threading models, and realistic throughput ceiling analysis. Sources include Kafka protocol specification, production system benchmarks (mqperf, LinkedIn, Confluent), Fila's own subsystem benchmarks, and architectural analysis of NATS, Redpanda, Pulsar, Iggy, and Chronicle Queue.

---

## 1. What Makes Kafka's Transport Fast

### 1.1 Wire Protocol: Raw TCP, Zero HTTP Overhead

Kafka uses a **length-prefixed binary protocol over raw TCP**. Every request/response is framed as `[4-byte int32 size][payload]`. The request header adds `api_key` (2B) + `api_version` (2B) + `correlation_id` (4B) + `client_id` (variable). Response header is just `correlation_id` (4B) + tagged fields (1B).

**Total per-RPC overhead: ~15-20 bytes** (request header + response header).

Compare to gRPC/HTTP/2 per unary RPC:

| Component | gRPC/HTTP/2 | Kafka |
|-----------|------------|-------|
| Request headers | 29-109 bytes (HPACK) | 14-20 bytes |
| Request data framing | 14 bytes (9B HTTP/2 + 5B gRPC) | 4 bytes (size prefix) |
| Response headers | 15-25 bytes (HPACK) | 5 bytes |
| Response data framing | 14 bytes | 4 bytes |
| Response trailers | 12-20 bytes (grpc-status) | 0 bytes |
| **Total overhead** | **84-182 bytes** | **27-33 bytes** |

But the byte overhead is the *least* of it. The CPU overhead is far more significant:

- **HPACK encode/decode**: Maintaining dynamic tables, Huffman encoding/decoding for every header on every RPC. Shows up as a major component of Fila's 62% HTTP/2 CPU.
- **HTTP/2 stream state machine**: Each unary RPC creates/opens/half-closes/closes a stream. State tracking per stream.
- **Flow control**: Two layers (connection-level + stream-level WINDOW_UPDATE frames). Kafka has zero wire-level flow control.
- **SETTINGS/PING/GOAWAY**: HTTP/2 connection management frames with no equivalent in Kafka.

Published benchmarks confirm the magnitude:
- **Ably** found ~50% of total CPU was kernel/HTTP/2 overhead in their gRPC-based system, with application code "barely registering."
- **RSocket vs gRPC**: RSocket (binary over TCP) achieved 300K TPS vs gRPC's 180K TPS -- **1.67x throughput at 45% less CPU** -- purely from eliminating HTTP/2.
- **Fila's own profiling**: 62% of server CPU in h2/hyper/tonic. Eliminating this alone yields a theoretical 2.6x throughput gain (1/0.38).

### 1.2 RecordBatch: Batch-First Data Format

Kafka's RecordBatch is the core unit of data on wire, on disk, and in memory -- the same bytes everywhere.

**RecordBatch header (61 bytes fixed):** baseOffset (8B) + batchLength (4B) + partitionLeaderEpoch (4B) + magic (1B) + CRC-32C (4B) + attributes (2B) + lastOffsetDelta (4B) + timestamps (16B) + producer metadata (14B) + recordCount (4B).

**Per-record format (varint-encoded, ~6-8 bytes overhead per record in practice):** Delta-encoded timestamps and offsets make varints tiny (usually 1-2 bytes each).

**The critical insight: the Kafka broker does NOT deserialize individual messages.** The data path is:

1. Producer serializes messages into a RecordBatch (with compression)
2. Broker receives bytes and writes them directly to the partition log as-is
3. Consumer fetches, broker reads bytes from log and sends directly to socket (sendfile)

CPU cost scales with **batches, not messages**. A batch of 1 and a batch of 10,000 cost the broker the same CPU. From the LinkedIn benchmark: batch size 1 = 50,000 msg/s; batch size 50 = 400,000 msg/s -- an **8x improvement** purely from producer-side batching.

### 1.3 Zero-Copy via sendfile(2)

Traditional data path: 4 data copies + 4 context switches (disk -> kernel buffer -> user buffer -> socket buffer -> NIC).

Kafka's sendfile path: 2 data copies + 2 context switches (disk -> page cache -> NIC DMA). With DMA gather operations (Linux 2.4+), only descriptor pointers are copied to the socket buffer -- no CPU data copy at all.

**Measured impact:** 30-50% CPU reduction under heavy loads, ~2x throughput for large transfers, 20-30% latency reduction.

**Critical limitation: TLS breaks zero-copy completely.** The broker must decrypt/encrypt in userspace, restoring the full 4-copy path. Kafka's documentation confirms this explicitly.

**Multi-consumer amplification:** Data enters page cache once and is reused for every consumer fetch. On a cluster where consumers are caught up, "you will see no read activity on the disks."

### 1.4 What Kafka DOESN'T Do

| Feature Fila Has | Kafka Doesn't | Estimated CPU Cost |
|-----------------|---------------|-------------------|
| Per-message ack/nack (3 storage mutations per ack) | Batch-level ack (single offset advance) | 20-40% of broker CPU |
| DRR fair scheduling (per-message selection) | No scheduling (sequential offset reads) | 5-15% |
| Lua hooks (VM invocation per message) | No server-side processing | 0-10% (depends on usage) |
| Individual message state tracking (pending/leased/acked) | Single offset per consumer group | 20-40% |
| Per-message routing (fairness key + DLQ) | Partition-level only | 1-5% |
| **Cumulative** | | **46-110% additional CPU** |

Kafka's broker is a dumb pipe: receive bytes, append to log, send bytes. This is why it's fast.

### 1.5 How Other Fast Systems Compare

From the mqperf benchmark (replicated, persistent):

| System | Protocol | Throughput | Architecture |
|--------|----------|-----------|--------------|
| Kafka | Custom binary/TCP | 828,836 msg/s | JVM, page cache, sendfile |
| RocketMQ | Custom binary/TCP | 485,322 msg/s | JVM, mmap files |
| Pulsar | Custom binary+protobuf/TCP | 358,323 msg/s | JVM, BookKeeper |
| Artemis | Custom binary/TCP | 52,820 msg/s | JVM, journal |
| NATS JetStream | Text/TCP | 27,400 msg/s | Go, file storage |
| RabbitMQ | AMQP/TCP | 19,120 msg/s | Erlang, quorum queues |
| Apache Iggy | Custom binary/TCP | ~5M msg/s | Rust, io_uring, thread-per-core |

**Common thread: every top performer uses a custom protocol over raw TCP. None use HTTP/2 or gRPC.**

Apache Iggy is the most relevant comparison: also Rust, uses a custom binary protocol, achieves 5M msg/s with 1KB messages. But Iggy is a log-based system without per-message features.

---

## 2. Custom Fila Wire Protocol Design (FIBP)

### 2.1 Design Principles

Based on analysis of Kafka, NATS, Pulsar, Iggy, and memcached protocols:

1. **Length-prefixed framing** (Kafka model): 4-byte big-endian size, `tokio_util::codec::LengthDelimitedCodec` out of the box
2. **Protobuf for metadata, raw bytes for payloads** (Pulsar model): schema evolution for control plane, zero-overhead for data
3. **Correlation IDs with in-order responses** (Kafka model): no multiplexing complexity
4. **Credit-based flow control for consume streams** (Pulsar model): simpler than HTTP/2
5. **Batch-first API**: all operations accept batches; single = batch of 1

### 2.2 Frame Format

```
+-------------------+---------------------+
| frame_length: u32 | frame_body: [u8]    |
+-------------------+---------------------+
|    4 bytes BE     | frame_length bytes  |
```

`frame_length` excludes itself. Maximum frame size: 16MB (configurable).

### 2.3 Frame Body Structure

```
+----------+--------+------------------+-----------------+
| flags:u8 | op:u8  | corr_id:u32     | payload:[u8]    |
+----------+--------+------------------+-----------------+
| 1 byte   | 1 byte | 4 bytes BE      | remaining bytes |
```

**Flags** (bitfield):
- Bit 0: direction (0 = request, 1 = response)
- Bit 1: error flag (response only)
- Bit 2: stream frame (consume delivery)
- Bit 3: compressed (lz4)
- Bits 4-7: reserved

**Operation codes:**

| Code | Operation | Direction |
|------|-----------|-----------|
| 0x01 | Enqueue | Request |
| 0x02 | Consume | Request (initiates stream) |
| 0x03 | Ack | Request |
| 0x04 | Nack | Request |
| 0x10 | CreateQueue | Request |
| 0x11 | DeleteQueue | Request |
| 0x12 | GetQueueStats | Request |
| 0x13 | ListQueues | Request |
| 0x14 | PauseQueue | Request |
| 0x15 | ResumeQueue | Request |
| 0x16 | Redrive | Request |
| 0x20 | Flow | Consumer -> broker |
| 0x21 | Heartbeat | Both |
| 0x30 | Auth | Client -> broker |
| 0xFE | Error | Response only |
| 0xFF | GoAway | Server -> client |

### 2.4 Enqueue Request Payload

```
+-----------------+--------------------+
| queue_len: u16  | queue: [u8]        |  <- queue name (UTF-8, once per batch)
+-----------------+--------------------+
| msg_count: u16  |                    |
+-----------------+--------------------+
| Per message:                         |
|   header_count: u8                   |
|   headers: repeated (u16+key, u16+value)
|   payload_len: u32                   |
|   payload: [u8]  (raw bytes)         |
+--------------------------------------+
```

Queue name appears once per batch. Message payloads are raw bytes -- never protobuf-encoded. Headers are inline length-prefixed key-value pairs.

### 2.5 Enqueue Response Payload

```
| msg_count: u16                       |
| Per message:                         |
|   ok: u8                             |
|   if ok=1: msg_id [u8; 16] (UUID)   |
|   if ok=0: err_code:u16, err_len:u16, err_msg:[u8] |
```

### 2.6 Consume: Credit-Based Flow Control

Consumer sends initial `Consume` request with `initial_credits: u32`. Broker pushes messages until credits are exhausted. Consumer grants more credits via `Flow` frames (op 0x20) containing `credits: u32`.

This is the Pulsar model. Prevents slow consumer overwhelm without TCP-level backpressure. Simpler than HTTP/2 flow control.

Server push frames have bit 2 (stream) set in flags. Message metadata uses protobuf (complex nested data: fairness_key, weight, timestamps). Payloads stay raw.

### 2.7 Connection Lifecycle

```
Client -> FIBP\x01\x00          (magic + version 1.0)
Server -> FIBP\x01\x00\x00      (magic + version + status ok)
[if auth enabled]
Client -> Auth frame with API key
Server -> Auth response
[normal request/response flow]
[server may send GoAway before closing]
```

### 2.8 Per-Message Overhead Comparison

For a 1KB Enqueue of 1 message with 2 headers:

| Component | Current (gRPC) | Proposed (FIBP) |
|-----------|---------------|-----------------|
| Frame overhead | ~300-400 bytes | 10 bytes (4+1+1+4) |
| Serialization | ~2-5 us (protobuf entire request) | ~0.5 us (fixed fields + memcpy) |
| Transport processing | ~50-200 us (HPACK + flow control + stream FSM) | ~1-5 us (read length + read body) |
| Buffer copies | 2-3 (HPACK decode, DATA reassembly, protobuf decode) | 1 (kernel -> BytesMut) |
| Syscalls per RPC | 2+ (may fragment across HTTP/2 frames) | 2 (one read, one write) |

**Estimated improvement: 10-40x reduction in per-message protocol overhead.**

### 2.9 Implementation in Rust/tokio

```rust
struct FibpCodec;

impl Decoder for FibpCodec {
    type Item = Frame;
    type Error = io::Error;

    fn decode(&mut self, src: &mut BytesMut) -> Result<Option<Frame>, io::Error> {
        if src.len() < 4 { return Ok(None); }
        let len = u32::from_be_bytes([src[0], src[1], src[2], src[3]]) as usize;
        if src.len() < 4 + len { return Ok(None); }
        src.advance(4);
        let body = src.split_to(len).freeze(); // zero-copy: Bytes ref into same memory
        Ok(Some(Frame::parse(body)?))
    }
}
```

The hot path becomes: `read syscall -> split_to (zero-copy) -> parse 6-byte header -> dispatch`. Message payload bytes flow as `Bytes` slices (reference-counted views into the read buffer) all the way to storage with zero copies.

---

## 3. Zero-Copy and sendfile

### 3.1 Can Fila Use sendfile?

**No, not for the consume path.** sendfile requires that the bytes on disk are the bytes you want to send, in the order you want to send them. DRR means the server picks which message to deliver based on fairness-key rotation -- it must read message metadata, make a scheduling decision, then send the chosen message. This breaks the sequential-stream assumption that sendfile depends on.

Even Kafka's zero-copy breaks under TLS, and Fila already supports mTLS.

### 3.2 What Fila CAN Do: Match Wire and Storage Formats

Even without sendfile, eliminating serialization/deserialization on the delivery path is a significant win:

1. Store messages in the wire format (FIBP binary encoding) on disk
2. On delivery, read raw bytes from RocksDB and frame them directly into the response without deserializing then re-serializing
3. Save two protobuf operations per consumed message (decode from storage + encode for wire)

This is not zero-copy in the sendfile sense, but it removes CPU overhead from the hot path.

### 3.3 Where DRR Conflicts with Sequential I/O

Kafka's entire storage design assumes sequential reads from an offset. DRR requires random-access reads by fairness key. This is a fundamental architectural difference:

- **Kafka**: Seek to offset, read forward, sendfile to socket. O(1) per consumer.
- **Fila**: Select next fairness key (DRR), look up pending messages for that key, read specific message, send. O(k) per delivery cycle where k = active fairness keys.

**No hybrid approach can reconcile sequential I/O with DRR's access pattern.** The DRR scheduling overhead (~120 ns per message, measured) is acceptable, but it prevents the zero-copy optimization that gives Kafka its final 2-3x advantage.

---

## 4. Storage for Throughput

### 4.1 Why Not Replace RocksDB

RocksDB accounts for ~10% of per-message time. Even eliminating it entirely (InMemory engine) yields only 6-13% throughput gain (measured). The engineering cost of replacing RocksDB is enormous; the return is marginal.

Current tuning is already extensive: unordered writes, per-level compression (None L0-L1, LZ4 L2+), bloom filters (10 bits/key), CompactOnDeletionCollector, memtable prefix bloom, Titan blob separation for cf_messages. The remaining RocksDB tuning options (universal compaction, direct I/O, FIFO compaction) are either incompatible with Fila's access pattern or offer diminishing returns.

**Verdict: RocksDB stays. Optimization effort belongs elsewhere.**

### 4.2 Append-Only Log + In-Memory Index (Future Option)

If storage ever becomes the bottleneck (at 100K+ msg/s after transport optimization), a custom design could be:

1. Append messages to a sequential log file (one per shard)
2. In-memory index: `HashMap<FairnessKey, VecDeque<LogPosition>>` for DRR
3. Per-message state tracked in memory with periodic checkpointing
4. Recovery: replay log to rebuild indexes

This eliminates RocksDB overhead entirely. Risk: memory limits in-flight messages, recovery time scales with log size. For a broker where messages are transient (acked and deleted), this is often acceptable.

**Not recommended until the transport bottleneck is resolved.** Premature storage optimization when storage is 10% of cost.

### 4.3 Memory-Mapped Storage: Not for Fila

Research on mmap (CMU CIDR 2022 paper, Chronicle Queue experience) shows:

- **Unpredictable page fault latency** -- any access can block the thread, invisible to the runtime
- **No control over dirty page writeback** -- OS can flush at any time, breaking ordering guarantees
- **TLB shootdowns** -- expensive cross-core IPIs during page eviction
- **OOM killer risk** under memory pressure with writable mmaps

For a broker that requires predictable latency for DRR scheduling and guaranteed durability for per-message ack, mmap is unsuitable.

---

## 5. Threading Model

### 5.1 Current Model: Multi-Shard on Tokio

Fila uses a multi-shard scheduler (default multi-shard as of Epic 34), each shard is a tokio task processing commands from a crossbeam channel. This is essentially thread-per-shard running on tokio's work-stealing executor.

Tokio scheduling overhead: ~169 ns/task spawn, ~563 ns ping-pong between tasks. At 9,500 msg/s, tokio overhead is ~0.1-0.5% of total CPU. **Tokio is not the bottleneck.**

### 5.2 Kafka's Model: Multi-Stage Reactor (NIO)

Kafka uses:
1. **Acceptor thread** (1 per listener): accepts connections, round-robins to processors
2. **Processor threads** (default 3): NIO Selector multiplexing, reads bytes, forms requests, writes responses
3. **I/O handler threads** (default 8): picks from request queue, does CRC check + sequential write

Not one-thread-per-partition. The hot path is blocking I/O in handler threads (sequential append). No async runtime overhead.

### 5.3 Thread-per-Core / io_uring (Iggy, Redpanda)

**Apache Iggy migrated from Tokio to thread-per-core with io_uring (2026):**

| Metric | Tokio | Thread-per-core | Change |
|--------|-------|-----------------|--------|
| Throughput (32 producers) | 1,000 MB/s | 1,001 MB/s | ~0% |
| P95 latency | 3.77 ms | 1.62 ms | **-57%** |
| P99 latency | 4.52 ms | 1.82 ms | **-60%** |
| P99.99 latency | 27.52 ms | 11.83 ms | **-57%** |
| Throughput with fsync | 931 MB/s | 1,102 MB/s | **+18%** |

**Key finding: throughput improvement is modest (~0-18%), but tail latency improvement is dramatic (57-60%).** The gap widens beyond 8 cores.

**Redpanda (Seastar/C++)** thread-per-core results are more nuanced than marketing claims. Independent testing (Jack Vanlightly) found:
- Kafka handled 500 MB/s with 100 producers; Redpanda topped at 330 MB/s
- Redpanda P99 latency reached 3.5 seconds after 12 hours; P99.99 hit 26 seconds
- Kafka remained stable throughout 24-hour tests

Thread-per-core alone is not a silver bullet. The advantages are: no GC (C++/Rust), no work-stealing cache thrashing, NUMA-local memory. The cost is: loss of tokio ecosystem, 6+ month rewrite, no work-stealing means manual load balancing.

### 5.4 Recommendation for Fila

**Short-term (no migration):** Pin scheduler shards to dedicated cores with `core_affinity`. Use tokio's `LocalSet` for per-shard tasks. Gets ~80% of cache locality benefit without abandoning tokio.

**Medium-term (if 100K+ msg/s):** Evaluate `tokio::runtime::Builder::new_current_thread()` per shard with manual work distribution.

**Long-term (if tail latency is critical):** Thread-per-core migration. Iggy's experience suggests 6+ month rewrite effort. Only justified if Fila targets 500K+ msg/s AND p99 < 5ms simultaneously.

io_uring specifically helps when:
- Core count > 8 (monoio shows 2-3x vs tokio at 16 cores)
- File I/O is moved to true async (replacing thread pool)
- Linux-only deployment (not macOS)

---

## 6. Dual-Protocol Strategy

### 6.1 How Production Systems Do It

| System | Protocols | Shared Layer |
|--------|-----------|-------------|
| ScyllaDB | CQL + DynamoDB/Alternator | Both call same `mutation_apply()`, `read_data()` |
| NATS | Custom + WebSocket + MQTT | All call `client.processMsg()` |
| Apache Iggy | TCP + QUIC + WebSocket + HTTP | All call same command handlers |
| Redis | RESP + modules (gRPC) | All call same command dispatch |

The proven pattern: **shared command/handler layer with pluggable transports.**

```
[gRPC transport] ---\
                     +--> Command enum --> Handler --> Broker core
[FIBP transport] ---/
```

### 6.2 Architecture for Fila

Fila already has the right structure. The tonic service handlers in `service.rs` parse gRPC requests and call broker methods. A custom transport would do the same:

1. Both transports share `Broker`, `Arc<dyn StorageEngine>`, `ClusterHandle`
2. Each transport: accept connection -> parse wire format -> build `SchedulerCommand` -> send to scheduler -> await response -> serialize wire format
3. gRPC for admin operations, SDK compatibility, ecosystem tooling (grpcurl, Postman)
4. FIBP for hot-path performance (enqueue, consume, ack, nack)

### 6.3 SDK Impact

A custom protocol requires new client code in all 6 SDKs (Rust, Go, Python, JS, Ruby, Java). Options:

1. **New SDK transport layer**: Each SDK adds FIBP alongside gRPC. Auto-negotiate on connect (try FIBP, fall back to gRPC). ~2-3 weeks per SDK.
2. **FIBP-only fast client**: Separate "high-performance" client class. Simpler to implement, explicit opt-in.
3. **Keep gRPC as the only SDK protocol**: Add FIBP only for the Rust SDK initially, use as internal transport for cluster communication. Other SDKs benefit indirectly from reduced server-side CPU.

Option 3 is the pragmatic start. The Rust SDK is in the same workspace -- easy to iterate. External SDKs can be migrated later if demand warrants.

---

## 7. Realistic Throughput Ceiling

### 7.1 Per-Message Cost Breakdown (All Features, Custom Transport)

From Fila's subsystem benchmarks (commit eed6eef):

| Component | Time | Source |
|-----------|-----:|--------|
| Protocol framing (FIBP) | ~200 ns | Estimated: write 4B length + payload |
| Protobuf decode | ~222 ns | Measured: subsystem bench |
| Lua on_enqueue hook | ~2,900 ns | Measured: complex routing script |
| DRR scheduling | ~122 ns | Measured: 8.2M sel/s at 10K keys |
| Throttle check | ~100 ns | Estimated: 1-3 keys |
| RocksDB write | ~6,200 ns | Measured: 160K ops/s at 1KB |
| Protobuf encode (response) | ~83 ns | Measured: 12,335 MB/s at 1KB |
| Protocol framing (response) | ~200 ns | Estimated |
| String interning + indexing | ~100 ns | Estimated |
| Metrics recording | ~100 ns | Estimated |
| **Total** | **~10,227 ns** | |

**Theoretical ceiling (all features, 1 thread): ~98K msg/s.**
**With 4 shards: ~391K msg/s.**

### 7.2 With Batch Amortization (100 Messages Per Batch)

Per-batch costs (amortized 1/100 each):

| Component | Per-batch | Per-message amortized |
|-----------|----------:|---------------------:|
| Protocol frame read/write | ~400 ns | ~4 ns |
| Protobuf decode (batch) | ~22,000 ns | ~220 ns |
| Channel send + await | ~700 ns | ~7 ns |
| RocksDB WriteBatch | ~30,000 ns | ~300 ns |
| Protobuf encode (response) | ~8,000 ns | ~80 ns |
| **Transport + storage subtotal** | | **~611 ns** |

Per-message costs (cannot be batched):

| Component | Per-message |
|-----------|------------:|
| Lua on_enqueue hook | ~2,900 ns |
| DRR add_key + pending push | ~170 ns |
| Throttle check | ~100 ns |
| Key generation | ~100 ns |
| Protobuf encode (storage) | ~83 ns |
| Message construction | ~250 ns |
| **Per-message subtotal** | **~3,603 ns** |

**Total with Lua: 611 + 3,603 = 4,214 ns/msg = ~237K msg/s per thread, ~948K msg/s with 4 shards.**
**Total without Lua: 611 + 703 = 1,314 ns/msg = ~761K msg/s per thread, ~3M msg/s with 4 shards.**

### 7.3 "Fast Queue" Mode (No Lua, FIFO-Only, Batch Ack)

| Component | Time |
|-----------|-----:|
| Protocol framing | ~200 ns |
| Protobuf decode | ~222 ns |
| Key generation | ~100 ns |
| Protobuf encode (storage) | ~83 ns |
| RocksDB write (amortized) | ~300 ns |
| Pending index push | ~50 ns |
| Response framing | ~200 ns |
| **Total** | **~1,155 ns** |

**Fast-queue ceiling: ~866K msg/s per thread, ~3.5M msg/s with 4 shards.** Exceeds Kafka's published single-broker numbers.

### 7.4 Published Competitive Numbers

| System | Throughput (1KB) | Per-message features |
|--------|----------------:|---------------------|
| Redpanda (3-node cluster) | ~800K msg/s | None (Kafka-compatible) |
| Kafka (single broker, batched) | ~143K msg/s | None |
| NATS JetStream | ~138K msg/s | Per-msg ack |
| Pulsar | ~100-250K msg/s | Per-msg ack, DLQ, delayed delivery |
| RabbitMQ (quorum queues) | ~38K msg/s | Per-msg ack, DLQ, priority |
| **Fila (current, 4 producers)** | **~25K msg/s** | **DRR, Lua, per-msg ack, DLQ, throttle** |

### 7.5 The Honest Answer

**True Kafka parity (400K+ msg/s) is achievable, but only under specific conditions:**

- Custom binary transport (eliminating 62% HTTP/2 overhead)
- Batch amortization (100+ messages per network round-trip)
- Multi-shard scheduling (4+ shards)
- Without Lua hooks (or with batch Lua evaluation)

**With all features enabled (DRR + Lua + per-message ack + throttling), the realistic ceiling is 100-250K msg/s.** This is Pulsar-class, not Kafka-class.

**But this is the right positioning.** The market segment that needs 400K+ msg/s (log aggregation, event streaming, click tracking) uses Kafka and doesn't need Fila's features. The market that needs fairness + hooks + per-message ack (multi-tenant SaaS, workflow engines, job queues) runs at 10K-100K msg/s and would find Fila at 100K+ msg/s extremely compelling.

RabbitMQ is the closest competitor in feature set at ~38K msg/s. **Fila at 100K msg/s = 2.6x RabbitMQ with a richer feature set.** That's the pitch: "RabbitMQ features with Pulsar throughput."

**No system achieves 400K+ msg/s with per-message server-side scripting.** Every system at that throughput (Kafka, Redpanda, NATS, Iggy) processes messages as opaque blobs. The fundamental tradeoff is real: at 400K msg/s you have 2.5 us per message -- barely enough for a HashMap lookup, let alone Lua VM invocation.

---

## 8. Incremental Path

### Phase 1: gRPC Streaming + Batch Optimization (2-4 weeks)

**What:** Optimize the existing gRPC streaming path. Ensure SDK `AccumulatorMode` defaults match Kafka's proven settings (~5ms linger, ~100-1000 messages per batch). Multi-shard default already done in Epic 34.

**Expected gain:** 3-5x. Single-producer: 10K -> 30-50K msg/s. Multi-producer: 25K -> 80-150K msg/s.

**Risk:** Low. Infrastructure exists, this is tuning.

**Why this first:** The streaming RPC path amortizes per-message HTTP/2 overhead. A batch of 100 messages in one DATA frame pays the HPACK/framing cost once instead of 100 times. This is the cheapest win available.

### Phase 2: Custom Binary Protocol (FIBP) (6-10 weeks)

**What:** Implement the FIBP protocol on a second TCP listener. Shared command layer with gRPC. Rust SDK adds FIBP transport. Other SDKs continue using gRPC.

**Expected gain:** 2-3x on top of Phase 1. Single-producer: 50K -> 100-150K msg/s. Multi-producer: 150K -> 300-500K msg/s.

**Risk:** Medium. New protocol requires testing, security review (FIBP needs its own TLS + auth path), cluster forwarding adaptation. But the dual-protocol pattern is proven (Iggy, NATS, ScyllaDB).

**Why this second:** Eliminates the 62% HTTP/2 overhead entirely for the hot path. gRPC stays for admin, tooling, and backward compatibility.

### Phase 3: Write Coalescing + Storage Optimization (3-5 weeks)

**What:** Batch RocksDB WriteBatch commits (N messages per commit instead of 1). Adopt Pulsar-style ack cursors for "fast-ack" mode (advance cursor instead of 3 delete mutations per message). Optional append-only log for throughput-critical queues.

**Expected gain:** 1.5-2x on storage-bound paths. Ack throughput: significant gain from 3 mutations -> 1 cursor advance.

**Risk:** Medium. Ack semantics change affects durability guarantees. Write coalescing adds latency (messages wait for batch to fill).

### Phase 4: Threading Optimization (4-8 weeks, optional)

**What:** Pin scheduler shards to cores. Evaluate thread-per-core for the FIBP listener (separate from tokio). io_uring for Linux deployments.

**Expected gain:** ~0% throughput, 50-60% P99 latency reduction (based on Iggy's results).

**Risk:** High. io_uring is Linux-only. Thread-per-core requires abandoning tokio for that code path. Only justified if tail latency matters more than development velocity.

### Phase 5: Lua Optimization (2-4 weeks, optional)

**What:** Batch Lua evaluation (pass N messages per VM invocation). Cache precompiled function references. Evaluate LuaJIT backend (mlua supports it).

**Expected gain:** 2-3x on Lua-heavy workloads. Reduces per-message Lua cost from ~2.9 us to ~1-1.5 us (amortize VM setup).

**Risk:** Low. Non-breaking change to the Lua engine internals.

### Summary: Cumulative Impact

| Phase | Target | Timeline | Risk |
|-------|--------|----------|------|
| Phase 1: gRPC streaming + batching | 30-50K msg/s (single) | 2-4 weeks | Low |
| Phase 2: FIBP custom protocol | 100-150K msg/s (single) | 6-10 weeks | Medium |
| Phase 3: Write coalescing | 150-200K msg/s (single) | 3-5 weeks | Medium |
| Phase 4: Thread pinning + io_uring | Same throughput, -60% P99 | 4-8 weeks | High |
| Phase 5: Batch Lua | 200-250K msg/s (with Lua) | 2-4 weeks | Low |

**Minimum viable change for biggest jump:** Phase 1 alone (2-4 weeks, 3-5x). This requires zero architectural changes -- just optimizing the existing batch/streaming path.

**Minimum change for "Pulsar-class" throughput:** Phase 1 + Phase 2 (8-14 weeks, 10-15x total). This puts Fila at 100K+ msg/s, competitive with Pulsar and 2.6x RabbitMQ.

---

## Appendix A: Fila vs Kafka Architecture Summary

| Aspect | Kafka | Fila (Current) | Fila (Post-Optimization) |
|--------|-------|----------------|--------------------------|
| Wire protocol | Custom binary/TCP | gRPC/HTTP/2 | FIBP + gRPC (dual) |
| Per-RPC overhead | ~30 bytes | ~84-182 bytes | ~10 bytes (FIBP) |
| Serialization | Custom binary (RecordBatch) | Protobuf (prost) | Protobuf metadata + raw payload |
| Batching | First-class (linger.ms + batch.size) | SDK accumulator | SDK accumulator + FIBP batch frames |
| Broker per-message work | Near zero (opaque blob append) | DRR + Lua + ack state + routing | Same (features are the product) |
| Storage | Append-only log, sendfile | RocksDB LSM tree | RocksDB + optional write coalescing |
| Consumer model | Pull (fetch offset range) | Push (server-side DRR selection) | Push (credit-based flow control) |
| Ack model | Batch offset commit | Per-message 3-mutation delete | Per-message (+ optional cursor mode) |
| Threading | Multi-stage reactor (NIO) | Multi-shard on tokio | Multi-shard on tokio (core-pinned) |
| Zero-copy | sendfile (non-TLS) | None | Wire=storage format match (not sendfile) |

## Appendix B: Key Sources

- [Apache Kafka Protocol Specification](https://kafka.apache.org/42/design/protocol/)
- [Kafka Message Format (RecordBatch)](https://kafka.apache.org/42/implementation/message-format/)
- [Kafka Efficient Design (sendfile, batching)](https://docs.confluent.io/kafka/design/efficient-design.html)
- [LinkedIn: Benchmarking Kafka -- 2M Writes/sec](https://engineering.linkedin.com/kafka/benchmarking-apache-kafka-2-million-writes-second-three-cheap-machines)
- [Ably: The Mysterious Gotcha of gRPC Stream Performance](https://ably.com/blog/grpc-stream-performance)
- [RSocket vs gRPC Benchmark](https://dzone.com/articles/rsocket-vs-grpc-benchmark)
- [tonic Buffer Copy Overhead (issue #1558)](https://github.com/hyperium/tonic/issues/1558)
- [NATS Client Protocol](https://docs.nats.io/reference/reference-protocols/nats-protocol)
- [Pulsar Binary Protocol](https://pulsar.apache.org/docs/next/developing-binary-protocol/)
- [Apache Iggy Thread-per-Core Migration](https://iggy.apache.org/blogs/2026/02/27/thread-per-core-io_uring/)
- [Jack Vanlightly: Kafka vs Redpanda -- Do the Claims Add Up?](https://jack-vanlightly.com/blog/2023/5/15/kafka-vs-redpanda-performance-do-the-claims-add-up)
- [Evaluating Persistent Replicated Message Queues (mqperf)](https://softwaremill.com/mqperf/)
- [CMU: Are You Sure You Want to Use MMAP?](https://db.cs.cmu.edu/mmap-cidr2022/)
- [gRPC over HTTP/2 Protocol](https://github.com/grpc/grpc/blob/master/doc/PROTOCOL-HTTP2.md)
- [Seastar Shared-Nothing Architecture](https://seastar.io/shared-nothing/)
- [DataDog: Introducing Glommio](https://www.datadoghq.com/blog/engineering/introducing-glommio/)
- [Monoio Benchmarks (ByteDance)](https://github.com/bytedance/monoio/blob/master/docs/en/benchmark.md)
- [Rust Serialization Benchmark](https://github.com/djkoloski/rust_serialization_benchmark)
- [Tokio Scheduler Rewrite (10x Faster)](https://tokio.rs/blog/2019-10-scheduler)
- [RocksDB Integrated BlobDB](https://rocksdb.org/blog/2021/05/26/integrated-blob-db.html)
- [RocksDB Tuning Guide](https://github.com/facebook/rocksdb/wiki/RocksDB-Tuning-Guide)
- [Kafka vs Redpanda Performance (Vanlightly)](https://jack-vanlightly.com/blog/2023/5/15/kafka-vs-redpanda-performance-do-the-claims-add-up)
- [RabbitMQ Queuing Theory](https://www.rabbitmq.com/blog/2012/05/11/some-queuing-theory-throughput-latency-and-bandwidth)
- [AMQP 0.9.1 Specification](https://www.rabbitmq.com/resources/specs/amqp0-9-1.pdf)
- [Chronicle Queue Performance](https://chronicle.software/queue/)
- [io_uring for DBMSs (arXiv)](https://arxiv.org/html/2512.04859v2)
