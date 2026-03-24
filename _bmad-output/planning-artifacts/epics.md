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
  - '_bmad-output/planning-artifacts/research/market-message-broker-infrastructure-research-2026-03-04.md'
  - '_bmad-output/brainstorming/brainstorming-session-2026-03-04.md'
  - '_bmad-output/planning-artifacts/epics.md (existing epics 1-11)'
  - '_bmad-output/planning-artifacts/research/technical-benchmarking-methodology-research-2026-03-23.md'
---

# Fila - Epic Breakdown (Phase 2+)

## Overview

This document provides the epic and story breakdown for Fila's post-v1 roadmap (Epics 12+), decomposing the Phase 2+ requirements from the updated PRD, Architecture, market research, and brainstorming session into implementable stories. Epics 1-11 (Phase 1 MVP) are complete and documented separately. Epic 21 added 2026-03-23 based on benchmarking methodology research. Epics 22-25 added 2026-03-23 based on performance optimization research — detailed breakdown in `performance-optimization-epics.md`.

## Requirements Inventory

### Functional Requirements

FR63: Developers can run a continuous benchmark suite on every PR that detects performance regressions
FR64: Evaluators can compare published benchmark results of Fila against Kafka, RabbitMQ, and NATS for queue workloads
FR65: Operators can view a benchmark dashboard tracking throughput, latency percentiles, and resource usage over time
FR66: The broker can persist messages using a purpose-built storage engine optimized for queue access patterns, replacing RocksDB
FR67: Operators can deploy multi-node clusters with embedded Raft consensus
FR68: Operators can create queues without managing partitions — Fila distributes and rebalances automatically
FR69: Consumers can connect to any node and be routed to the correct partition transparently
FR70: Operators can view cluster-wide queue stats aggregated from all nodes
FR71: Operators can enable mTLS for transport-level security between clients and broker
FR72: Operators can authenticate clients via API keys
FR73: Operators can define per-queue access control policies
FR74: Developers can consult an SDK-server compatibility matrix with documented guarantees
FR75: Operators can deploy stability release branches with backported fixes
FR76: Operators can monitor queues and visualize scheduling state via a web-based management GUI
FR77: Consumers can join broker-managed consumer groups with automatic rebalancing
FR78: Script authors can use built-in Lua helpers for common patterns (exponential backoff, tenant routing)
FR-P1: Server applies queue-optimized RocksDB configuration (bloom filters, block cache, pipelined writes, CompactOnDeletionCollector)
FR-P2: Server configures gRPC/HTTP/2 flow control and keepalive for high-throughput streaming
FR-P3: DRR scheduler uses O(1) data structure for key selection instead of O(n) linear scan
FR-P4: SDK supports client-side batching with configurable linger_ms and batch_size
FR-P5: Server supports BatchEnqueue RPC for multi-message writes in a single WriteBatch
FR-P6: Scheduler coalesces concurrent enqueue requests into batched writes
FR-P7: Consumer streaming delivers multiple messages per gRPC response frame
FR-P8: Message payload uses bytes::Bytes for zero-copy passthrough
FR-P9: Scheduler supports sharded execution across multiple threads
FR-P10: Storage key encoding uses pre-allocated buffers
FR-P11: Purpose-built append-only storage engine (FR66, deferred)

### NonFunctional Requirements

NFR22: Linear throughput scaling: 2 nodes >= 1.8x single-node throughput
NFR23: Automatic failover within 10 seconds of node failure detection
NFR24: Zero message loss during planned node additions and removals
NFR25: Consumer reconnection to healthy nodes within 5 seconds of partition
NFR26: Cluster state convergence within 30 seconds of membership change
NFR27: mTLS handshake < 5ms overhead per connection
NFR28: API key validation < 100us per request
NFR29: Secure defaults — authentication required unless explicitly disabled
NFR30: Queue-optimized write throughput >= 2x RocksDB for append-heavy workloads
NFR31: Predictable latency — no compaction-induced latency spikes > 10ms p99
NFR32: Efficient TTL expiry without full-table scans
NFR33: Storage footprint < 1.5x raw message data (overhead from indexing and metadata)
NFR-P1: Enqueue throughput >= 10K msg/s (1KB) after Tier 1 optimizations
NFR-P2: Enqueue throughput >= 30K msg/s (1KB) after Tier 2 optimizations (with batching)
NFR-P3: 10K fairness key throughput >= 1,500 msg/s (from 506 baseline)
NFR-P4: Latency must not regress under batched workloads (p50 <= 1ms, p99 <= 5ms)
NFR-P5: Memory RSS overhead from RocksDB tuning <= 512MB
NFR-P6: CPU efficiency >= 500 msg/s per CPU percent

### Additional Requirements

- Storage engine and clustering are a coupled workstream — must be designed together. Clean storage trait abstraction enables future engine swaps. CockroachDB-style Raft-per-queue model: shard by queue, not by partition. RocksDB is sufficient as Raft state machine backend; purpose-built engine is a future optimization.
- "Zero graduation" positioning requires published benchmark data — teams consume published benchmarks during evaluation, rarely run their own.
- Benchmark yourself first (throughput ceiling, latency percentiles, RocksDB compaction impact, memory footprint), competitive comparison second.
- Continuous benchmarks on PRs — shift-left, not just per-release.
- Single binary must stay single binary — embedded Raft, no external consensus dependencies. Follows etcd/CockroachDB precedent.
- Queue semantics never leak the log — no offsets, no rebalancing exposed to users. Invisible partitioning.
- Auth minimum viable — mTLS + API keys. Not full RBAC from day one.
- Release engineering — versioning scheme TBD (semver vs calver). SDK compatibility contract needed. Distribution channels: Homebrew, apt beyond existing curl|bash/cargo install/Docker.
- DX items (web dashboard, consumer groups, Lua helpers) are lower priority.
- Memphis cautionary tale — "simpler than Kafka" alone is not durable. Fairness + Lua scripting are the differentiators.

### FR Coverage Map

FR63: Epic 12 — Continuous benchmark suite on every PR
FR64: Epic 12 — Published competitive benchmarks vs Kafka/RabbitMQ/NATS
FR65: Epic 12 — Benchmark dashboard — throughput, latency, resources over time
FR66: Deferred (post-clustering optimization) — Purpose-built storage engine replacing RocksDB
FR67: Epic 14 — Multi-node clusters with embedded Raft (Raft-per-queue model)
FR68: Epic 14 — Automatic queue distribution across cluster nodes
FR69: Epic 14 — Transparent consumer routing to queue's Raft leader
FR70: Epic 14 — Cluster-wide aggregated queue stats
FR71: Epic 15 — mTLS for transport security
FR72: Epic 15 — API key authentication
FR73: Epic 15 — Per-queue access control policies
FR74: Epic 16 — Versioning & compatibility policy docs
FR75: Deferred — Stability release branches (premature pre-1.0)
Stability/hardening — Epic 16.5 (quality infrastructure, strengthens NFR22-29 coverage, no new FRs)
FR76: Epic 20 — Web-based management GUI
FR77: Epic 18 — Broker-managed consumer groups
FR78: Epic 19 — Built-in Lua helpers for common patterns (DX epic)
GH#63: Epic 17 — Distinguish 'node not ready' from 'queue not found' errors
GH#64: Epic 17 — Ack/nack linear scan fix in Raft apply path
GH#65: Epic 17 — Consume on non-leader returns leader hint (+ SDK updates)
GH#66: Epic 17 — Automatic queue-to-node assignment for balanced leadership
FR-B1: Epic 21 — Replace LatencySampler with HdrHistogram
FR-B2: Epic 21 — Increase latency sample count to 10,000+
FR-B3: Epic 21 — Report p99.9, p99.99, max in all latency benchmarks
FR-B4: Epic 21 — Increase measurement duration to 30s+ (configurable)
FR-B5: Epic 21 — Run competitive benchmarks 3x with median aggregation
FR-B6: Epic 21 — Open-loop load generation mode
FR-B7: Epic 21 — Concurrent produce/consume for competitive latency
FR-B8: Epic 21 — Consumer processing time simulation (0/1/10/100ms)
FR-B9: Epic 21 — Backpressure ramp test (10%→150% capacity)
FR-B10: Epic 21 — Queue depth effect on latency test
FR-B11: Epic 21 — Nack storm / DLQ routing / failure-path benchmarks
FR-B12: Epic 21 — Jain's Fairness Index for fairness tests
FR-B13: Epic 21 — Disk I/O measurement in competitive resource benchmarks
FR-B14: Epic 21 — Emit github-action-benchmark JSON format for CI visualization
FR-P1: Epic 22 — RocksDB queue-optimized configuration
FR-P2: Epic 22 — gRPC HTTP/2 tuning
FR-P3: Epic 22 — DRR O(1) key selection
FR-P4: Epic 23 — Client-side SDK batching
FR-P5: Epic 23 — BatchEnqueue RPC
FR-P6: Epic 23 — Server-side write coalescing
FR-P7: Epic 23 — Delivery batching
FR-P8: Epic 24 — Zero-copy protobuf passthrough
FR-P9: Epic 24 — Scheduler sharding
FR-P10: Epic 24 — Key encoding optimization
FR-P11: Epic 25 (deferred) — Purpose-built storage engine (FR66)

