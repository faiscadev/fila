---
stepsCompleted:
  - step-01-init
  - step-02-discovery
  - step-03-success
  - step-04-journeys
  - step-05-domain
  - step-06-innovation
  - step-07-project-type
  - step-08-scoping
  - step-09-functional
  - step-10-nonfunctional
  - step-11-polish
  - step-e-01-discovery
  - step-e-02-review
  - step-e-03-edit
inputDocuments:
  - '_bmad-output/planning-artifacts/product-brief-fila-2026-02-10.md'
  - '_bmad-output/brainstorming/brainstorming-session-2026-02-09.md'
  - '_bmad-output/brainstorming/brainstorming-session-2026-03-04.md'
  - '_bmad-output/planning-artifacts/research/market-message-broker-infrastructure-research-2026-03-04.md'
documentCounts:
  briefs: 1
  research: 1
  brainstorming: 2
  projectDocs: 0
classification:
  projectType: api_backend
  domain: general
  complexity: high
  projectContext: greenfield
workflowType: 'prd'
lastEdited: '2026-03-04'
editHistory:
  - date: '2026-03-04'
    changes: 'Strategic update: positioning shift to fairness-first/zero-graduation, roadmap rewrite post-Epic 11, competitive landscape expansion from market research, Lease to Consume rename, future-phase FRs/NFRs'
---

# Product Requirements Document — Fila

**Author:** Lucas
**Date:** 2026-02-10
**Last Updated:** 2026-03-04

## Executive Summary

Fila is the fairness-first open-source message broker. Built in Rust, deployed as a single binary, it makes fair scheduling and per-key throttling first-class broker primitives — capabilities no other open-source broker provides.

**Vision:** Start with Fila day 1, never leave. Scales like Kafka, works like a queue, deploys like a single binary.

Every existing broker — Kafka, RabbitMQ, Pulsar, NATS, SQS — is architecturally blind at dequeue time, delivering messages in FIFO order with no awareness of which tenant is starved or which rate limit has been hit. Teams build the same consume-check-redrive anti-pattern: pull a message, check external state, requeue if conditions aren't met. Fila eliminates this by making scheduling decisions at dequeue time. A Deficit Round Robin scheduler tracks live fairness state across keys; token buckets enforce per-key rate limits; only ready messages are ever delivered to consumers.

**Core differentiators:**
- **Fairness-first** — Built-in DRR fair scheduling across keys. No open-source broker offers this.
- **Zero wasted work** — Consumers only receive messages ready for processing. The consume-check-redrive anti-pattern eliminated at the broker level.
- **Lua rules engine** — `on_enqueue` and `on_failure` hooks for user-defined scheduling policy. Programmable routing at the broker level.
- **Combined primitives** — Fairness + priority + DLQ with redrive + scripting in one system. Each exists somewhere; no single broker combines all four.
- **Single binary, zero dependencies** — No JVM, no Erlang, no ZooKeeper. Rust gives memory safety without GC overhead.

**Positioning:** Fila occupies the "Level 1.5" gap in the message broker market — more capable than Postgres/Redis queues, simpler than Kafka/RabbitMQ. No open-source broker serves this space with fairness as a first-class primitive. SQS Fair Queues (July 2025) validated fairness as a named market category; Fila brings it to the open-source, self-hosted world.

**Target users:** Platform/backend engineers building multi-tenant or rate-sensitive event-driven systems, SREs operating message infrastructure, tech leads evaluating queue architecture, and teams outgrowing Postgres/Redis queues.

**Project status:** Phase 1 (MVP) complete. 11 epics delivered: core broker, fair scheduling, Lua scripting, throttling, runtime config, observability, streaming delivery, Rust SDK, e2e tests, scheduler refactoring, 6 language SDKs, documentation, release pipeline, and SDK publishing. All SDKs published to package registries. Bleeding-edge and tagged releases operational.

## Success Criteria

### User Success

- **Zero wasted work**: Consumers never receive unprocessable messages. No consume-check-redrive loops.
- **Custom fairness layer eliminated**: Replace Redis token buckets, priority demotion middleware, and delay-queue plumbing with Fila's native scheduling.
- **Zero graduation**: Teams start with Fila and never need to migrate to a different broker as they scale. Fila grows from single-node to clustered without changing the API or operational model.
- **Single-system observability**: Answer "why was tenant X delayed?" from Fila's OTel metrics alone.
- **Minutes to first value**: Download to fair-scheduling demo in under 10 minutes via Docker, `cargo install`, or `curl | bash`.

### Technical Success

