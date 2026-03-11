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
---

# Fila - Epic Breakdown (Phase 2+)

## Overview

This document provides the epic and story breakdown for Fila's post-v1 roadmap (Epics 12+), decomposing the Phase 2+ requirements from the updated PRD, Architecture, market research, and brainstorming session into implementable stories. Epics 1-11 (Phase 1 MVP) are complete and documented separately.

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
FR74: Epic 16 — SDK-server compatibility matrix
FR75: Epic 16 — Stability release branches with backported fixes
FR76: Epic 17 — Web-based management GUI
FR77: Epic 17 — Broker-managed consumer groups
FR78: Epic 17 — Built-in Lua helpers for common patterns

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
Teams get stability guarantees and version compatibility contracts. An SDK-server compatibility matrix documents which SDK versions work with which server versions. Operators can deploy stability release branches with backported fixes. Versioning scheme (semver/calver) formalized.
**FRs covered:** FR74, FR75

### Epic 17: Developer Experience
Operators can monitor queues and visualize real-time scheduling state via a web-based management GUI. Consumers can join broker-managed consumer groups with automatic rebalancing. Script authors get built-in Lua helpers for common patterns — exponential backoff, tenant routing — reducing boilerplate.
**FRs covered:** FR76, FR77, FR78

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
**And** the CLI `fila stats` shows cluster-wide summary when connected to any node

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

Teams get stability guarantees and version compatibility contracts. An SDK-server compatibility matrix documents which SDK versions work with which server versions. Operators can deploy stability release branches with backported fixes. Versioning scheme formalized.

### Story 16.1: Versioning Scheme & SDK Compatibility Matrix

As a developer,
I want to consult an SDK-server compatibility matrix with documented guarantees,
So that I know which SDK versions work with which server versions.

**Acceptance Criteria:**

**Given** Fila server and 6 SDKs are independently versioned
**When** the versioning scheme is formalized
**Then** the server adopts semantic versioning (semver): MAJOR.MINOR.PATCH
**And** MAJOR bumps indicate breaking proto/API changes
**And** MINOR bumps indicate new features (backward compatible)
**And** PATCH bumps indicate bug fixes
**And** a `docs/compatibility.md` documents the SDK-server compatibility matrix: minimum server version per SDK version, supported proto versions, deprecation policy
**And** the server exposes a `GetServerInfo` RPC returning: server version, proto version, supported features
**And** SDKs can query `GetServerInfo` at connection time for optional compatibility verification
**And** the compatibility document is published alongside release notes
**And** the proto backward compatibility policy is formalized: field additions only within a MAJOR version, no field removals or type changes

### Story 16.2: Stability Release Branches & Backport Workflow

As an operator,
I want to deploy stability release branches with backported fixes,
So that I can get bug fixes without adopting new features or risking regressions.

**Acceptance Criteria:**

**Given** the server follows semantic versioning (from Story 16.1)
**When** a new MINOR version is released (e.g., 1.2.0)
**Then** a `release/1.x` branch is created from the release tag
**And** critical bug fixes and security patches can be cherry-picked from main into the release branch
**And** a PATCH release (e.g., 1.2.1) is tagged from the release branch
**And** the release CI pipeline builds release binaries and Docker images for PATCH releases
**And** `CHANGELOG.md` documents which fixes are backported to which release branches
**And** a `docs/release-policy.md` documents: release cadence, support window (N-1 minor versions receive patches), backport criteria (security, data loss, critical bugs only)
**And** the bleeding-edge release workflow (existing) continues unchanged for the main branch
**And** at least the 2 most recent minor release branches receive security and critical bug fixes
**And** the workflow is verified by creating a test release branch, cherry-picking a fix, tagging a patch release, and confirming CI builds artifacts

---

## Epic 17: Developer Experience

Operators can monitor queues and visualize real-time scheduling state via a web-based management GUI. Consumers can join broker-managed consumer groups with automatic rebalancing. Script authors get built-in Lua helpers for common patterns — exponential backoff, tenant routing — reducing boilerplate.

### Story 17.1: Built-in Lua Helpers

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

### Story 17.2: Broker-Managed Consumer Groups

As a consumer,
I want to join a consumer group with automatic rebalancing,
So that multiple instances of my service share the workload without manual coordination.

**Acceptance Criteria:**

**Given** consumers can connect via the Consume RPC
**When** a consumer specifies a `consumer_group` parameter in the Consume request
**Then** the broker tracks group membership: which consumers belong to which group for which queue
**And** messages from the queue are distributed across group members — each message goes to exactly one member
**And** the distribution respects fairness scheduling (DRR) — the group as a whole receives fairly-scheduled messages, then the broker round-robins within the group
**And** when a consumer joins or leaves a group, the broker rebalances: redistributes assignment among remaining members
**And** rebalancing is seamless — in-flight messages are governed by visibility timeout, no message loss
**And** a consumer that disconnects is removed from the group after its session timeout (configurable, default 30 seconds)
**And** consumer groups work in both single-node and clustered modes
**And** the admin API includes `GetConsumerGroups` to inspect group membership and assignment
**And** consumers without a `consumer_group` parameter behave as before — independent consumers (backward compatible)
**And** integration tests verify: 3 consumers in a group each receive ~33% of messages, one consumer disconnects and remaining 2 each receive ~50%, new consumer joins and rebalancing redistributes

### Story 17.3: Web Management GUI

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