## Epic List

### Epic 12: Benchmarks & Competitive Positioning
Developers get automatic performance regression detection on every PR. Evaluators can compare Fila's published benchmark data against Kafka, RabbitMQ, and NATS for queue workloads. Operators can track throughput, latency percentiles, and resource usage over time. This is the data-driven foundation — you can't improve what you can't measure.
**FRs covered:** FR63, FR64, FR65

### Epic 13: Storage Abstraction & Clustering Prep
Clean storage trait abstraction using Fila-domain terms (not RocksDB internals) and phase 2 viability seams for the Raft-per-queue clustering model. RocksDB remains the storage engine — under Raft it serves as a local state machine backend, making a custom engine a future optimization rather than a prerequisite. The storage trait enables future engine swaps and provides a clean interface for Raft state machine application.
**FRs covered:** (preparatory — enables FR67-FR70)
**Note:** FR66 (purpose-built storage engine) deferred to post-clustering. NFR30-33 deferred with it.

### Epic 14: Clustering & Horizontal Scaling
Operators can deploy multi-node Fila clusters with embedded Raft consensus — zero external dependencies, single binary stays single binary. CockroachDB-style single-binary model: every node runs the same code (storage + scheduler + gateway), cluster self-organizes. Each queue is a Raft group with one leader handling scheduling, storage, and delivery. Followers replicate everything via Raft. Users create queues; Fila distributes them across nodes automatically. Queue semantics never leak — no offsets, no partitions exposed to users.
**FRs covered:** FR67, FR68, FR69, FR70
**NFRs addressed:** NFR22, NFR23, NFR24, NFR25, NFR26

### Epic 15: Authentication & Security
Operators can deploy Fila in real production environments with transport security and client authentication. mTLS secures the wire, API keys authenticate clients, per-queue ACLs control access. Secure defaults — authentication required unless explicitly disabled.
**FRs covered:** FR71, FR72, FR73
**NFRs addressed:** NFR27, NFR28, NFR29

### Epic 16: Release Engineering & SDK Compatibility
Versioning policy and proto backward compatibility formalized. All 5 external SDKs updated with TLS and API key auth support. Reshaped from 3 to 2 stories — stability release branches deferred pre-1.0.
**FRs covered:** FR74

### Epic 16.5: Stability Hardening & Test Coverage
Systematic hardening of the highest-risk subsystems — clustering, TLS, and auth — through blackbox e2e testing, edge case coverage, and CI-integrated code coverage. Ensures the foundation is solid before building further features. Triggered by Epic 16 retro finding: "silent TLS downgrade is a universal SDK bug pattern."
**FRs covered:** (hardening — strengthens NFR22-29 coverage, no new FRs)

### Epic 17: Cluster Hardening
Production-readiness fixes for the Raft clustering layer. Error clarity (distinguish node-not-ready from queue-not-found), performance (eliminate O(n) ack/nack scan in Raft apply path), consume routing (leader hint + SDK reconnect), and load-balanced queue-to-node assignment. All items sourced from GitHub issues #63-#66 discovered during Epic 14 work.
**FRs covered:** (hardening — improves FR67-FR70 quality, no new FRs)
**GitHub issues:** #63, #64, #65, #66