| KPI | Target | Status |
|-----|--------|--------|
| Throughput | 100k+ msg/s single node | Benchmark pending |
| Scheduling overhead | < 5% vs raw FIFO | Benchmark pending |
| Fairness accuracy | Within 5% of fair share under sustained load | Implemented, benchmark pending |
| Lua hook latency | < 50us p99 | Implemented, benchmark pending |
| Crash recovery | Full state recovery, zero message loss | Implemented |
| Operability | Single engineer deploys, configures, monitors via CLI + Grafana | Implemented |
| Competitive positioning | Queue workload throughput within 2x of Kafka | Benchmark pending |

### Community Signals

1k GitHub stars in 3 months of public launch, 5k in a year. HN frontpage, external contributors, and production deployment reports. "Fairness-first broker" recognized as Fila's category in developer discussions.

## User Journeys

### Journey 1: Maya — "Replacing the Hack" (Primary, Happy Path)

Maya is a senior backend engineer at a mid-size SaaS company. Her team spent last quarter building a fairness and throttling layer on top of RabbitMQ — Redis token buckets, a priority demotion service, custom consumer logic that checks rate limits before processing. When a high-volume tenant spikes, everything slows down for everyone.

**Opening Scene:** Maya is debugging yet another redelivery loop. A config change made the Redis token bucket stale, and tenant messages are cycling through consume-check-redrive at 10x normal rate. She searches "message queue fair scheduling" and finds Fila's README.

**Rising Action:** She runs `docker run fila` locally. Creates a queue with a simple Lua `on_enqueue` that assigns `fairness_key = msg.headers["tenant_id"]`. Enqueues 1,000 messages across 5 tenants — 900 from tenant A, 25 each from B-E. Every tenant gets their fair share.

**Climax:** She sets a throttle limit on tenant A via the API — 50 msg/s. Fila skips tenant A when the bucket is empty, serves B-E immediately. Zero wasted work. She realizes her entire custom stack is obsolete.

**Resolution:** Maya writes two Lua hooks, sets throttle limits via the admin API, and deploys Fila to staging. The three-system Rube Goldberg machine is replaced by a single binary.

### Journey 2: Maya — "Why Isn't Tenant X Getting Messages?" (Primary, Edge Case)

**Opening Scene:** A customer success manager reports tenant X's webhook deliveries stopped 20 minutes ago.

**Rising Action:** Maya opens Grafana. Per-fairness-key metrics show tenant X's throughput dropped to zero. The throttle key `webhook:provider-Y` shows 100% hit rate — someone set the rate limit too low yesterday.

**Climax:** `fila config set webhook:provider-Y rate_limit 500`. Throughput recovers within seconds. Total investigation: 3 minutes.

**Resolution:** She adds an alert on sustained 100% throttle hit rate. One system, one dashboard, one fix.

### Journey 3: Tomas — "The 3 AM Page That Didn't Happen" (Operations)

**Opening Scene:** Tomas is the SRE who used to get paged when Maya's fairness stack broke — three separate systems, three failure modes, three runbooks.

**Rising Action:** After Fila replaces the old stack, Tomas deploys a single binary. OTel metrics wire into Prometheus/Grafana. Per-key throughput, DRR state, throttle hit rates, Lua execution times — all out of the box.

**Climax:** At 3 AM, a high-volume tenant spikes to 10x. DRR naturally serves other tenants their fair share. No page. Tomas sleeps through it.

**Resolution:** One binary, one dashboard, one set of metrics. "What happened last night?" answered in under a minute.

### Journey 4: Priya — "Should We Adopt This?" (Evaluation)

**Opening Scene:** Priya is a staff engineer evaluating infrastructure for a webhook delivery platform needing per-destination rate limiting and fair scheduling. She's been through "build fairness on Kafka" before. She knows Kafka Share Groups (KIP-932) added queue semantics in 4.2, but they don't address fairness. SQS Fair Queues exist but are AWS-only and standard queues only.

**Rising Action:** She reads Fila's README, runs the benchmark suite (competitive throughput for queue workloads), reviews the Lua hook model — maps directly to her use case. The competitive matrix is clear: no other open-source broker combines fairness + priority + DLQ + scripting.

**Climax:** Architecture proposal: Kafka + Redis + custom middleware (3-month build) vs. Fila (single broker, 2-week integration). Fila eliminates an entire layer. The gRPC API means swap-out is possible if needed.

**Resolution:** She recommends Fila, pointing to the unique capability set and the escape hatch: standard gRPC means no lock-in.

### Journey 5: Dev — "Integrating Fila Into My App" (SDK Consumer)

