---
stepsCompleted:
  - step-01-validate-prerequisites
  - step-02-design-epics
  - step-03-create-stories
  - step-04-final-validation
status: complete
inputDocuments:
  - '_bmad-output/planning-artifacts/prd.md'
  - '_bmad-output/planning-artifacts/architecture.md'
  - '_bmad-output/planning-artifacts/research/technical-performance-optimization-strategies-research-2026-03-23.md'
  - '_bmad-output/planning-artifacts/research/technical-benchmarking-methodology-research-2026-03-23.md'
  - '_bmad-output/planning-artifacts/epics.md (existing epics 12-21)'
  - '_bmad-output/implementation-artifacts/sprint-status.yaml'
---

# Fila - Performance Optimization Epics (22-25)

## Overview

This document breaks down the performance optimization roadmap into epics and stories, based on the performance optimization research (2026-03-23). Fila currently sits at ~2.7K msg/s (1KB) single-node throughput — 60x behind Kafka and NATS. The research identifies 12 optimization strategies across 4 tiers. These epics follow the research's prioritization: highest-impact, lowest-effort changes first, deferring architectural rewrites until tuning headroom is exhausted.

**Baseline (commit d2cb526):** 2,724 msg/s enqueue (1KB), 0.40ms p50 latency, 2,393 msg/s lifecycle, 506 msg/s at 10K fairness keys.

**Target envelope:**
| Phase | Target (1KB) | Epic |
|-------|-------------:|------|
| Current | 2.7K msg/s | — |
| After Tier 1 | 10K-15K msg/s | Epic 22 |
| After Tier 2 | 30K-100K msg/s | Epic 23 |
| After Tier 3 | 50K-150K msg/s | Epic 24 |
| After FR66 | 200K-500K msg/s | Epic 25 (deferred) |

## Requirements Inventory

### Functional Requirements (Performance-Specific)

FR-P1: Server applies queue-optimized RocksDB configuration (bloom filters, block cache, pipelined writes, CompactOnDeletionCollector, per-CF tuning)
FR-P2: Server configures gRPC/HTTP/2 flow control windows and keepalive for high-throughput streaming
FR-P3: DRR scheduler uses O(1) or O(log n) data structure for key selection instead of O(n) linear scan
FR-P4: SDK supports client-side batching with configurable `linger_ms` and `batch_size` parameters
FR-P5: Server supports `BatchEnqueue` RPC that accepts multiple messages and commits them in a single WriteBatch
FR-P6: Scheduler coalesces concurrent enqueue requests into batched writes (server-side write coalescing)
FR-P7: Consumer streaming delivers multiple messages per gRPC response frame (delivery batching)
FR-P8: Message payload uses `bytes::Bytes` for zero-copy passthrough, skipping re-serialization for opaque payloads
FR-P9: Scheduler supports sharded execution — multiple scheduler threads, each responsible for a subset of queues
FR-P10: Storage key encoding uses pre-allocated buffers and stack arrays instead of per-key heap allocation
FR-P11: The broker can persist messages using a purpose-built append-only storage engine (FR66, deferred)

### Non-Functional Requirements (Performance Targets)

NFR1: 100,000+ msg/s single-node throughput on commodity hardware (existing, currently unmet at 2.7K)
NFR2: < 5% throughput cost for fair scheduling vs raw FIFO (existing, currently met at 3.3%)
NFR6: Enqueue-to-consume latency < 1ms p50 when consumer is waiting (existing, currently met at 0.40ms)
NFR-P1: Enqueue throughput >= 10K msg/s (1KB) after Tier 1 optimizations
NFR-P2: Enqueue throughput >= 30K msg/s (1KB) after Tier 2 optimizations (with batching)
NFR-P3: 10K fairness key throughput >= 1,500 msg/s (currently 506 msg/s — 3x improvement)
NFR-P4: Latency must not regress: p50 <= 1ms, p99 <= 5ms under batched workloads
NFR-P5: Memory RSS overhead from RocksDB tuning <= 512MB (currently 268MB idle)
NFR-P6: CPU efficiency improvement: >= 500 msg/s per CPU percent (currently ~117)

### Additional Requirements (from Architecture & Research)