### Epic 18: Consumer Groups (deferred — design rework needed)
Consumers can join broker-managed consumer groups where each group gets an independent view of every message (Kafka-style semantics). Within a group, messages are distributed so each is processed by exactly one member. Requires fundamental rearchitecture: per-group delivery state, per-group DRR scheduling, per-group ack/nack lifecycle, per-group throttle/Lua hooks. Original implementation (PR #85) shipped wrong semantics (groups split throughput) and was reverted. See `_bmad-output/planning-artifacts/research/consumer-group-semantics.md` for design analysis.
**FRs covered:** FR77
**Status:** Deferred. Stories TBD pending design decisions.

### Epic 19: Developer Experience
Docs website, deployment guides, Helm charts, Lua helpers, and DX sugar. The "make it easy to adopt and operate" epic — shipped after all features are in so documentation covers the complete system. Includes built-in Lua helper functions for common patterns (exponential backoff, tenant routing, rate limit keys, max retries).
**FRs covered:** FR78

### Epic 20: Web Management GUI
Operators can monitor queues and visualize real-time scheduling state via a web-based management interface. Read-only dashboard bundled into the server binary, showing queue depths, DRR state, throttle status, consumer connections, and cluster topology. Optional — disabled by default, zero overhead when disabled.
**FRs covered:** FR76

### Epic 21: Trustworthy Benchmark Suite
Developers and evaluators can trust Fila's benchmark numbers for optimization decisions and competitive claims — with statistically valid latency measurement (HdrHistogram, CO correction, 10K+ samples), realistic workload profiles (open-loop, processing delay, backpressure, failure paths), and reproducible results (multi-run aggregation, histogram merging). Builds on Epic 12's benchmark infrastructure with methodology fixes identified in the benchmarking methodology research.
**FRs covered:** FR-B1 through FR-B14
**NFRs addressed:** NFR-B1 (reproducibility), NFR-B2 (CI time), NFR-B3 (measurement overhead)

### Epic 22: Tier 1 — Configuration Tuning & Data Structure Fixes
High-impact, low-effort optimizations: RocksDB queue-optimized configuration (bloom filters, block cache, pipelined writes, CompactOnDeletionCollector), gRPC HTTP/2 tuning (window sizes, keepalive, tcp_nodelay), and DRR scheduler O(1) key selection. No API changes. Target: 10K-15K msg/s (1KB). Full breakdown in `performance-optimization-epics.md`.
**FRs covered:** FR-P1, FR-P2, FR-P3
**NFRs addressed:** NFR-P1, NFR-P3, NFR-P5

### Epic 23: Tier 2 — Batching (Client, Server, Delivery)
The primary throughput lever. Client-side SDK batching (linger_ms + batch_size), BatchEnqueue RPC, server-side write coalescing, and delivery batching for consumers. Requires proto changes and scheduler loop refactor. Target: 30K-100K msg/s (1KB). Full breakdown in `performance-optimization-epics.md`.
**FRs covered:** FR-P4, FR-P5, FR-P6, FR-P7
**NFRs addressed:** NFR-P2, NFR-P4, NFR1

### Epic 24: Tier 3 — Zero-Copy & Scheduler Sharding
Diminishing-returns optimizations: zero-copy protobuf passthrough (bytes::Bytes, skip re-serialization), key encoding optimization, and multi-threaded scheduler sharding. Only justified after Tier 2 profiling. Target: 50K-150K msg/s (1KB). Full breakdown in `performance-optimization-epics.md`.
**FRs covered:** FR-P8, FR-P9, FR-P10
**NFRs addressed:** NFR-P6, NFR1

### Epic 25: Tier 4 — Purpose-Built Storage Engine (Deferred)
Replace RocksDB with append-only log-segment storage engine. Deferred until Tiers 1-3 are exhausted and profiling confirms storage is the remaining bottleneck. Target: 200K-500K msg/s. Full breakdown in `performance-optimization-epics.md`.
**FRs covered:** FR-P11 (FR66)
**NFRs addressed:** NFR30, NFR31, NFR32, NFR33
**Status:** Deferred. Stories TBD pending post-Tier-3 profiling.

### Epic 26: SDK Batch Operations & Auto-Batching
Bring all 5 external SDKs to batch operation parity with the Rust SDK and deliver auto-batching. Full breakdown in `performance-optimization-epics.md`.

### Epic 27: Profiling Infrastructure
Build profiling tooling (flamegraphs, subsystem benchmarks, batch benchmark scenarios) so future performance work targets real bottlenecks. Full breakdown in `performance-optimization-epics.md`.

### Epic 29: Transport Optimization — Fair Benchmarks & Streaming Enqueue
Fix the competitive benchmark to use batching (fair comparison), add bidirectional streaming `StreamEnqueue` RPC, and integrate streaming transparently into the Rust SDK. Profiling showed 94% of per-message time is gRPC/HTTP2 overhead. Full breakdown in `transport-optimization-epics.md`.

---

## Epic 12: Benchmarks & Competitive Positioning

Developers get automatic performance regression detection on every PR. Evaluators can compare Fila's published benchmark data against Kafka, RabbitMQ, and NATS for queue workloads. Operators can track throughput, latency percentiles, and resource usage over time. This is the data-driven foundation — you can't improve what you can't measure.

### Story 12.1: Benchmark Harness & Self-Benchmarking

As a developer,
I want a comprehensive benchmark suite that measures Fila's single-node performance across all critical dimensions,
So that we have a quantified baseline for optimization and can validate Phase 1 NFR targets.

**Acceptance Criteria:**

**Given** the Fila server is running
**When** the benchmark suite executes
**Then** it measures single-node enqueue throughput (msg/s) for 1KB payload messages
**And** it measures enqueue-to-consume latency at p50/p95/p99 under varying load levels (light, moderate, saturated)
**And** it measures throughput with fair scheduling enabled vs raw FIFO to validate NFR2 (<5% overhead)
**And** it measures fairness accuracy under sustained load across 5+ keys with varying weights to validate NFR3 (within 5%)
**And** it measures Lua `on_enqueue` execution latency at p99 to validate NFR4 (<50us)
**And** it measures queue depth scaling: enqueue/consume throughput at 1M and 10M queued messages
**And** it measures fairness key cardinality impact: 10, 1K, 10K, and 100K active keys
**And** it measures consumer concurrency impact: 1, 10, and 100 simultaneous consumers
**And** it measures memory footprint under load (RSS)
**And** it measures RocksDB compaction impact on tail latency (p99 during active compaction vs idle)
**And** benchmark results are output in machine-readable format (JSON) for CI consumption
**And** the benchmark crate is added to the Cargo workspace as `fila-bench`
**And** `cargo bench -p fila-bench` runs the full suite
**And** CI pipeline is updated to include the new crate (build + clippy)

### Story 12.2: CI Regression Detection

As a developer,
I want automatic performance regression detection on every PR,
So that performance degradation is caught before merge.

**Acceptance Criteria:**

**Given** a PR is opened against the repository
**When** the CI benchmark workflow runs
**Then** the benchmark suite from Story 12.1 executes on the PR branch
**And** results are compared against stored baselines from the main branch
**And** if any key metric regresses beyond a configurable threshold (default: 10%), the CI check fails
**And** the PR gets a comment with a summary table showing metric changes (improved / regressed / unchanged)
**And** baseline results are automatically updated when PRs merge to main
**And** baseline storage uses GitHub Actions cache or artifact storage
**And** the workflow uses statistical methods (multiple runs, median of N) to reduce CI environment variance
**And** developers can manually update baselines via workflow dispatch when intentional trade-offs are made
**And** the benchmark workflow is triggered on the feature branch to verify it works before merge (per CLAUDE.md CI workflow verification rule)

### Story 12.3: Competitive Benchmarks

As an evaluator,
I want to compare Fila's benchmark results against Kafka, RabbitMQ, and NATS for queue workloads,
So that I can make informed adoption decisions based on data.

**Acceptance Criteria:**

**Given** Docker Compose configurations exist for each competitor (Kafka, RabbitMQ, NATS)
**When** the competitive benchmark suite runs
**Then** each broker is tested with identical workloads: single-producer/single-consumer throughput, fan-out (1 producer / N consumers), multi-producer, and varying message sizes (64B, 1KB, 64KB)
**And** queue-specific workloads are tested: enqueue → consume → ack lifecycle throughput, visibility timeout / redelivery overhead
**And** Fila-only workloads are included: fair scheduling overhead, throttle-aware delivery (no equivalent in competitors)
**And** latency is measured at p50/p95/p99 for each broker under identical load
**And** the methodology is documented: hardware specs, broker configuration, warmup period, measurement window, number of runs
**And** results are reproducible: a single make target (e.g., `make bench-competitive`) runs the full suite locally
**And** competitor configurations use recommended production settings (not default development settings)
**And** results include resource utilization: CPU, memory, disk I/O per broker during the benchmark

### Story 12.4: Published Results & Benchmark Dashboard

As an evaluator,
I want to view Fila's benchmark results and competitive positioning in published form,
So that I can reference performance data during architecture evaluation.

**Acceptance Criteria:**

**Given** benchmark results exist from Stories 12.1 and 12.3
**When** the results are published
**Then** a `docs/benchmarks.md` page presents self-benchmark results with formatted tables for throughput, latency percentiles, and scaling curves
**And** the competitive comparison is presented as tables with clear methodology links
**And** the README includes a performance summary line linking to the full benchmarks page
**And** historical benchmark results are tracked in the repository (CI updates results on each release)
**And** the benchmark methodology page is detailed enough for external reproduction: exact commands, hardware specs, configuration files
**And** the page acknowledges limitations: hardware-specific results, configuration choices, workload representativeness
**And** results include the Fila version and commit hash for traceability

---

## Epic 13: Storage Abstraction & Clustering Prep

Clean storage trait abstraction using Fila-domain terms (not RocksDB internals) and phase 2 viability seams for the Raft-per-queue clustering model. RocksDB remains the storage engine — under Raft it serves as a local state machine backend, making a custom engine a future optimization rather than a prerequisite. The storage trait enables future engine swaps and provides a clean interface for Raft state machine application.

> **Context:** This epic was reshaped from a 5-story purpose-built storage engine epic based on clustering architecture research (see `_bmad/docs/research/decoupled-scheduler-sharded-storage.md`). The research found that RocksDB is sufficient under Raft and that sharding should be by queue, not by Kafka-style partitions. The original Epic 13 PRs (#49-53) were closed without merge.

### Story 13.1: Clean Storage Trait Abstraction

As a developer,
I want a clean storage engine trait using Fila-domain terms,
So that the storage implementation can be swapped without changing broker logic, and the interface is ready for Raft state machine application.

**Acceptance Criteria:**

**Given** the existing codebase uses RocksDB directly throughout fila-core
**When** the storage abstraction is implemented
**Then** a `StorageEngine` trait is defined with methods covering all current storage operations: message CRUD, lease management, queue config, state/config operations, expiry scanning
**And** the trait uses Fila-domain terms: message store, lease store, config store — not RocksDB concepts (column families, raw iterators, write batches)
**And** the trait does NOT use PartitionId — queues are the unit of distribution in the Raft-per-queue model
**And** the trait supports atomic batch mutations (`apply_mutations(batch)`) suitable for Raft state machine application (applying committed log entries)
**And** a `RocksDbEngine` struct implements the `StorageEngine` trait, wrapping all existing RocksDB logic
**And** all broker and scheduler code is migrated from direct RocksDB calls to `StorageEngine` trait methods
**And** all existing unit and integration tests pass without modification to test assertions (only internal wiring changes)
**And** the e2e test suite (11 tests) passes with the RocksDB adapter
**And** the trait is defined in fila-core with no RocksDB-specific types in the trait interface (RocksDB is an implementation detail)

### Story 13.2: Phase 2 Viability Seams

As a developer,
I want thin architectural seams that enable future hierarchical queue scaling,
So that phase 2 (splitting hot queues across multiple Raft groups) is a matter of implementing new logic behind existing interfaces, not rearchitecting the core.

**Acceptance Criteria:**

**Given** the storage trait and broker code from Story 13.1
**When** viability seams are added
**Then** a routing indirection layer maps `(queue, fairness_key)` → `RaftGroup` — in phase 1 the implementation is trivial (every fairness key in a queue maps to the same group, 1:1), but the indirection exists in the code path
**And** DRR is scoped to a key-set parameter: the scheduler runs DRR over "the fairness keys I'm responsible for" — in phase 1 this happens to be all keys in the queue, but the scope is explicit, not hardcoded
**And** each queue emits aggregate scheduling stats as OTel metrics: messages scheduled per fairness key, current deficit state, throughput — in phase 1 these are consumed only for observability
**And** the enqueue path threads `fairness_key` through the routing decision: routing is by `(queue, fairness_key)`, not just queue — in phase 1 the fairness key is ignored in routing (all go to the same group)
**And** all 278 tests + 11 e2e tests pass with zero behavioral changes
**And** no speculative abstractions or premature engineering — these are thin seams, not full implementations

---

## Epic 14: Clustering & Horizontal Scaling

Operators can deploy multi-node Fila clusters with embedded Raft consensus — zero external dependencies, single binary stays single binary. CockroachDB-style single-binary model: every node runs the same code (storage + scheduler + gateway), cluster self-organizes. Each queue is a Raft group with one leader handling scheduling, storage, and delivery for that queue. Followers replicate everything via Raft. Users create queues; Fila distributes them across nodes automatically. Queue semantics never leak — no offsets, no partitions exposed to users. The "zero graduation" vision realized: scales like Kafka, works like a queue.

> **Architecture:** See `_bmad/docs/research/decoupled-scheduler-sharded-storage.md` for the full research and design rationale.

### Story 14.1: Raft Integration & Single-Node Mode

As a developer,
I want Raft consensus embedded in the Fila binary with zero overhead in single-node mode,
So that clustering is built into the same binary without affecting existing single-node deployments.

**Acceptance Criteria:**

**Given** Fila runs as a single binary
**When** cluster mode is configured
**Then** Fila embeds a Raft consensus implementation (e.g., openraft) compiled into the same binary — no external consensus service required
**And** cluster configuration is specified via `fila.toml`: `cluster.enabled`, `cluster.node_id`, `cluster.peers` (initial peer list), `cluster.bind_addr` (intra-cluster communication)
**And** a single-node cluster can be bootstrapped with `cluster.bootstrap = true`
**And** additional nodes join an existing cluster by specifying seed peers
**And** leader election completes within the Raft election timeout (configurable, default 1 second)
**And** cluster membership changes (add/remove node) are committed via Raft log entries
**And** the Raft state machine applies committed entries to local state: message writes, DRR state, leases, pending index, config — there is no separate "storage" vs "scheduler" replication, it's one Raft log per queue
**And** intra-cluster communication uses a dedicated gRPC service (separate from client-facing RPCs)
**And** single-node mode (`cluster.enabled = false`, the default) continues to work exactly as before — zero Raft overhead, zero behavior change
**And** integration tests verify: 3-node cluster bootstrap, leader election, membership change (add 4th node, remove a node)

### Story 14.2: Queue-Level Raft Groups & Assignment

As an operator,
I want each queue to be its own Raft group distributed across the cluster,
So that queues scale independently and failure of one queue's leader doesn't affect other queues.

**Acceptance Criteria:**

**Given** a Fila cluster is running with multiple nodes (from Story 14.1)
**When** an operator creates a queue
**Then** a new Raft group is created for that queue with all N nodes as replicas (or a configurable subset for large clusters)
**And** the queue → Raft group mapping is stored in a placement table (using the routing indirection from Epic 13 Story 13.2)
**And** one node is elected Raft leader for the queue — the leader handles all scheduling, storage writes, and consumer delivery for that queue
**And** leadership is balanced across nodes automatically (different queues have different leaders)
**And** fencing tokens (Raft term number) are included on every scheduling operation — stale leaders are rejected
**And** deleting a queue removes its Raft group
**And** queue creation, deletion, and management RPCs work from any node (forwarded to leader if needed)
**And** operators never interact with Raft groups directly — `CreateQueue` and `DeleteQueue` RPCs are unchanged (FR68)
**And** adding a node → it joins as Raft follower for existing queue groups, cluster rebalances some queue leaderships to it
**And** removing a node → its queue leaderships transfer to other nodes in 1-2 seconds, followers already have full state, zero data migration needed
**And** integration tests verify: create queues on 3-node cluster, verify leadership distributed, add 4th node, verify leadership rebalance, remove a node, verify leadership transfer

### Story 14.3: Request Routing & Transparent Delivery

As a consumer,
I want to connect to any Fila node and have my requests served correctly,
So that I don't need to know which node is the leader for which queue.

**Acceptance Criteria:**

**Given** a multi-node cluster with queue-level Raft groups (from Story 14.2)
**When** a client sends an Enqueue request to any node
**Then** the receiving node routes the request to the queue's Raft leader
**And** the leader commits the message to the Raft log — ack-after-replicate: message is committed to a quorum before the producer receives acknowledgment (NFR24)
**And** routing is transparent — the client receives a normal response regardless of which node it connected to

**Given** a client opens a Consume stream on any node
**When** the queue's leader is on a different node
**Then** the consuming node proxies the stream to the queue's Raft leader — the leader handles all DRR scheduling for its queues (no cross-node DRR merging needed)
**And** lease records are committed to Raft-replicated state before the message is sent to the consumer
**And** Ack and Nack requests are routed to the queue's Raft leader
**And** routing adds minimal latency overhead (one network hop for cross-node requests, zero for direct leader connections)
**And** SDK connection strings accept multiple node addresses for automatic failover
**And** integration tests verify: producer enqueues via node A, consumer receives via node B, ack via node C — full lifecycle across nodes

### Story 14.4: Replication, Failover & Recovery

As an operator,
I want automatic failover when a node goes down,
So that message processing continues without manual intervention or data loss.

**Acceptance Criteria:**

**Given** a multi-node cluster with queue-level Raft groups (from Stories 14.1–14.3)
**When** data is written for a queue
**Then** the Raft leader replicates everything via its Raft log: message data, DRR deficits, leases, pending index, scheduler metadata — followers have full replicated state at all times
**And** writes are committed only after a quorum of replicas acknowledge
**And** zero messages are lost during planned node additions and removals (NFR24)

**Given** a node fails unexpectedly
**When** the Raft followers detect the failure via heartbeat timeout
**Then** a new leader is elected from followers (who already have full state) within 1-2 seconds
**And** automatic failover completes within 10 seconds (NFR23) — no data migration, no state reconstruction
**And** consumer streams connected to the failed node receive a disconnection
**And** consumers reconnect to healthy nodes within 5 seconds (NFR25) — SDKs handle reconnection automatically
**And** in-flight messages are governed by their visibility timeout (at-least-once delivery preserved)

**Given** a failed node recovers
**When** it rejoins the cluster
**Then** it catches up from the Raft log (or receives a Raft snapshot from the leader if too far behind)
**And** cluster state converges within 30 seconds of membership change (NFR26)
**And** cluster rebalances some queue leaderships to the recovered node
**And** integration tests verify: 3-node cluster, kill one node, verify failover < 10s, verify zero message loss, restart node, verify rejoin and convergence

### Story 14.5: Cluster Observability & Scaling Validation

As an operator,
I want to view aggregated stats across all cluster nodes and verify linear scaling,
So that I can monitor the cluster as a single system and trust that adding nodes increases capacity.

**Acceptance Criteria:**

**Given** a multi-node cluster is operational (from Stories 14.1–14.4)
**When** an operator calls GetStats
**Then** the response includes cluster-wide aggregated metrics: total queue depth, total throughput, per-node breakdown (FR70)
**And** per-queue stats show which node is the Raft leader for each queue
**And** cluster health is reported: node count, per-queue leader distribution, replication status per queue group
**And** OTel metrics include cluster-level dimensions: `node_id` labels on existing metrics, cluster-level rollup metrics
**And** the CLI `fila queue inspect <name>` shows that queue's stats when connected to any node

**Given** a 2-node cluster is benchmarked using the Epic 12 benchmark suite
**When** throughput is measured across multiple queues
**Then** aggregate throughput is >= 1.8x single-node throughput (NFR22: linear scaling across queues)
**And** the benchmark methodology documents how to reproduce the scaling test
**And** integration tests verify: GetStats returns correct aggregated counts across a 3-node cluster

---

## Epic 15: Authentication & Security

Operators can deploy Fila in real production environments with transport security and client authentication. mTLS secures the wire, API keys authenticate clients, per-queue ACLs control access. Secure defaults — authentication required unless explicitly disabled.

### Story 15.1: mTLS Transport Security

As an operator,
I want to enable mutual TLS on the Fila server,
So that all client-broker communication is encrypted and mutually authenticated.

**Acceptance Criteria:**

**Given** a Fila server is configured with TLS certificates
**When** mTLS is enabled via configuration
**Then** `fila.toml` accepts TLS configuration: `tls.enabled`, `tls.cert_file`, `tls.key_file`, `tls.ca_file` (for client certificate verification)
**And** the gRPC server listens on TLS-secured connections using the configured certificates
**And** clients must present a valid certificate signed by the configured CA
**And** connections without valid client certificates are rejected at the TLS handshake
**And** mTLS handshake adds < 5ms overhead per connection establishment (NFR27)
**And** intra-cluster gRPC communication (from Epic 14) also uses mTLS when TLS is enabled
**And** all 6 SDKs support TLS configuration: CA cert, client cert, client key
**And** the CLI supports TLS flags: `--tls-cert`, `--tls-key`, `--ca-cert`
**And** when TLS is disabled (default for backward compatibility), the server behaves exactly as before
**And** integration tests verify: TLS connection succeeds with valid certs, connection rejected with invalid cert, connection rejected without cert when mTLS is required

### Story 15.2: API Key Authentication

As an operator,
I want to authenticate clients using API keys,
So that I can control which clients can access the broker.

**Acceptance Criteria:**

**Given** a Fila server has API key authentication enabled
**When** API key auth is configured
**Then** `fila.toml` accepts: `auth.enabled`, `auth.type = "api_key"`
**And** API keys are managed via admin RPCs: `CreateApiKey` (returns key + key_id), `RevokeApiKey`, `ListApiKeys`
**And** API keys are stored hashed (SHA-256) in the broker's persistent state
**And** clients include the API key in gRPC metadata (`authorization: Bearer <key>`)
**And** every RPC validates the API key before processing — invalid or missing keys return `UNAUTHENTICATED` status
**And** API key validation adds < 100us overhead per request (NFR28)
**And** API keys have an optional expiration time
**And** key creation and revocation are audit-logged
**And** all 6 SDKs accept an `api_key` parameter in their connection configuration
**And** the CLI accepts `--api-key` flag
**And** when auth is disabled (default), no authentication is required — backward compatible
**And** integration tests verify: request succeeds with valid key, request rejected with invalid key, request rejected without key when auth enabled, revoked key is rejected

### Story 15.3: Per-Queue Access Control

As an operator,
I want to define access control policies per queue,
So that I can restrict which clients can produce to or consume from specific queues.

**Acceptance Criteria:**

**Given** API key authentication is enabled (from Story 15.2)
**When** ACL policies are configured
**Then** each API key can be associated with a set of permissions: `produce:<queue_pattern>`, `consume:<queue_pattern>`, `admin:<queue_pattern>`
**And** queue patterns support wildcards: `*` matches any queue, `orders.*` matches `orders.us`, `orders.eu`, etc.
**And** permissions are checked on every RPC: Enqueue checks `produce`, Consume checks `consume`, admin RPCs check `admin`
**And** unauthorized operations return `PERMISSION_DENIED` status with a descriptive message
**And** ACL policies are managed via admin RPCs: `SetAcl` (associate permissions with a key_id), `GetAcl`
**And** a superadmin key type bypasses ACL checks (for operators)
**And** ACL changes take effect immediately — no restart required
**And** secure defaults: when auth is enabled, new API keys have no permissions until explicitly granted (NFR29)
**And** integration tests verify: key with produce-only can enqueue but not consume, key with consume-only can consume but not enqueue, admin-only key can manage queues, superadmin bypasses all checks

---

## Epic 16: Release Engineering & SDK Compatibility

> **Reshaped 2026-03-20:** Original 3-story epic reduced to 2. Story 16.3 (stability release branches) dropped as premature — no users to serve backport workflows pre-1.0. Story 16.1 slimmed to docs-only (no GetServerInfo RPC, no --version flags, no SDK changes). Story 16.2 (SDK auth parity) unchanged — closes the critical gap from Epic 15.

Versioning policy and proto backward compatibility formalized. All 5 external SDKs updated with TLS and API key auth support (feature parity with Rust SDK).

### Story 16.1: Versioning & Compatibility Policy

As a developer,
I want documented versioning and proto backward compatibility policies,
So that I understand the stability guarantees when depending on Fila.

**Acceptance Criteria:**

**Given** Fila server and 6 SDKs are independently versioned
**When** the versioning scheme is formalized
**Then** a `docs/compatibility.md` documents:
**And** the semver versioning policy: MAJOR = breaking proto/API changes, MINOR = new features (backward compatible), PATCH = bug fixes
**And** the proto backward compatibility policy: field additions only within a MAJOR version, no field removals or type changes, field numbers never reused
**And** the deprecation policy: minimum 1 MINOR version deprecation window before removal
**And** the document is linked from the main README

### Story 16.2: SDK Auth Feature Parity

As a developer using a non-Rust SDK,
I want TLS and API key authentication support in all 5 external SDKs,
So that I can connect securely to a Fila server that has auth enabled.

**Acceptance Criteria:**

**Given** the Fila server supports mTLS (Epic 15, Story 15.1) and API key auth (Story 15.2)
**When** each external SDK is updated
**Then** fila-go, fila-python, fila-js, fila-ruby, and fila-java each support:
**And** TLS connection options: CA certificate, client certificate, client key (for mTLS)
**And** API key authentication: attaching `authorization: Bearer <key>` metadata to every outgoing RPC
**And** updated proto definitions reflecting the new admin RPCs (CreateApiKey, RevokeApiKey, ListApiKeys, SetAcl, GetAcl)
**And** each SDK's README documents TLS and API key usage
**And** each SDK's integration tests include at least one TLS test and one API key auth test
**And** each SDK's CI pipeline provisions fila-server with auth enabled for integration tests
**And** backward compatible: when no TLS/auth options are set, behavior is identical to before

---

## Epic 16.5: Stability Hardening & Test Coverage

Systematic hardening of the highest-risk subsystems — clustering, TLS, and auth — through blackbox e2e testing, edge case coverage, and CI-integrated code coverage. Ensures the foundation is solid before building further features.

> **Context:** Epic 16 retrospective surfaced "silent TLS downgrade is a universal SDK bug pattern" — all 5 external SDKs had identical bug where partial mTLS config silently fell back to plaintext. Codebase gap analysis revealed: no cluster e2e tests (Epic 14 has only unit tests for Raft), mTLS not tested at e2e level, no code coverage metrics in CI. This is a "sharpen the saw" epic — harden what's built before adding new features.

### Story 16.5.1: Cluster E2E Test Suite

As an operator,
I want blackbox e2e tests proving cluster failover, leader routing, and replication work end-to-end,
So that I can trust multi-node deployments in production.

**Acceptance Criteria:**

**Given** the fila-e2e test suite and a multi-node cluster (3 nodes)
**When** cluster e2e tests execute
**Then** a test verifies: enqueue on node A, consume on node B, ack on node C — full cross-node lifecycle
**And** a test verifies: leader node killed → new leader elected → consumer reconnects → zero message loss
**And** a test verifies: non-leader node receives request → forwards to leader → client gets correct response transparently
**And** a test verifies: node rejoins after crash → catches up from Raft log → becomes eligible for leadership
**And** a test verifies: `fila queue inspect` on any node returns cluster-wide aggregated counts
**And** cluster e2e tests spawn 3 fila-server processes with `cluster.enabled = true` and distinct ports (client + cluster ports)
**And** test helpers manage multi-node lifecycle: start cluster, stop/kill individual nodes, wait for leader election
**And** CI pipeline runs cluster e2e tests (new workflow or extended e2e.yml)
**And** tests have reasonable timeouts accounting for Raft election (10s failover window per NFR23)

### Story 16.5.2: TLS & Auth Edge Case Hardening

As a security-conscious operator,
I want comprehensive TLS and auth edge case tests,
So that security bypass patterns (like silent TLS downgrade) are caught automatically.

**Acceptance Criteria:**

**Given** the fila-e2e test suite
**When** TLS edge case tests execute
**Then** a test verifies: mTLS with client certificate — server validates client cert, connection succeeds
**And** a test verifies: mTLS without client cert — server rejects connection when client auth is required
**And** a test verifies: partial mTLS config (cert+key without CA) — connection fails explicitly, never silently downgrades to plaintext (the "silent TLS downgrade" pattern)
**And** a test verifies: expired certificate — connection rejected with clear error
**And** a test verifies: TLS enabled on server, plaintext client — connection rejected

**Given** auth edge case tests execute
**When** API key and ACL edge cases are tested
**Then** a test verifies: key revocation takes effect immediately — revoked key rejected on next request
**And** a test verifies: permission removal takes effect immediately — previously-authorized operation rejected
**And** a test verifies: bootstrap key has superadmin scope (can perform all operations including data and admin)
**And** a test verifies: superadmin key revocation — revoked superadmin loses all access

**Given** these are universal patterns
**When** test templates are established
**Then** a documented TLS test checklist exists (in docs/ or test comments) that lists the mandatory scenarios any future SDK or TLS change must cover
**And** the checklist includes the "silent downgrade" pattern explicitly as a P0 test case

### Story 16.5.3: CI Code Coverage & Quality Gates

As a developer,
I want code coverage reporting in CI with visibility into under-tested areas,
So that quality gaps are surfaced before they become production incidents.

**Acceptance Criteria:**

**Given** the CI pipeline
**When** coverage reporting is configured
**Then** `cargo-llvm-cov` (or equivalent) runs on every PR and reports line coverage for all crates
**And** coverage results are posted as a PR comment or CI check summary showing per-crate coverage percentages
**And** the auth module (`fila-core/src/auth/`), TLS configuration paths, and cluster module (`fila-core/src/cluster/`) have coverage explicitly reported
**And** a coverage baseline is established and stored (similar to benchmark baselines)
**And** coverage regressions (new code with 0% coverage in security-critical paths) are flagged in CI — not as a hard gate initially, but as a visible warning
**And** the coverage workflow is triggered on the feature branch to verify it works before merge (per CLAUDE.md CI workflow verification rule)

---

## Epic 17: Cluster Hardening

Production-readiness fixes for the Raft clustering layer. All items sourced from GitHub issues #63-#66 discovered during Epic 14 work and Cubic PR reviews.

### Story 17.1: Cluster Error Clarity & Ack/Nack Performance

As an operator,
I want clear error messages that distinguish transient cluster state from real failures, and efficient ack/nack processing,
So that clients can make correct retry decisions and cluster performance scales with queue depth.

**Acceptance Criteria:**

**Given** a node is joining the cluster and hasn't caught up on Raft log entries
**When** a client sends a request for a queue whose Raft group isn't locally available yet
**Then** the server returns gRPC `UNAVAILABLE` (not `NOT_FOUND`) with a `NodeNotReady` error variant
**And** clients/load balancers can distinguish "queue doesn't exist" from "node is still catching up"
**And** `ClusterWriteError` has a new `NodeNotReady` variant mapped to gRPC `UNAVAILABLE`

**Given** a message is acked or nacked in clustered mode
**When** the Raft state machine applies the ack/nack entry
**Then** the storage key is included in `ClusterRequest::Ack`/`ClusterRequest::Nack` (passed through Raft from the leader)
**And** followers perform a direct key lookup instead of scanning all messages in the queue
**And** ack/nack is O(1) regardless of queue depth (previously O(n))

**GitHub issues:** #63, #64

### Story 17.2: Consume Leader Hint & SDK Reconnect

As a consumer,
I want to connect to any cluster node and be transparently routed to the queue's leader,
So that I don't need to know which node leads which queue.

**Acceptance Criteria:**

**Given** a client calls `Consume` on a node that is not the Raft leader for the requested queue
**When** the server detects the non-leader condition
**Then** the server returns an error with the leader's client address (e.g., `NOT_LEADER { leader_addr: "node2:5555" }`)
**And** the Rust SDK (`fila-sdk`) handles this error transparently — reconnects to the hinted leader and retries
**And** all 5 external SDKs (Go, Python, JS, Ruby, Java) handle the leader hint and reconnect transparently
**And** if the hinted leader is unavailable, the SDK falls back to the original error (no infinite redirect loops)
**And** e2e tests verify: consume on non-leader redirects to leader, consumer receives messages after redirect

**GitHub issues:** #65

### Story 17.3: Automatic Queue-to-Node Assignment

As an operator,
I want the cluster to automatically distribute queue leadership across nodes,
So that I don't end up with all queues on one node after scaling or restarts.

**Acceptance Criteria:**

**Given** a new queue is created in a multi-node cluster
**When** the system assigns which nodes participate in the queue's Raft group
**Then** the assignment distributes leadership across available nodes (not all queues on the same leader)
**And** a load-aware or round-robin strategy selects the preferred leader based on current queue distribution
**And** for clusters larger than the replication factor, the system selects which N nodes out of M participate
**And** the admin API exposes the current queue-to-node mapping
**And** e2e tests verify: creating 6 queues on a 3-node cluster results in roughly balanced leadership (no node has more than 3)

**GitHub issue:** #66

---

## Epic 18: Consumer Groups (deferred — design rework needed)

**Status:** Deferred. Original implementation (PR #85, Story 18.1) reverted — shipped wrong semantics. Design rework required before re-implementation.

**Problem:** The original implementation treated consumer groups as labeled subsets that split message throughput — each group was one delivery target competing with other groups via round-robin. The correct behavior (Kafka-style) is that each consumer group gets an independent view of every message with its own delivery state. Within a group, messages are distributed so each is processed by exactly one member.

**Design analysis:** See `_bmad-output/planning-artifacts/research/consumer-group-semantics.md`

**Stories:** TBD — pending resolution of open design questions around per-group scheduling, throttle, and Lua hook semantics.

---

## Epic 19: Developer Experience

Docs website, deployment guides, Helm charts, Lua helpers, and DX sugar. Shipped after all features are in so documentation covers the complete system.

### Story 19.1: Built-in Lua Helpers

As a script author,
I want built-in Lua helper functions for common patterns,
So that I can implement standard scheduling policies without writing boilerplate.

**Acceptance Criteria:**

**Given** the Lua sandbox provides `fila.get()` and standard libraries
**When** built-in helpers are loaded
**Then** the Lua environment includes a `fila.helpers` module available to all scripts
**And** `fila.helpers.exponential_backoff(attempts, base_ms, max_ms)` returns delay in milliseconds with jitter
**And** `fila.helpers.tenant_route(msg, header_name)` extracts a header value as fairness_key with safe defaults for missing headers
**And** `fila.helpers.rate_limit_keys(msg, patterns)` generates throttle key arrays from header patterns
**And** `fila.helpers.max_retries(attempts, max)` returns `{action = "retry"}` or `{action = "dlq"}` based on attempt count
**And** helpers are documented in `docs/lua-patterns.md` (update existing doc with helper API reference)
**And** helpers are unit tested in Rust (via mlua) with edge cases: nil headers, missing keys, zero attempts, overflow values
**And** existing user scripts continue to work unchanged — helpers are additive, not replacing any existing API

### Story 19.2: Documentation Website & Deployment Guides

As an evaluator or operator,
I want a polished documentation website with deployment guides,
So that I can evaluate Fila quickly and deploy it confidently in production.

**Acceptance Criteria:**

*Story ACs to be refined during story creation — high-level scope:*
- Static docs website (e.g., mdBook, Docusaurus, or similar) published from `docs/`
- Production deployment guide: systemd, Docker Compose, Kubernetes
- Helm chart for Kubernetes deployment (single-node and clustered modes)
- Configuration reference with all options, defaults, and examples
- Migration/upgrade guide covering version compatibility
- Getting started guide updated for all deployment methods

### Story 19.3: SDK & Integration Guides

As a developer,
I want comprehensive SDK guides and integration examples,
So that I can integrate Fila into my application quickly in my language of choice.

**Acceptance Criteria:**

*Story ACs to be refined during story creation — high-level scope:*
- Per-SDK quick start guides (Rust, Go, Python, JS, Ruby, Java)
- Common integration patterns: producer/consumer, fan-out, request-reply
- Consumer group usage examples (depends on Epic 18)
- TLS and API key configuration per SDK
- Troubleshooting guide for common issues

---

## Epic 20: Web Management GUI

Operators can monitor queues and visualize real-time scheduling state via a web-based management interface. Read-only dashboard bundled into the server binary. Optional — disabled by default, zero overhead when disabled.

### Story 20.1: Web Management GUI

As an operator,
I want a web-based management interface to monitor queues and scheduling state,
So that I can visualize broker behavior without setting up external monitoring.

**Acceptance Criteria:**

**Given** the Fila server exposes metrics and stats via gRPC
**When** the web GUI is enabled
**Then** the server serves a built-in web interface on a configurable HTTP port (default: 8080, configured via `gui.enabled`, `gui.listen_addr`)
**And** the dashboard shows real-time queue list with depth, throughput, and consumer count per queue
**And** per-queue detail view shows: fairness key distribution (DRR state), throttle key status (bucket fill levels), consumer connections, DLQ depth
**And** a scheduling visualization shows live DRR rounds — which fairness keys are being served, deficit states, skip events
**And** message throughput is graphed over time (last 1h, 6h, 24h)
**And** the GUI is a single-page application bundled into the server binary (no external dependencies to serve)
**And** the GUI communicates with the broker via a lightweight HTTP/JSON API (thin wrapper over existing gRPC stats)
**And** the GUI is read-only — no administrative actions (create/delete queues, config changes) to minimize security surface
**And** the GUI is optional — disabled by default, zero overhead when disabled
**And** in clustered mode, the GUI shows cluster-wide view: node list, partition distribution, replication status
**And** integration tests verify: GUI serves on configured port, dashboard returns queue data matching gRPC GetStats

---

## Epic 21: Trustworthy Benchmark Suite

Developers and evaluators can trust Fila's benchmark numbers for optimization decisions and competitive claims — with statistically valid latency measurement, realistic workload profiles, and reproducible results. This epic addresses 12 methodology gaps identified in the benchmarking methodology research (`_bmad-output/planning-artifacts/research/technical-benchmarking-methodology-research-2026-03-23.md`), transforming Epic 12's benchmark infrastructure from "directionally useful" to "trustworthy for engineering decisions."

> **Context:** The research found that coordinated omission in our closed-loop latency measurement can understate tail latency by orders of magnitude (2,670x demonstrated in ScyllaDB's case). With only 100 latency samples, our p99 is a single data point — noise, not signal. And our 3-second measurement windows are too short to capture RocksDB compaction, memory pressure, or fairness scheduling artifacts. None of the major vendor tools (Kafka, RabbitMQ, OMB) fully solve these problems either — Fila can leapfrog the industry bar with targeted fixes.

### Story 21.1: Statistical Foundation — HdrHistogram & Measurement Rigor

As a developer optimizing Fila's performance,
I want latency benchmarks that use HDR histograms with sufficient sample counts and extended percentile reporting,
So that I can trust p99+ numbers for engineering decisions instead of relying on single-datapoint noise.

**Acceptance Criteria:**

**Given** the `hdrhistogram` crate is added as a dependency to `fila-bench`
**When** any latency benchmark runs
**Then** latency is recorded into an `hdrhistogram::Histogram` (3 significant figures) instead of the current `LatencySampler`
**And** `LatencySampler` is removed from `measurement.rs`

**Given** a latency benchmark (e2e latency light/moderate/saturated, compaction impact)
**When** the benchmark completes
**Then** at least 10,000 latency samples are collected per load level (up from 100)
**And** measurement duration is at least 30 seconds per load level (configurable via `FILA_BENCH_DURATION_SECS` env var, default 30)

**Given** any benchmark that reports latency percentiles
**When** results are emitted to the JSON report
**Then** the report includes p50, p95, p99, p99.9, p99.99, and max values
**And** the `BenchReport` schema is updated to support multiple percentile fields per latency metric

**Given** the CI regression workflow runs 3 times and aggregates
**When** aggregation computes the median
**Then** aggregation uses histogram merging via `Histogram::add()` (not median-of-percentiles)
**And** percentiles are computed from the merged histogram

**Given** the benchmark suite runs end-to-end with the new measurement infrastructure
**When** run 5 times on the same machine
**Then** p99 latency variance is < 10% across runs (NFR-B1 reproducibility)

**Given** the benchmark harness records latency
**When** comparing wall-clock overhead of HdrHistogram recording vs old LatencySampler
**Then** measurement overhead does not exceed 1% of measured values (NFR-B3)

### Story 21.2: Competitive Benchmark Overhaul

As an evaluator comparing Fila against other brokers,
I want competitive benchmarks that use concurrent produce/consume, multiple runs, and comprehensive resource measurement,
So that published comparison numbers reflect realistic behavior, not best-case sequential latency.

**Acceptance Criteria:**

**Given** the competitive latency benchmark for any broker
**When** the benchmark runs
**Then** producers and consumers run concurrently (not sequentially)
**And** the producer sends at a fixed rate, the consumer processes independently
**And** end-to-end latency is measured as `consume_time - produce_timestamp` (timestamp embedded in message payload)
**And** at least 10,000 latency samples are collected per broker using HdrHistogram (from Story 21.1)

**Given** the competitive benchmark orchestration (`Makefile`)
**When** `make bench-competitive` runs
**Then** each broker benchmark runs 3 times
**And** results are aggregated using histogram merging (for latency) and median (for throughput)
**And** the final `bench-{broker}.json` contains the aggregated results

**Given** the competitive resource benchmark
**When** resource usage is measured
**Then** disk I/O (bytes read/written) is captured alongside CPU% and memory MB
**And** disk I/O is obtained via `docker stats --format` block I/O fields or equivalent

**Given** the competitive benchmark results
**When** the JSON report is emitted
**Then** latency results include p50, p95, p99, p99.9, p99.99, and max (matching Story 21.1 format)

**Given** the competitive benchmark measurement
**When** the benchmark runs for any broker
**Then** measurement duration is at least 30 seconds per workload (up from 3 seconds)
**And** a warmup period of at least 5 seconds precedes measurement (data discarded)

### Story 21.3: Open-Loop Load Generation & Latency-Under-Load Benchmarks

As a developer investigating tail latency under realistic load,
I want an open-loop load generator that sends at a fixed rate regardless of response time, with workloads for processing delay, backpressure, and queue depth effects,
So that latency measurements include coordinated-omission-corrected response time, not just service time.

**Acceptance Criteria:**

**Given** the fila-bench harness
**When** a benchmark specifies open-loop mode
**Then** the producer sends requests at a configurable fixed rate using `tokio::time::interval`
**And** each request is spawned as an independent task (fire-and-forget, response collected asynchronously)
**And** latency is measured as `completed_time - scheduled_time` (includes queuing delay)
**And** HdrHistogram records values with `record_correct(value, expected_interval)` for CO correction

**Given** the open-loop generator with N worker tasks
**When** the benchmark completes
**Then** per-worker histograms are merged via `Histogram::add()` into a single result histogram

**Given** a new "latency under load" self-benchmark
**When** the benchmark runs
**Then** it tests 3 load levels: 50%, 80%, and 100% of max throughput (max discovered via a short closed-loop saturation probe)
**And** each load level runs for at least 30 seconds with open-loop generation
**And** results report latency percentiles (p50 through max) at each load level

**Given** a new "consumer processing time" self-benchmark
**When** the benchmark runs
**Then** it tests 4 processing delays: 0ms, 1ms, 10ms, 100ms
**And** each delay level uses open-loop production at a fixed rate with concurrent consumption
**And** consumers simulate processing time with `tokio::time::sleep` before acking
**And** results report throughput and latency at each delay level

**Given** a new "backpressure ramp" self-benchmark
**When** the benchmark runs
**Then** it ramps producer rate from 10% to 150% of max throughput in 10% increments
**And** each step runs for at least 10 seconds with open-loop generation
**And** results include throughput achieved and latency percentiles at each step
**And** the saturation inflection point is identifiable from the results

**Given** a new "queue depth latency" self-benchmark
**When** the benchmark runs
**Then** it pre-loads the queue to depths of 0, 1K, 10K, and 100K messages
**And** at each depth, measures e2e consume latency for newly produced messages (open-loop, 10 seconds)
**And** results report latency percentiles at each queue depth

### Story 21.4: Failure-Path & Fairness Benchmarks

As a developer validating Fila's behavior under adverse conditions,
I want benchmarks for nack storms, DLQ routing overhead, poison pill isolation, and formal fairness measurement,
So that I know the cost of failure paths and can prove Fila's fairness scheduling works correctly under load.

**Acceptance Criteria:**

**Given** a new "nack storm" self-benchmark
**When** the benchmark runs
**Then** it produces messages where 10% are nacked (redelivered) and 90% are acked
**And** it measures overall throughput and latency compared to a 100%-ack baseline
**And** results report the throughput degradation percentage from nack handling

**Given** a new "DLQ routing overhead" self-benchmark
**When** the benchmark runs
**Then** it configures a queue with `max_retries` and a DLQ
**And** it produces messages where a configurable fraction (e.g., 5%) exhaust retries and route to DLQ
**And** results report throughput and latency for the mixed workload vs pure-ack baseline

**Given** a new "poison pill isolation" self-benchmark
**When** the benchmark runs
**Then** it creates a queue with multiple fairness keys
**And** one fairness key's messages are all nacked (poison pills), other keys ack normally
**And** results report per-fairness-key throughput — proving non-poisoned keys maintain throughput
**And** the test explicitly asserts that poisoned-key throughput degrades while other keys are unaffected

**Given** the existing fairness accuracy benchmark
**When** the benchmark runs
**Then** it computes and reports Jain's Fairness Index: `(sum(x_i))^2 / (n * sum(x_i^2))` where `x_i` is per-key consumed count
**And** the result is a single float between 0 and 1 (1.0 = perfect fairness)
**And** weighted fairness tests compute Jain's Index on normalized ratios (actual/expected)

**Given** the existing fairness benchmark with equal weights
**When** N fairness keys have equal weight and the benchmark completes
**Then** Jain's Fairness Index is reported alongside existing per-key deviation metrics
**And** the index value is >= 0.95 for the equal-weight case

### Story 21.5: CI Visualization & Reporting

As a developer reviewing a PR,
I want benchmark results visualized as trend charts on GitHub Pages with regression alerts on PRs,
So that I can see performance trends over time and catch regressions without reading raw JSON.

**Acceptance Criteria:**

**Given** the benchmark suite produces results
**When** results are emitted
**Then** a second output file in `github-action-benchmark` JSON format is generated alongside the existing `BenchReport` JSON
**And** each metric maps to either `customSmallerIsBetter` (latency, overhead) or `customBiggerIsBetter` (throughput) tool type
**And** the format includes `name`, `unit`, `value`, and optional `range` fields

**Given** the `bench-regression.yml` CI workflow
**When** benchmarks complete on a push to `main`
**Then** the `github-action-benchmark` action stores results to the `gh-pages` branch under `dev/bench/`
**And** a Chart.js visualization page is generated at the repository's GitHub Pages URL

**Given** the `bench-regression.yml` CI workflow
**When** benchmarks complete on a PR
**Then** the `github-action-benchmark` action compares against the stored baseline
**And** if any metric regresses beyond the configured threshold (default 115%), a comment is posted on the PR with the regression details
**And** the existing custom comparison (`bench-compare`) continues to run alongside for backward compatibility

**Given** the benchmark results page
**When** a developer visits the GitHub Pages benchmark URL
**Then** they see time-series charts for all key metrics (throughput, latency percentiles, fairness)
**And** each chart shows at least the last 50 data points (commits to main)

**Given** the CI benchmark workflow
**When** all benchmarks (self + competitive) run end-to-end
**Then** total CI wall-clock time is under 15 minutes (NFR-B2)