**Opening Scene:** Dev needs to integrate Fila into a Go service. Never used Fila before.

**Rising Action:** `go get github.com/faiscadev/fila-go`. Clean client API: `client.Enqueue()` with headers, `client.Consume()` returns ready messages via stream, `Ack()` on success, `Nack()` on failure. SDKs available in Go, Python, JavaScript, Ruby, Rust, and Java — all published to their respective package registries.

**Climax:** Consumer code is 40 lines instead of 200. No `checkRateLimit()`, no `shouldRetry()`, no `requeue()`. Pull, process, ack.

**Resolution:** Integration takes a day instead of a week. Dev focuses on business logic, not infrastructure plumbing.

### Journey 6: Maya — "Getting the Lua Hooks Right" (Script Author)

**Opening Scene:** Maya writes Lua hooks for fair scheduling across tenants, per-provider throttling, and exponential backoff retry.

**Rising Action:** `on_enqueue` in 10 lines — reads headers, assigns fairness and throttle keys. Tests via `fila`. `on_failure` is trickier — exponential backoff with max 5 retries. Lua execution metrics in Grafana show no errors, 12us execution time.

**Climax:** Debugging: `attempts` was read as string, not number. Quick fix, redeploy, working.

**Resolution:** Two hooks, ~30 lines total, replace a 500-line middleware service and Redis-backed state machine.

### Journey 7: Sam — "Outgrowing Postgres" (Graduation)

**Opening Scene:** Sam's team has been using PostgreSQL with SKIP LOCKED as their job queue. It worked great at first — no new infrastructure, simple queries. But at 50K jobs/minute, autovacuum can't keep up, CPU contention spikes under concurrent consumers, and they need per-tenant fairness that Postgres can't provide.

**Rising Action:** Sam evaluates the options: RabbitMQ (complex routing but no fairness, Erlang dependency), Kafka (overkill for queue workloads, massive operational burden), NATS (simple but no fairness or priority), SQS (AWS lock-in, no priorities). Fila appears in a search for "fair queue broker" — single binary like their Postgres setup, but purpose-built for queues.

**Climax:** `pip install fila-client`. Sam's producer is 10 lines. The Lua `on_enqueue` hook assigns `fairness_key` from the job's `tenant_id` header. Immediate improvement: per-tenant fair scheduling, DLQ for failed jobs, visibility timeouts — all features they'd been building half-solutions for in application code.

**Resolution:** Sam's team migrates in a week. No new operational complexity — one binary, one set of metrics. They got the queue features they needed without the operational burden of RabbitMQ or Kafka.

### Journey Requirements Summary

| Journey | Key Capabilities Revealed |
|---------|--------------------------|
| Maya — Happy Path | Queue creation, Lua hooks, fairness keys, throttle API, CLI stats, Docker + install paths |
| Maya — Debugging | Per-key OTel metrics, throttle hit rates, runtime config via CLI |
| Tomas — Operations | Single-binary deployment, OTel/Prometheus, per-key dashboarding, crash recovery |
| Priya — Evaluation | Benchmark suite, documentation, competitive positioning, gRPC escape hatch |
| Dev — SDK Consumer | Language SDKs (Go, Python, JS, Ruby, Rust, Java), Enqueue/Consume/Ack API |
| Maya — Lua Scripting | Script testing via CLI, Lua metrics, error visibility, debuggability |
| Sam — Graduation | Postgres-to-Fila migration, fairness keys, DLQ, visibility timeouts, operational simplicity |

## Innovation & Competitive Landscape

### Market Context

The message broker market is a $1.5-3B market growing at 10-12% CAGR, dominated by Kafka (38.7% adoption) and RabbitMQ (28.6%). Including event streaming platforms, the TAM exceeds $5B. The market is consolidating: IBM acquiring Confluent for $11B, WarpStream absorbed by Confluent, Broadcom owns RabbitMQ via VMware/Tanzu.

Teams follow a graduation ladder — Postgres, Redis, RabbitMQ, Kafka — wasting months migrating at each step. The strongest 2025 counter-narrative is "just use Postgres" (SKIP LOCKED, PGMQ, Solid Queue), which handles ~70K msg/sec but lacks fairness, priority, DLQ, visibility timeouts, and fan-out.

### Novel Architecture

1. **Dequeue-time scheduling as a broker primitive** — Every existing broker delivers in FIFO order. Fila makes scheduling decisions at dequeue time based on live DRR fairness state and per-key token bucket state.

2. **Zero wasted work model** — The consume-check-redrive anti-pattern eliminated at the broker level. Consumers only receive ready messages.