- All optimizations must be measurable using the Epic 21 trustworthy benchmark suite (HdrHistogram, open-loop, multi-run)
- Benchmarks must run before AND after each optimization to quantify actual impact — no "expected impact" without measurement
- Latency must not regress while improving throughput. Any latency increase from batching must be opt-in and configurable
- Single-message path must remain unchanged — batching is additive, not a replacement
- Per-queue ordering guarantees must be preserved through all scheduler changes
- Fairness accuracy (NFR2: <5% overhead, NFR3: within 5% of fair share) must be maintained through all DRR changes
- RocksDB tuning should be verified against the RocksDB wiki "Implement Queue Service Using RocksDB" recommendations
- Existing 432 tests + e2e suite must pass after every story
- CI bench-regression workflow must detect improvements (update baselines) and catch regressions

### FR Coverage Map

FR-P1: Epic 22, Story 22.1 — RocksDB tuning
FR-P2: Epic 22, Story 22.1 — gRPC tuning (bundled — both are config-only, low-effort)
FR-P3: Epic 22, Story 22.2 — DRR data structure optimization
FR-P4: Epic 23, Story 23.1 — Client-side batching (SDK)
FR-P5: Epic 23, Story 23.1 — BatchEnqueue RPC (server-side, paired with SDK)
FR-P6: Epic 23, Story 23.2 — Server-side write coalescing
FR-P7: Epic 23, Story 23.3 — Delivery batching (consumer-side)
FR-P8: Epic 24, Story 24.1 — Zero-copy protobuf passthrough
FR-P9: Epic 24, Story 24.2 — Scheduler sharding
FR-P10: Epic 24, Story 24.1 — Key encoding optimization (bundled with zero-copy)
FR-P11: Epic 25 (deferred) — Purpose-built storage engine

## Epic List

### Epic 22: Tier 1 — Configuration Tuning & Data Structure Fixes
High-impact, low-effort optimizations that require no API changes. RocksDB queue-optimized configuration, gRPC HTTP/2 tuning, and DRR scheduler data structure fix. Target: 10K-15K msg/s (1KB), 3x improvement at 10K fairness keys.
**FRs covered:** FR-P1, FR-P2, FR-P3
**NFRs addressed:** NFR-P1, NFR-P3, NFR-P5
**Benchmark scenarios:** enqueue throughput, key cardinality scaling, streaming consumer throughput, e2e latency

### Epic 23: Tier 2 — Batching (Client, Server, Delivery)
The primary throughput lever. Client-side SDK batching, server-side write coalescing, and delivery batching for consumers. Requires proto changes (BatchEnqueue RPC), scheduler loop refactor, and SDK updates. Target: 30K-100K msg/s (1KB).
**FRs covered:** FR-P4, FR-P5, FR-P6, FR-P7
**NFRs addressed:** NFR-P2, NFR-P4, NFR1
**Benchmark scenarios:** batched vs unbatched throughput, latency-throughput tradeoff curves, lifecycle throughput

### Epic 24: Tier 3 — Zero-Copy & Scheduler Sharding
Diminishing-returns optimizations for squeezing out remaining headroom before architectural changes. Zero-copy protobuf passthrough, key encoding optimization, and multi-threaded scheduler sharding. Target: 50K-150K msg/s (1KB).
**FRs covered:** FR-P8, FR-P9, FR-P10
**NFRs addressed:** NFR-P6, NFR1

### Epic 25: Tier 4 — Purpose-Built Storage Engine (Deferred)
Replace RocksDB with an append-only log-segment storage engine tailored to queue access patterns. Only justified after Tiers 1-3 are exhausted and profiling confirms storage is the remaining bottleneck. Target: 200K-500K msg/s. FR66.
**FRs covered:** FR-P11 (FR66)
**NFRs addressed:** NFR30, NFR31, NFR32, NFR33
**Status:** Deferred until post-Tier 3 profiling justifies it.

### Epic 26: SDK Batch Operations & Auto-Batching
Bring all 5 external SDKs (Go, Python, JS, Ruby, Java) to feature parity with the Rust SDK for batch operations added in Epic 23. Also deliver the auto-batching with `linger_ms` timer deferred from Story 23.1. Without this epic, the primary throughput lever (batching) is only available to Rust SDK users.
**FRs covered:** FR-P4 (external SDKs), FR-P5 (external SDKs), FR-P7 (external SDKs)
**Prerequisite:** Epic 23 (BatchEnqueue RPC, delivery batching already on server)