3. **Lua rules engine for scheduling policy** — Two hooks (`on_enqueue`, `on_failure`) separate policy from enforcement. Lua = logic, API = data, Rust = enforcement.

4. **Redis-inspired single-threaded scheduler** — Single-threaded core with lock-free IO channels eliminates lock contention on the scheduling hot path.

### Competitive Matrix

| Solution | Fairness | Priority | DLQ + Redrive | Scripting | Deployment | License |
|----------|----------|----------|---------------|-----------|------------|---------|
| Kafka 4.x | None | None | Manual | None | Multi-node cluster | Apache 2.0 |
| RabbitMQ 4.x | None | Two-tier (4.0+) | Basic | None | Multi-node cluster | MPL 2.0 |
| SQS | Fair Queues (2025, std only) | None | Basic | None | Managed only | Proprietary |
| NATS/JetStream | None | None | Manual | None | Single binary | Apache 2.0 |
| Redpanda | None | None | Manual | None | Single binary | BSL |
| Inngest | Per-key concurrency | None | Built-in | Workflow SDK | Managed/self-hosted | AGPL + BSL |
| BullMQ | None | Yes | Yes | None | Redis library | MIT |
| Postgres (PGMQ) | None | None | None | SQL | Existing infra | PostgreSQL |
| **Fila** | **DRR (core)** | **Yes** | **Built-in** | **Lua** | **Single binary** | **AGPL-3.0** |

Fila occupies a genuinely empty cell: open-source, standalone broker, with fairness, priority, DLQ with redrive, and programmable Lua scripting as native primitives.

### Key Competitive Dynamics

**Kafka Share Groups (KIP-932):** Queue-like semantics natively in Kafka, production-ready in Kafka 4.2 (Feb 2026). This normalizes queue semantics within the Kafka ecosystem but does not address fairness, priority, or programmable routing. Teams already running Kafka may use Share Groups for simple queue workloads rather than adopting a separate broker. Fila's response: position on fairness, not on "queue vs stream."

**SQS Fair Queues (July 2025):** First major platform to address multi-tenant fairness natively. Validates fairness as a named market need. Limitations: AWS-only, standard queues only (not FIFO), no priorities, no scripting. Fila brings fairness to the open-source, self-hosted world with richer primitives.

**"Just use Postgres" movement:** Strongest competitive pressure at the low end. PGMQ, Solid Queue, SKIP LOCKED handle ~70K msg/sec. But: no fairness, no priority, no DLQ, no visibility timeouts, table bloat under load, 200-300ms latency ceiling, and CPU contention at scale. Fila targets the team that's outgrowing Postgres and needs a real broker without Kafka's complexity.

**License trust erosion:** Redis relicensed (2024), NATS/Synadia attempted BSL (2025), Confluent going to IBM, Broadcom owns RabbitMQ. Teams wanting truly open-source infrastructure with no governance risk have fewer options. Fila's AGPL-3.0 guarantees the code stays open.

**Memphis cautionary tale:** "Simpler than Kafka" alone is not a durable market position. Memphis had GUI, schema management, DLQ — but nothing that couldn't be replicated as a Kafka plugin. It died and pivoted to Superstream (Kafka cost optimization). Fila's fairness queuing and Lua scripting are genuine differentiators Memphis lacked.

**Inngest:** Most direct conceptual overlap. $34M funding, first-class fair queuing via `concurrency.key`. But Inngest is a workflow platform — code runs inside Inngest functions. Fila is protocol-level (gRPC), decoupling producers from consumers without dictating execution model. Inngest validates the problem; Fila serves a different architecture.

### The "Level 1.5" Gap

The market has clear tiers: Postgres/Redis at the bottom, Kafka/RabbitMQ at the top. The space between — more than a database queue, simpler than a full broker cluster — has no clear owner. Fila's single binary with fairness, priority, DLQ, and scripting fits precisely in this gap. The natural Beanstalkd successor for the modern era.

### Adoption Dynamics

- Operational complexity is the #1 developer concern — teams actively reject brokers requiring dedicated infra teams
- "Time to first working example" is the primary adoption gate — under 5 minutes is the target
- Bottom-up adoption dominates: developers choose, then champion internally
- HN is the #1 seeding channel for developer infrastructure tools
- Published benchmarks are consumed during the "Google Phase" of evaluation — teams rarely run their own

## API Specification

### Hot Path RPCs

| RPC | Direction | Description |
|-----|-----------|-------------|
| `Enqueue` | Producer → Broker | Submit message with headers and payload. Triggers `on_enqueue` Lua hook. |
| `Consume` | Consumer ← Broker | Server-streaming. Broker pushes ready messages based on DRR + throttle decisions. |
| `Ack` | Consumer → Broker | Confirm successful processing. Removes message. |
| `Nack` | Consumer → Broker | Report failure. Triggers `on_failure` Lua hook. |

### Admin RPCs

| RPC | Direction | Description |
|-----|-----------|-------------|
| `CreateQueue` | Admin → Broker | Create queue with Lua scripts and configuration. |
| `DeleteQueue` | Admin → Broker | Remove queue and messages. |
| `SetConfig` | Admin → Broker | Set runtime key-value config. Readable by Lua via `fila.get()`. |
| `GetStats` | Admin → Broker | Queue stats, per-key throughput, DRR state, throttle state. |
| `Redrive` | Admin → Broker | Move messages from DLQ back to source queue. |

### Message Envelope (Protobuf)

- `id` — Broker-generated unique identifier
- `headers` — User-provided key-value map (used by Lua hooks for key assignment)
- `payload` — Opaque bytes (broker never inspects content)
- `timestamps` — Enqueue, consume, ack/nack times
- `metadata` — Broker-assigned: fairness_key, weight, throttle_keys, attempt count

### Error Handling

| Condition | Behavior |
|-----------|----------|
| Lua script error/timeout | Circuit breaker: default fairness_key, weight=1, no throttle keys. Log + counter. |
| `on_failure` script error | Default to retry with backoff. Log error. |
| No ready messages | Stream held open; messages pushed when ready. |
| Unknown message Ack/Nack | Error response. Idempotent. |
| Visibility timeout expired | Message re-enters ready pool. At-least-once semantics. |
| Stream disconnection | Consumed messages governed by visibility timeout. Reconnect opens new stream. |

### Authentication

**Phase 1 (MVP)**: None. Trust the network. **Phase 4**: mTLS + API keys.

### SDKs

**Published (6 languages):** Go, Python, JavaScript, Ruby, Rust, Java — all available on their respective package registries (crates.io, PyPI, npm, RubyGems, Maven Central, Go modules).

Thin gRPC wrappers generated from Protobuf definitions with ergonomic language-specific APIs. Core operations: `enqueue()`, `consume()` (streaming), `ack()`, `nack()`.

### Implementation Architecture

- **Protocol**: gRPC only (tonic). Single protocol for hot path and admin.
- **Delivery**: Server-streaming gRPC for Consume. Ack/Nack as separate unary RPCs.
- **Versioning**: Protobuf backward compatibility. Field addition only.
- **Concurrency**: gRPC IO threads (tonic/tokio) → lock-free channels → single-threaded scheduler core → lock-free channels → IO threads. Streaming connections managed by IO threads.

## Product Scope & Roadmap

### Phase 1 — MVP (Complete)

11 epics delivered. Full broker with fair scheduling, Lua scripting, throttling, observability, streaming delivery, CLI, 6 language SDKs published to registries, documentation, and release pipeline.

| Epic | Deliverable | Status |
|------|-------------|--------|
| 1-2 | Protobuf envelope, RocksDB storage, gRPC server, DRR scheduler, fairness keys, visibility timeout | Done |
| 3 | Lua integration, `on_enqueue`/`on_failure` hooks, execution limits, DLQ | Done |
| 4 | Token buckets, throttle keys from Lua, skip throttled keys in DRR | Done |
| 5 | Runtime key-value store, `fila.get()` Lua bridge, admin RPCs, CLI | Done |
| 6 | OTel observability: 14 metric instruments, tracing spans, W3C trace context | Done |
| 7 | Rust SDK (`fila-sdk` crate), per-operation error types | Done |
| 8 | E2E test suite (11 blackbox tests), scheduler decomposition, trace context wiring | Done |
| 9 | Go, Python, JavaScript, Ruby, Java SDKs — separate repos with CI | Done |
| 10 | Release pipeline, SDK publishing, binary distribution, documentation, tutorials | Done |
| 11 | Release pipeline verification, all SDKs published to registries | Done |

278 tests, zero regressions.

### Phase 2 — Benchmarks & Competitive Positioning

Establish Fila's performance baseline and competitive standing. Data-driven foundation for all subsequent optimization work.

| Deliverable | Description |
|-------------|-------------|
| Continuous benchmark suite | Automated benchmarks on every PR — throughput, latency percentiles, queue depth scaling, fairness key cardinality, consumer concurrency |
| Self-benchmarking | Single-node throughput ceiling, latency p50/p95/p99 under load, RocksDB compaction impact, memory footprint profile |
| Competitive benchmarks | Head-to-head vs Kafka, RabbitMQ, NATS for queue workloads. Reproducible, published methodology |
| Published results | Benchmark results as content — blog post, README badge, HN launch material |