### Epic 27: Profiling Infrastructure
Build profiling tooling so performance bottlenecks can be identified before optimizing. Epics 22-24 optimized based on theoretical predictions (10K-150K msg/s targets) without profiling — actual results were 2.7K msg/s. This epic ensures future performance work targets real bottlenecks. Includes flamegraph generation, subsystem-level benchmarks, and batch benchmark scenarios for the existing suite.

---

## Epic 22: Tier 1 — Configuration Tuning & Data Structure Fixes

High-impact, low-effort optimizations that require no API changes: RocksDB queue-optimized configuration, gRPC HTTP/2 tuning, and DRR scheduler data structure replacement. These are the lowest-risk, highest-certainty wins — configuration changes and a data structure swap with existing property tests for validation.

**Performance targets (measured by Epic 21 benchmark suite):**
- Enqueue throughput (1KB): >= 10K msg/s (from 2.7K baseline)
- 10K fairness key throughput: >= 1,500 msg/s (from 506 baseline)
- E2E latency p50: no regression (must stay <= 1ms)
- Streaming consumer throughput improvement measurable via consumer concurrency benchmark

### Story 22.1: RocksDB Queue-Optimized Configuration & gRPC Tuning

As an operator,
I want Fila to use RocksDB configuration optimized for queue access patterns and tuned gRPC settings,
So that throughput improves 3-10x without any API or behavioral changes.

**Acceptance Criteria:**