### Phase 3 — Storage Abstraction + Clustering (Coupled Workstream)

> **Updated 2026-03-10:** Technical research (`_bmad/docs/research/decoupled-scheduler-sharded-storage.md`) validated a CockroachDB-style Raft-per-queue model. RocksDB is sufficient as a Raft state machine backend — a purpose-built storage engine is a future optimization, not a prerequisite. Sharding is by queue (not Kafka-style partitions), preserving perfect per-queue fairness with zero algorithmic changes.

| Deliverable | Description |
|-------------|-------------|
| Storage abstraction | Clean storage trait using Fila-domain terms. Enables future engine swaps and provides interface for Raft state machine application |
| Phase 2 viability seams | Routing indirection, DRR key-set scoping, scheduling stats emission — thin seams that keep hierarchical scaling viable without premature engineering |
| Embedded consensus | Raft-based clustering. Zero external dependencies — single binary stays single binary. Each queue is a Raft group |
| Automatic queue distribution | Users create queues, Fila distributes them across cluster nodes via Raft groups. Queue semantics never leak — no offsets, no partitions exposed |
| Transparent routing | Any node accepts any request; routes to queue's Raft leader. Consumers connect to any node |
| Purpose-built storage engine | **Deferred** — future optimization after clustering is proven. RocksDB serves as local state machine backend under Raft |

### Phase 4 — Authentication & Security

Unblock real production deployments. Current state: trust the network.

| Deliverable | Description |
|-------------|-------------|
| mTLS | Mutual TLS for transport security |
| API keys | Per-client authentication with key management |
| Authorization | Per-queue access control |

### Phase 5 — Release Engineering & Distribution

Continuous delivery from main. Every merge is a release.

| Deliverable | Description |
|-------------|-------------|
| Versioning scheme | Semver or calver — TBD based on community expectations |
| SDK compatibility contract | Server-SDK version matrix, backward compatibility guarantees |
| Distribution channels | Homebrew, apt, Docker, `cargo install`, `curl \| bash` (already operational) |
| Stability releases | Backported fixes for teams wanting stable branches |

### Phase 6 — Developer Experience

| Deliverable | Description |
|-------------|-------------|
| Management GUI | Web interface for monitoring and real-time scheduling visualization |
| Zero-ops self-healing | Auto-compaction, storage management, health self-checks |
| Consumer groups | Broker-managed consumer coordination |
| Declarative config sugar | Built-in Lua helpers, common patterns as config |

### Risk Analysis

**Technical:**
- **Storage engine complexity** — Purpose-built storage is a multi-month effort. Mitigated by clean abstraction trait; RocksDB remains functional during development.
- **Clustering correctness** — Distributed consensus is notoriously hard. Mitigated by using embedded Raft (proven approach: etcd, CockroachDB) and extensive testing.
- **Single-threaded scheduler throughput ceiling** — Redis precedent validates model. Benchmarks (Phase 2) will reveal actual ceiling and inform whether sharding is needed.
- **Lua on the hot path** — Pre-compiled scripts, timeouts, memory limits, circuit breaker already implemented. Benchmarks will quantify real overhead.

**Market:**
- **Kafka Share Groups (KIP-932)** — Queue semantics natively in Kafka. Mitigated by positioning on fairness, not on "queue vs stream." Share Groups don't address fairness, priority, or scripting.
- **"Just use Postgres" momentum** — Strongest competitive pressure at the low end. Mitigated by clearly articulating when Postgres isn't enough (fairness, priority, DLQ, scale).
- **AGPL-3.0 perception** — May deter some enterprise adopters. Mitigated by precedent (MongoDB, Grafana) and potential dual licensing if enterprise demand materializes.
- **Single-maintainer governance risk** — Common for new OSS projects. Mitigated by transparent governance and community building.

**Timing:**
- **Favorable:** License trust erosion across ecosystem creates appetite for independent alternatives. SQS Fair Queues validated fairness as a category. SME segment growing 22% annually.
- **Unfavorable:** Market consolidating around large players (IBM, AWS, Broadcom). Kafka 4.0 simplified operations. High skepticism toward new broker entrants.

REST API dropped — reintroduce only if real demand surfaces.

## Functional Requirements

### Delivered (Phase 1)

#### Message Lifecycle

- **FR1**: Producers can enqueue messages with arbitrary headers and opaque byte payloads
- **FR2**: Consumers can receive ready messages via a persistent server-streaming connection
- **FR3**: Consumers can acknowledge successful message processing
- **FR4**: Consumers can negatively acknowledge failed processing, triggering failure handling logic
- **FR5**: The broker automatically re-makes available messages whose visibility timeout has expired
- **FR6**: The broker routes failed messages to a dead-letter queue based on failure handling rules
- **FR7**: Operators can redrive messages from a dead-letter queue back to the source queue

#### Fair Scheduling

- **FR8**: The broker assigns messages to fairness groups based on message properties
- **FR9**: The broker schedules delivery using Deficit Round Robin across fairness groups
- **FR10**: Higher-weighted fairness groups receive proportionally more throughput
- **FR11**: Only active fairness groups (those with pending messages) participate in scheduling
- **FR12**: Each fairness group's actual throughput stays within 5% of its fair share under sustained load

#### Throttling

- **FR13**: The broker enforces per-key rate limits using token bucket rate limiting
- **FR14**: Messages can be subject to multiple independent throttle keys simultaneously
- **FR15**: The broker skips throttled keys during scheduling and serves the next ready key
- **FR16**: Operators can set and update throttle rates at runtime without restart
- **FR17**: Hierarchical throttling via multiple throttle key assignment

#### Rules Engine

- **FR18**: Operators can attach Lua scripts to queues that execute on message enqueue
- **FR19**: `on_enqueue` computes and assigns fairness key, weight, and throttle keys from message headers
- **FR20**: Operators can attach Lua scripts that execute on message failure
- **FR21**: `on_failure` decides between retry (with delay) and dead-letter based on properties and attempt count
- **FR22**: Lua scripts can read runtime configuration values via `fila.get()`
- **FR23**: The broker enforces execution timeouts and memory limits on Lua scripts
- **FR24**: Circuit breaker falls back to safe defaults on Lua error or timeout
- **FR25**: Lua scripts are pre-compiled and cached for repeated execution

#### Runtime Configuration

- **FR26**: External systems can set key-value config entries via the API
- **FR27**: Lua scripts read config at execution time via `fila.get()`
- **FR28**: Config changes take effect on subsequent enqueues without restart
- **FR29**: Operators can view current configuration state

#### Queue Management

- **FR30**: Operators can create named queues with Lua scripts and configuration
- **FR31**: Operators can delete queues and their messages
- **FR32**: The broker auto-creates a dead-letter queue for each queue
- **FR33**: Operators can view queue stats: depth, per-key throughput, scheduling state
- **FR34**: All queue management available through `fila` CLI

#### Observability

- **FR35**: The broker exports OpenTelemetry-compatible metrics for all operations
- **FR36**: Per-fairness-key throughput metrics (actual vs fair share)
- **FR37**: Per-throttle-key hit/pass rate metrics and token bucket state
- **FR38**: DRR scheduling round metrics
- **FR39**: Lua execution time histograms and error counters
- **FR40**: Tracing spans for enqueue, consume, ack, and nack
- **FR41**: Operators can diagnose "why was key X delayed?" from broker metrics alone

#### Client SDKs

- **FR42**: Go client SDK — published as Go module
- **FR43**: Python client SDK — published on PyPI
- **FR44**: JavaScript/Node.js client SDK — published on npm
- **FR45**: Ruby client SDK — published on RubyGems
- **FR46**: Rust client SDK — published on crates.io
- **FR47**: Java client SDK — published on Maven Central
- **FR48**: All SDKs support enqueue, streaming consume, ack, and nack

#### Deployment & Distribution

- **FR49**: Single binary process
- **FR50**: Docker image (ghcr.io)
- **FR51**: `cargo install` installation
- **FR52**: Shell script installation (`curl | bash`)
- **FR53**: Full crash recovery with zero message loss
- **FR54**: Bleeding-edge releases on every push to main
- **FR55**: Tagged releases on version tags

#### Documentation & LLM Readiness

- **FR56**: Documentation covers all queue operations, Lua hooks, configuration, and SDK usage with at least one working example per concept
- **FR57**: Working code examples for every SDK in every supported language
- **FR58**: Guided tutorials for common use cases (multi-tenant fairness, per-provider throttling, retry policies)
- **FR59**: API reference generated from Protobuf definitions
- **FR60**: Structured `llms.txt` for LLM agent consumption
- **FR61**: Copy-paste Lua hook examples for common patterns
- **FR62**: New users can complete the getting-started tutorial in under 10 minutes using only the documentation

### Planned (Phase 2+)