**Given** the RocksDB storage engine currently uses entirely default configuration
**When** queue-optimized settings are applied
**Then** a shared LRU block cache of 256MB is configured with `cache_index_and_filter_blocks = true` and `pin_l0_filter_and_index_blocks_in_cache = true`
**And** `enable_pipelined_write = true` is set (WAL and memtable writes run in parallel)
**And** `manual_wal_flush = true` with `wal_bytes_per_sync = 512KB` is set (buffered WAL — safe when Raft provides durability)
**And** the messages column family has: `write_buffer_size = 128MB`, `max_write_buffer_number = 4`, `min_write_buffer_number_to_merge = 2`
**And** the messages column family has 10-bit bloom filters enabled with `memtable_prefix_bloom_size_ratio = 0.1`
**And** the messages column family uses no compression on L0-L1 and LZ4 on L2+
**And** `CompactOnDeletionCollector` is enabled on the messages and raft_log column families (critical for queue's delete-heavy pattern — recommended by RocksDB wiki "Implement Queue Service Using RocksDB")
**And** the leases column family uses similar settings with 64MB write buffer (smaller scale)
**And** the lease_expiry column family has bloom filters disabled (range scans don't use them) and no compression
**And** `iterate_upper_bound` is set on all prefix scans to prevent iterators from walking past tombstones
**And** all RocksDB tuning settings are configurable via `fila.toml` under a `[storage.rocksdb]` section with the queue-optimized values as defaults
**And** the gRPC server is configured with: `initial_stream_window_size = 2MB`, `initial_connection_window_size = 4MB`, `tcp_nodelay = true`, `http2_keepalive_interval = 15s`, `http2_keepalive_timeout = 10s`
**And** the full benchmark suite (Epic 21) runs before and after the changes, with results compared in the PR description
**And** all 432 existing tests pass
**And** all e2e tests pass
**And** memory RSS is measured and documented (expected increase from 268MB to ~400-512MB due to larger block cache and write buffers)

### Story 22.2: DRR Scheduler O(1) Key Selection

As a developer,
I want the DRR scheduler to select the next eligible fairness key in O(1) time,
So that throughput at high key cardinality (1K-10K keys) matches low-cardinality performance.

**Acceptance Criteria:**

**Given** the current `next_key()` implementation in `drr.rs` does an O(n) linear scan of `active_keys: VecDeque<String>` to find the first key with positive deficit
**When** the data structure is replaced
**Then** an eligible-key set (or tiered queue structure) tracks which keys currently have positive deficit
**And** `next_key()` pops from the eligible set in O(1) time instead of scanning all active keys
**And** `consume_deficit()` removes keys from the eligible set when their deficit reaches zero
**And** `replenish_deficits()` rebuilds the eligible set when a new round starts
**And** all existing DRR property-based tests pass without modification (the behavior is identical, only the performance characteristic changes)
**And** the key cardinality benchmark shows: 10K keys >= 1,500 msg/s (from 506 baseline), 1K keys >= 1,500 msg/s (from 848 baseline)
**And** the 10-key benchmark shows no regression (>= 1,600 msg/s)
**And** fairness accuracy remains within 5% of fair share under sustained load (NFR3)
**And** the full benchmark suite runs before and after, with results compared in the PR description

---

## Epic 23: Tier 2 — Batching (Client, Server, Delivery)

The primary throughput lever. Every gRPC call currently processes exactly one message — the per-message overhead stack (HTTP/2 frame, protobuf decode, crossbeam hop, UUID, RocksDB put, crossbeam hop back, protobuf encode, HTTP/2 frame) doesn't amortize. Batching amortizes all of it. This is how Kafka achieves 534x higher throughput at 64B: `linger.ms=5` + `batch.size=16KB` means one network call carries ~16 messages, one disk write carries the entire batch.

**Performance targets:**
- Enqueue throughput (1KB, batched): >= 30K msg/s (from post-Tier-1 baseline of ~10K)
- Lifecycle throughput (enqueue+consume+ack, 1KB, batched): >= 15K msg/s (from 2.4K baseline)
- Latency: single-message path unchanged (p50 <= 1ms). Batched path latency increases by linger_ms (configurable, documented)
- Consumer throughput: >= 2x improvement with delivery batching

### Story 23.1: Client-Side Batching & BatchEnqueue RPC

As a developer using the Fila SDK,
I want to configure client-side message batching with linger time and batch size limits,
So that high-throughput producers can amortize per-message overhead without changing application code.

**Acceptance Criteria:**

**Given** the current SDK sends one message per `enqueue()` call
**When** batching is configured on the SDK client
**Then** a new `BatchEnqueue` RPC is added to the proto definition that accepts `repeated EnqueueRequest` and returns `repeated EnqueueResponse`
**And** the server-side handler processes all messages in the batch within a single `apply_mutations` call (one RocksDB WriteBatch for the entire batch)
**And** each message in the batch is independently validated — invalid messages get individual error responses without failing the batch
**And** per-queue ordering within a batch is preserved (messages to the same queue appear in batch order)
**And** the Rust SDK (`fila-sdk`) adds `BatchConfig` with: `linger_ms: Option<u64>` (time threshold, default None = disabled), `batch_size: Option<usize>` (max messages per batch, default 100)
**And** when `linger_ms` is set, the SDK accumulates messages in a per-queue buffer and flushes when either `linger_ms` elapses or `batch_size` is reached (whichever comes first)
**And** `enqueue()` returns a future that resolves when the batch containing that message is flushed and acknowledged
**And** when batching is disabled (default), the SDK uses the existing single-message `Enqueue` RPC — zero behavior change
**And** the benchmark suite measures batched throughput at various configurations: `linger_ms=1/batch_size=50`, `linger_ms=5/batch_size=100`, `linger_ms=10/batch_size=500`
**And** results show >= 10x throughput improvement over unbatched at 1KB with `linger_ms=5/batch_size=100`
**And** all existing tests pass (single-message path unchanged)
**And** new integration tests verify: batch enqueue correctness (all messages stored), partial failure handling (one bad message doesn't fail the batch), ordering preserved

### Story 23.2: Server-Side Write Coalescing

As a developer,
I want the scheduler to coalesce concurrent enqueue requests into batched storage writes,
So that even unbatched single-message clients benefit from reduced RocksDB write overhead under load.

**Acceptance Criteria:**

**Given** the current scheduler loop processes one command at a time from the crossbeam channel: receive → process → respond → receive
**When** write coalescing is implemented
**Then** the scheduler drains up to N commands from the channel in a tight loop (configurable, default N=100) or waits up to a coalescing window (configurable, default 1ms) — whichever triggers first
**And** all drained enqueue commands are processed together: UUID generation, Lua hooks, DRR updates run per-message, but the final `apply_mutations` call combines all messages into a single RocksDB WriteBatch
**And** responses are dispatched to all coalesced callers after the WriteBatch commits
**And** non-enqueue commands (ack, nack, admin) are processed inline — they don't wait for the coalescing window
**And** partial failures within a coalesced batch are handled: if one message's Lua hook fails, that message gets an error response but other messages in the batch succeed
**And** the coalescing window is configurable via `fila.toml` under `[scheduler]`: `write_coalesce_max_batch: 100`, `write_coalesce_window_us: 1000`
**And** when only one message is in the channel (low-load), processing is immediate — no artificial delay from the coalescing window
**And** the benchmark suite measures: unbatched throughput under concurrent load (10+ producers) before and after coalescing
**And** results show >= 2x throughput improvement for concurrent unbatched producers
**And** latency at light load (single producer) does not regress — no coalescing delay when there's no contention
**And** all existing tests pass
**And** new unit tests verify: single-message fast path (no delay), multi-message coalescing (N messages → 1 WriteBatch), partial failure handling

### Story 23.3: Delivery Batching (Consumer-Side)

As a consumer,
I want to receive multiple messages per gRPC streaming response,
So that consumer throughput improves by amortizing HTTP/2 framing and protobuf encoding overhead.

**Acceptance Criteria:**

**Given** the current consume stream sends one message per `StreamConsumeResponse`
**When** delivery batching is implemented
**Then** the proto `StreamConsumeResponse` is extended with `repeated Message messages` (alongside the existing single `message` field for backward compatibility)
**And** the server buffers delivered messages and flushes when either a count threshold (configurable, default 10) or a time threshold (configurable, default 1ms) is reached
**And** when a consumer processes messages slowly (one at a time), messages are sent individually — no artificial delay
**And** when the consumer is keeping up and messages are available, they're batched for maximum throughput
**And** each message in a delivery batch is independently leased (individual visibility timeouts)
**And** ack/nack still operates on individual message IDs — no batch-level acknowledgment (preserves existing semantics)
**And** the Rust SDK handles delivery batches transparently — the consumer callback/iterator sees individual messages regardless of batch size
**And** delivery batch configuration is set on the server: `fila.toml` → `[delivery]`: `batch_max_messages: 10`, `batch_window_us: 1000`
**And** benchmark suite measures: consumer throughput with 1/10/100 concurrent consumers, before and after delivery batching
**And** results show >= 1.5x consumer throughput improvement at 10+ concurrent consumers
**And** all existing tests pass (backward compatible — old clients see the existing `message` field)
**And** new integration tests verify: batch delivery correctness, individual ack/nack within a batch, visibility timeout per message

---

## Epic 24: Tier 3 — Zero-Copy & Scheduler Sharding

Diminishing-returns optimizations that extract remaining headroom before considering architectural changes. Zero-copy protobuf passthrough eliminates redundant serialization cycles. Scheduler sharding breaks the single-threaded bottleneck for multi-queue workloads. These are higher-effort with narrower impact than Tiers 1-2 — do them only after Tier 2 results are measured and the bottleneck is confirmed.

**Prerequisites:** Epic 22 and Epic 23 completed and benchmarked. Profiling data from post-Tier-2 benchmarks identifies whether the bottleneck is serialization (→ Story 24.1) or single-threaded scheduling (→ Story 24.2).

**Performance targets:**
- Enqueue throughput (1KB, batched): >= 50K msg/s single-queue
- Multi-queue throughput: near-linear scaling with shard count (4 shards ~= 4x for independent queues)
- CPU efficiency: >= 500 msg/s per CPU percent

### Story 24.1: Zero-Copy Protobuf Passthrough & Key Encoding

As a developer,
I want to eliminate redundant protobuf serialization and reduce per-message allocation overhead,
So that the enqueue and delivery hot paths do less CPU work per message.

**Acceptance Criteria:**

**Given** every enqueue currently: (1) deserializes the incoming protobuf request, (2) constructs a domain `Message`, (3) re-serializes to protobuf for storage; and every delivery: (1) reads raw bytes from RocksDB, (2) deserializes to domain `Message`, (3) re-serializes to gRPC response
**When** zero-copy passthrough is implemented
**Then** prost is configured with `bytes = "bytes"` for payload fields in `build.rs` so message payloads use `bytes::Bytes` (reference-counted, zero-copy clone)
**And** the enqueue path stores the already-serialized protobuf bytes directly when the message body is opaque — skipping domain type construction and re-serialization
**And** the delivery path reads raw bytes from RocksDB and constructs the gRPC response without full deserialization — only parsing the fields needed for DRR scheduling (fairness key, headers) via partial protobuf parsing
**And** messages that require Lua hook processing still go through full deserialization (the hook needs the domain type)
**And** storage key construction uses `Vec::with_capacity()` with known maximum key sizes instead of growing `Vec<u8>`
**And** hot-path key construction reuses a thread-local buffer where possible (scheduler thread owns a pre-allocated key buffer)
**And** the benchmark suite measures: enqueue throughput at 64B, 1KB, and 64KB before and after
**And** improvement is most visible at larger message sizes (64KB) where serialization overhead dominates
**And** all existing tests pass
**And** new tests verify: zero-copy path correctness (stored bytes round-trip correctly), Lua hook path still works (falls back to full deserialization), partial parse extracts fairness key correctly

### Story 24.2: Scheduler Sharding

As an operator running multiple queues,
I want independent queues to be processed in parallel by separate scheduler threads,
So that multi-queue workloads scale beyond the single-threaded scheduler bottleneck.

**Acceptance Criteria:**

**Given** the current scheduler runs all queues on a single dedicated OS thread
**When** scheduler sharding is implemented
**Then** the server starts N scheduler threads (configurable via `fila.toml` → `scheduler.shard_count`, default = 1 for backward compatibility)
**And** queues are assigned to shards by consistent hashing of the queue name — queue-to-shard assignment is stable across restarts
**And** each shard has its own crossbeam channel, DRR state, and storage write path
**And** gRPC handlers route requests to the correct shard based on queue name
**And** per-queue ordering is preserved — all messages for a queue go through the same shard
**And** cross-shard operations (GetStats, ListQueues) aggregate results from all shards
**And** admin commands (CreateQueue, DeleteQueue) are routed to the correct shard
**And** shard count changes (e.g., scaling from 2 to 4) are handled via queue reassignment on restart (not hot-rebalancing)
**And** the benchmark suite measures: multi-queue throughput (4 independent queues) with shard_count=1 vs shard_count=4
**And** results show near-linear scaling: 4-shard, 4-queue throughput >= 3x single-shard throughput
**And** single-queue throughput with shard_count=1 shows no regression from the sharding infrastructure
**And** all existing tests pass (default shard_count=1 = identical to current behavior)
**And** new integration tests verify: multi-shard routing correctness, per-queue ordering preserved across shards, cross-shard stats aggregation

---

## Epic 25: Tier 4 — Purpose-Built Storage Engine (Deferred)

Replace RocksDB with an append-only log-segment storage engine tailored to Fila's queue access pattern (append, read-head, delete-by-ID). This eliminates LSM write amplification (20-30x), tombstone accumulation, and enables segment-level GC. Only justified after Tiers 1-3 are exhausted and profiling confirms storage is the remaining bottleneck.

**Status:** Deferred. Effort: 3-6 months. Will be broken into stories when triggered by post-Tier-3 profiling.

**Trigger criteria (all must be met):**
1. RocksDB tuning (Epic 22) is applied and benchmarked
2. Batching (Epic 23) is applied and benchmarked
3. Profiling shows storage operations (RocksDB WriteBatch, compaction, WAL sync) are the dominant remaining bottleneck
4. Tombstone accumulation causes measurable latency spikes in sustained workloads
5. Target throughput (100K+ msg/s) cannot be reached with tuned RocksDB

**Preliminary story outline (to be refined when triggered):**
- 25.1: Append-only segment file format + write path
- 25.2: WAL for crash recovery + in-memory index for point lookups
- 25.3: Segment-level GC + fairness key index
- 25.4: Raft integration + `StorageEngine` trait implementation swap
- 25.5: Performance validation + migration tooling

**FRs covered:** FR-P11 (FR66)
**NFRs addressed:** NFR30 (2x RocksDB throughput), NFR31 (no compaction spikes > 10ms p99), NFR32 (efficient TTL expiry), NFR33 (< 1.5x storage overhead)