#### Benchmarking

- **FR63**: Developers can run a continuous benchmark suite on every PR that detects performance regressions
- **FR64**: Evaluators can compare published benchmark results of Fila against Kafka, RabbitMQ, and NATS for queue workloads
- **FR65**: Operators can view a benchmark dashboard tracking throughput, latency percentiles, and resource usage over time

#### Clustering & Storage

- **FR66**: ~~The broker can persist messages using a purpose-built storage engine optimized for queue access patterns, replacing RocksDB~~ **Deferred** — RocksDB is sufficient as Raft state machine backend. Future optimization, not a clustering prerequisite.
- **FR67**: Operators can deploy multi-node clusters with embedded Raft consensus (Raft-per-queue model)
- **FR68**: Operators can create queues without managing partitions — Fila distributes them across cluster nodes automatically
- **FR69**: Consumers can connect to any node and be transparently routed to the queue's Raft leader
- **FR70**: Operators can view cluster-wide queue stats aggregated from all nodes

#### Authentication & Authorization

- **FR71**: Operators can enable mTLS for transport-level security between clients and broker
- **FR72**: Operators can authenticate clients via API keys
- **FR73**: Operators can define per-queue access control policies

#### Release Engineering

- **FR74**: Developers can consult an SDK-server compatibility matrix with documented guarantees
- **FR75**: Operators can deploy stability release branches with backported fixes

#### Developer Experience

- **FR76**: Operators can monitor queues and visualize scheduling state via a web-based management GUI
- **FR77**: Consumers can join broker-managed consumer groups with automatic rebalancing
- **FR78**: Script authors can use built-in Lua helpers for common patterns (exponential backoff, tenant routing)

## Non-Functional Requirements

### Delivered (Phase 1)

#### Performance

- **NFR1**: 100,000+ msg/s single-node throughput on commodity hardware
- **NFR2**: < 5% throughput cost for fair scheduling vs raw FIFO
- **NFR3**: Fairness accuracy within 5% of fair share under sustained load
- **NFR4**: Lua `on_enqueue` < 50us p99
- **NFR5**: Lua `on_failure` < 50us p99
- **NFR6**: Enqueue-to-consume latency < 1ms p50 when consumer is waiting
- **NFR7**: Token bucket decisions < 1us overhead per scheduling loop iteration

#### Reliability

- **NFR8**: Full state recovery after crash, zero message loss
- **NFR9**: Atomic enqueue persistence (RocksDB WriteBatch)
- **NFR10**: Expired visibility timeouts resolved within one scheduling cycle
- **NFR11**: Lua circuit breaker activates within 3 consecutive failures
- **NFR12**: At-least-once delivery under all failure conditions

#### Operability

- **NFR13**: Single engineer can deploy, configure, and monitor without tribal knowledge
- **NFR14**: Download to fair-scheduling demo in under 10 minutes
- **NFR15**: Single binary, no external runtime dependencies
- **NFR16**: All state queryable via `fila` CLI or OTel metrics
- **NFR17**: Runtime config changes without restart or downtime

#### Integration

- **NFR18**: OTel-compatible metrics exportable to Prometheus, Grafana, Datadog
- **NFR19**: gRPC best practices (status codes, metadata propagation, deadlines)
- **NFR20**: Protobuf backward compatibility across minor versions
- **NFR21**: Idiomatic SDK conventions per target language

### Planned (Phase 2+)

#### Clustering

- **NFR22**: Linear throughput scaling: 2 nodes >= 1.8x single-node throughput
- **NFR23**: Automatic failover within 10 seconds of node failure detection
- **NFR24**: Zero message loss during planned node additions and removals
- **NFR25**: Consumer reconnection to healthy nodes within 5 seconds of partition
- **NFR26**: Cluster state convergence within 30 seconds of membership change

#### Authentication & Security

- **NFR27**: mTLS handshake < 5ms overhead per connection
- **NFR28**: API key validation < 100us per request
- **NFR29**: Secure defaults — authentication required unless explicitly disabled

#### Storage Engine (Deferred)

> **Deferred 2026-03-10:** These NFRs apply to a purpose-built storage engine (FR66), which is deferred until after clustering is proven. RocksDB remains the storage engine for now.

- **NFR30**: Queue-optimized write throughput >= 2x RocksDB for append-heavy workloads
- **NFR31**: Predictable latency — no compaction-induced latency spikes > 10ms p99
- **NFR32**: Efficient TTL expiry without full-table scans
- **NFR33**: Storage footprint < 1.5x raw message data (overhead from indexing and metadata)
