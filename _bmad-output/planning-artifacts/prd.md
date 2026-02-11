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
inputDocuments:
  - '_bmad-output/planning-artifacts/product-brief-fila-2026-02-10.md'
  - '_bmad-output/brainstorming/brainstorming-session-2026-02-09.md'
documentCounts:
  briefs: 1
  research: 0
  brainstorming: 1
  projectDocs: 0
classification:
  projectType: api_backend
  domain: general
  complexity: high
  projectContext: greenfield
workflowType: 'prd'
---

# Product Requirements Document — Fila

**Author:** Lucas
**Date:** 2026-02-10

## Executive Summary

Fila is an open-source, Rust-based message broker where fair scheduling and per-key throttling are first-class primitives. Every existing broker (Kafka, RabbitMQ, Pulsar, NATS, SQS) is architecturally blind at dequeue time — delivering messages in FIFO order with no awareness of which tenant is starved or which rate limit has been hit. Teams are forced to build the same consume-check-redrive anti-pattern: pull a message, check external state, requeue if conditions aren't met.

Fila eliminates this by making scheduling decisions at dequeue time. A Deficit Round Robin scheduler tracks live fairness state across keys; token buckets enforce per-key rate limits; and only ready messages are ever delivered to consumers. Zero wasted work.

**Core differentiators:**
- Fairness and throttling as broker primitives — not consumer-side afterthoughts
- Lua rules engine (`on_enqueue`, `on_failure`) for user-defined scheduling policy
- Redis-inspired single-threaded scheduler core with lock-free IO channels
- Zero wasted work — consumers only receive messages ready for processing

**Target users:** Platform/backend engineers building multi-tenant or rate-sensitive event-driven systems, SREs operating message infrastructure, and tech leads evaluating queue architecture.

**Project context:** Open-source greenfield project. Solo developer. Rust. Success measured by technical merit and community impact.

## Success Criteria

### User Success

- **Zero wasted work**: Consumers never receive unprocessable messages. No consume-check-redrive loops.
- **Custom fairness layer eliminated**: Replace Redis token buckets, priority demotion middleware, and delay-queue plumbing with fila's native scheduling.
- **Single-system observability**: Answer "why was tenant X delayed?" from fila's OTel metrics alone.
- **Minutes to first value**: Download to fair-scheduling demo in under 10 minutes via Docker, `cargo install`, or `curl | bash`.

### Technical Success

| KPI | Target |
|-----|--------|
| Throughput | 100k+ msg/s single node |
| Scheduling overhead | < 5% vs raw FIFO |
| Fairness accuracy | Within 5% of fair share under sustained load |
| Lua hook latency | < 50μs p99 |
| Crash recovery | Full state recovery, zero message loss |
| Operability | Single engineer deploys, configures, monitors via CLI + Grafana |

### Community Signals (Aspirational)

1k GitHub stars in 3 months, 5k in a year. HN frontpage, external contributors, and production deployment reports as strong positive signals if they happen.

## User Journeys

### Journey 1: Maya — "Replacing the Hack" (Primary, Happy Path)

Maya is a senior backend engineer at a mid-size SaaS company. Her team spent last quarter building a fairness and throttling layer on top of RabbitMQ — Redis token buckets, a priority demotion service, custom consumer logic that checks rate limits before processing. When a high-volume tenant spikes, everything slows down for everyone.

**Opening Scene:** Maya is debugging yet another redelivery loop. A config change made the Redis token bucket stale, and tenant messages are cycling through consume-check-redrive at 10x normal rate. She searches "message queue fair scheduling" and finds fila's README.

**Rising Action:** She runs `docker run fila` locally. Creates a queue with a simple Lua `on_enqueue` that assigns `fairness_key = msg.headers["tenant_id"]`. Enqueues 1,000 messages across 5 tenants — 900 from tenant A, 25 each from B–E. Every tenant gets their fair share.

**Climax:** She sets a throttle limit on tenant A via the API — 50 msg/s. Fila skips tenant A when the bucket is empty, serves B–E immediately. Zero wasted work. She realizes her entire custom stack is obsolete.

**Resolution:** Maya writes two Lua hooks, sets throttle limits via the admin API, and deploys fila to staging. The three-system Rube Goldberg machine is replaced by a single binary.

### Journey 2: Maya — "Why Isn't Tenant X Getting Messages?" (Primary, Edge Case)

**Opening Scene:** A customer success manager reports tenant X's webhook deliveries stopped 20 minutes ago.

**Rising Action:** Maya opens Grafana. Per-fairness-key metrics show tenant X's throughput dropped to zero. The throttle key `webhook:provider-Y` shows 100% hit rate — someone set the rate limit too low yesterday.

**Climax:** `fila-cli config set webhook:provider-Y rate_limit 500`. Throughput recovers within seconds. Total investigation: 3 minutes.

**Resolution:** She adds an alert on sustained 100% throttle hit rate. One system, one dashboard, one fix.

### Journey 3: Tomas — "The 3 AM Page That Didn't Happen" (Operations)

**Opening Scene:** Tomas is the SRE who used to get paged when Maya's fairness stack broke — three separate systems, three failure modes, three runbooks.

**Rising Action:** After fila replaces the old stack, Tomas deploys a single binary. OTel metrics wire into Prometheus/Grafana. Per-key throughput, DRR state, throttle hit rates, Lua execution times — all out of the box.

**Climax:** At 3 AM, a high-volume tenant spikes to 10x. DRR naturally serves other tenants their fair share. No page. Tomas sleeps through it.

**Resolution:** One binary, one dashboard, one set of metrics. "What happened last night?" answered in under a minute.

### Journey 4: Priya — "Should We Adopt This?" (Evaluation)

**Opening Scene:** Priya is a staff engineer evaluating infrastructure for a webhook delivery platform needing per-destination rate limiting and fair scheduling. She's been through "build fairness on Kafka" before.

**Rising Action:** She reads fila's README, runs the benchmark suite (Kafka-competitive throughput), reviews the Lua hook model — maps directly to her use case.

**Climax:** Architecture proposal: Kafka + Redis + custom middleware (3-month build) vs. fila (single broker, 2-week integration). Fila eliminates an entire layer.

**Resolution:** She recommends fila, pointing to the competitive landscape and the escape hatch: standard gRPC means swap-out is possible.

### Journey 5: Dev — "Integrating Fila Into My App" (SDK Consumer)

**Opening Scene:** Dev needs to integrate fila into a Go service. Never used fila before.

**Rising Action:** `go get github.com/fila/fila-go`. Clean client API: `client.Enqueue()` with headers, `client.Lease()` returns ready messages via stream, `Ack()` on success, `Nack()` on failure.

**Climax:** Consumer code is 40 lines instead of 200. No `checkRateLimit()`, no `shouldRetry()`, no `requeue()`. Pull, process, ack.

**Resolution:** Integration takes a day instead of a week. Dev focuses on business logic, not infrastructure plumbing.

### Journey 6: Maya — "Getting the Lua Hooks Right" (Script Author)

**Opening Scene:** Maya writes Lua hooks for fair scheduling across tenants, per-provider throttling, and exponential backoff retry.

**Rising Action:** `on_enqueue` in 10 lines — reads headers, assigns fairness and throttle keys. Tests via `fila-cli`. `on_failure` is trickier — exponential backoff with max 5 retries. Lua execution metrics in Grafana show no errors, 12μs execution time.

**Climax:** Debugging: `attempts` was read as string, not number. Quick fix, redeploy, working.

**Resolution:** Two hooks, ~30 lines total, replace a 500-line middleware service and Redis-backed state machine.

### Journey Requirements Summary

| Journey | Key Capabilities Revealed |
|---------|--------------------------|
| Maya — Happy Path | Queue creation, Lua hooks, fairness keys, throttle API, CLI stats, Docker + install paths |
| Maya — Debugging | Per-key OTel metrics, throttle hit rates, runtime config via CLI |
| Tomas — Operations | Single-binary deployment, OTel/Prometheus, per-key dashboarding, crash recovery |
| Priya — Evaluation | Benchmark suite, documentation, competitive positioning, gRPC escape hatch |
| Dev — SDK Consumer | Language SDKs (Go, Python, JS, Ruby, Rust, Java), Enqueue/Lease/Ack API |
| Maya — Lua Scripting | Script testing via CLI, Lua metrics, error visibility, debuggability |

## Innovation & Competitive Landscape

### Novel Architecture

1. **Dequeue-time scheduling as a broker primitive** — Every existing broker delivers in FIFO order. Fila makes scheduling decisions at dequeue time based on live DRR fairness state and per-key token bucket state.

2. **Zero wasted work model** — The consume-check-redrive anti-pattern eliminated at the broker level. Consumers only receive ready messages.

3. **Lua rules engine for scheduling policy** — Two hooks (`on_enqueue`, `on_failure`) separate policy from enforcement. Lua = logic, API = data, Rust = enforcement.

4. **Redis-inspired single-threaded scheduler** — Single-threaded core with lock-free IO channels eliminates lock contention on the scheduling hot path.

### Competitive Matrix

| Solution | Fairness | Throttling | Open Source | Standalone Broker |
|----------|----------|------------|-------------|-------------------|
| Kafka, RabbitMQ, Pulsar, NATS | No | No | Yes | Yes |
| Amazon SQS Fair Queues (2025) | Yes | No | No | No (managed) |
| BullMQ Pro | Yes | Yes | No (commercial) | No (Redis library) |
| Temporal | Adding | Adding | Yes | No (workflow engine) |
| Hatchet, Inngest | Custom | Custom | Partial | No (platforms) |
| **Fila** | **Yes (DRR)** | **Yes (token buckets)** | **Yes** | **Yes** |

Fila occupies a genuinely empty cell: open-source, standalone broker, with both fairness and throttling as native primitives.

## API Specification

### Hot Path RPCs

| RPC | Direction | Description |
|-----|-----------|-------------|
| `Enqueue` | Producer → Broker | Submit message with headers and payload. Triggers `on_enqueue` Lua hook. |
| `Lease` | Consumer ← Broker | Server-streaming. Broker pushes ready messages based on DRR + throttle decisions. |
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
- `timestamps` — Enqueue, lease, ack/nack times
- `metadata` — Broker-assigned: fairness_key, weight, throttle_keys, attempt count

### Error Handling

| Condition | Behavior |
|-----------|----------|
| Lua script error/timeout | Circuit breaker: default fairness_key, weight=1, no throttle keys. Log + counter. |
| `on_failure` script error | Default to retry with backoff. Log error. |
| No ready messages | Stream held open; messages pushed when ready. |
| Unknown message Ack/Nack | Error response. Idempotent. |
| Visibility timeout expired | Message re-enters ready pool. At-least-once semantics. |
| Stream disconnection | Leases governed by visibility timeout. Reconnect opens new stream. |

### Authentication

**MVP**: None. Trust the network. **Post-MVP**: API keys, mTLS.

### SDKs

**Launch languages** (6): Go, Python, JavaScript, Ruby, Rust, Java

Thin gRPC wrappers generated from Protobuf definitions with ergonomic language-specific APIs. Core operations: `enqueue()`, `lease()` (streaming), `ack()`, `nack()`. API documentation generated from `.proto` files.

### Implementation Architecture

- **Protocol**: gRPC only (tonic). Single protocol for hot path and admin.
- **Delivery**: Server-streaming gRPC for Lease. Ack/Nack as separate unary RPCs.
- **Versioning**: Protobuf backward compatibility. Field addition only.
- **Concurrency**: gRPC IO threads (tonic/tokio) → lock-free channels → single-threaded scheduler core → lock-free channels → IO threads. Streaming connections managed by IO threads.

## Product Scope & Roadmap

### MVP (Phase 1)

| Milestone | Deliverable | Validates |
|-----------|-------------|-----------|
| M1 | Protobuf envelope, RocksDB storage, gRPC server (Enqueue, Lease, Ack), broker process, OTel | End-to-end skeleton |
| M2 | DRR scheduler, fairness_key + weight, visibility timeout, Nack, per-key metrics | Fair scheduling correctness and performance |
| M3 | mlua integration, pre-compiled scripts, `on_enqueue`/`on_failure` hooks, execution limits, DLQ | User-defined scheduling policy |
| M4 | Token buckets per throttle key, throttle_keys from Lua, skip throttled keys in DRR | Rate limiting alongside fairness |
| M5 | Runtime key-value store, `fila.get()` Lua bridge, admin RPCs, `fila-cli` | Full operator experience |
| M6 | Server-streaming gRPC Lease with backpressure, consumer flow control via stream management | Efficient consumer delivery |
| SDKs | Go, Python, JavaScript, Ruby, Rust, Java client libraries | Developer accessibility |

**MVP Strategy:** Problem-solving MVP — prove the core scheduling primitive works correctly at Kafka-competitive throughput. Solo developer (Lucas), Rust.

**MVP Journeys Supported:** Maya (happy path + Lua scripting), Dev (SDK consumer), Tomas (operations).

### Phase 2 (Production Readiness + Adoption)

- Authentication and authorization — API keys, mTLS
- Distributed clustering — multi-node horizontal scalability
- Consumer groups — broker-managed consumer coordination
- Management GUI — web interface for monitoring and real-time scheduling visualization (adoption driver)

### Phase 3 (Advanced Capabilities)

- Declarative config sugar and built-in Lua helpers
- Payload parsing (`fila.parse_json`, `fila.parse_protobuf`)
- Dual token buckets (committed + burst rates)
- CFS-style fairness debt tracking
- Decision event stream
- WFQ scheduler

### Phase 4 (Platform)

- Custom storage engine
- Terraform provider
- Hosted platform

REST API dropped — reintroduce only if real demand surfaces.

### Risk Analysis

**Technical:**
- **Single-threaded scheduler throughput** — Redis precedent validates model. Benchmark early in M2.
- **Lua on the hot path** — Pre-compiled scripts, timeouts, memory limits, circuit breaker. Benchmark in M3.
- **RocksDB key layout rigidity** — Strict storage trait + column family isolation. Design carefully in M1.
- **Six SDKs as solo dev** — Thin gRPC wrappers from protobuf generation. Stagger releases if needed.

**Market:**
- **Problem validation** — SQS Fair Queues (2025) confirms market need. Consume-check-redrive is well-documented pain.
- **Incumbent risk** — Fairness-at-dequeue requires fundamental broker redesign, incompatible with existing FIFO architectures.

**Resource:**
- **Minimum viable launch** — M1–M5 + streaming + Rust SDK. Other SDKs follow in weeks.

## Functional Requirements

### Message Lifecycle

- **FR1**: Producers can enqueue messages with arbitrary headers and opaque byte payloads
- **FR2**: Consumers can receive ready messages via a persistent server-streaming connection
- **FR3**: Consumers can acknowledge successful message processing
- **FR4**: Consumers can negatively acknowledge failed processing, triggering failure handling logic
- **FR5**: The broker automatically re-makes available messages whose visibility timeout has expired
- **FR6**: The broker routes failed messages to a dead-letter queue based on failure handling rules
- **FR7**: Operators can redrive messages from a dead-letter queue back to the source queue

### Fair Scheduling

- **FR8**: The broker assigns messages to fairness groups based on message properties
- **FR9**: The broker schedules delivery using Deficit Round Robin across fairness groups
- **FR10**: Higher-weighted fairness groups receive proportionally more throughput
- **FR11**: Only active fairness groups (those with pending messages) participate in scheduling
- **FR12**: Each fairness group's actual throughput stays within 5% of its fair share under sustained load

### Throttling

- **FR13**: The broker enforces per-key rate limits using token bucket rate limiting
- **FR14**: Messages can be subject to multiple independent throttle keys simultaneously
- **FR15**: The broker skips throttled keys during scheduling and serves the next ready key
- **FR16**: Operators can set and update throttle rates at runtime without restart
- **FR17**: Hierarchical throttling via multiple throttle key assignment

### Rules Engine

- **FR18**: Operators can attach Lua scripts to queues that execute on message enqueue
- **FR19**: `on_enqueue` computes and assigns fairness key, weight, and throttle keys from message headers
- **FR20**: Operators can attach Lua scripts that execute on message failure
- **FR21**: `on_failure` decides between retry (with delay) and dead-letter based on properties and attempt count
- **FR22**: Lua scripts can read runtime configuration values via `fila.get()`
- **FR23**: The broker enforces execution timeouts and memory limits on Lua scripts
- **FR24**: Circuit breaker falls back to safe defaults on Lua error or timeout
- **FR25**: Lua scripts are pre-compiled and cached for repeated execution

### Runtime Configuration

- **FR26**: External systems can set key-value config entries via the API
- **FR27**: Lua scripts read config at execution time via `fila.get()`
- **FR28**: Config changes take effect on subsequent enqueues without restart
- **FR29**: Operators can view current configuration state

### Queue Management

- **FR30**: Operators can create named queues with Lua scripts and configuration
- **FR31**: Operators can delete queues and their messages
- **FR32**: The broker auto-creates a dead-letter queue for each queue
- **FR33**: Operators can view queue stats: depth, per-key throughput, scheduling state
- **FR34**: All queue management available through `fila-cli`

### Observability

- **FR35**: The broker exports OpenTelemetry-compatible metrics for all operations
- **FR36**: Per-fairness-key throughput metrics (actual vs fair share)
- **FR37**: Per-throttle-key hit/pass rate metrics and token bucket state
- **FR38**: DRR scheduling round metrics
- **FR39**: Lua execution time histograms and error counters
- **FR40**: Tracing spans for enqueue, lease, ack, and nack
- **FR41**: Operators can diagnose "why was key X delayed?" from broker metrics alone

### Client SDKs

- **FR42**: Go client SDK
- **FR43**: Python client SDK
- **FR44**: JavaScript/Node.js client SDK
- **FR45**: Ruby client SDK
- **FR46**: Rust client SDK
- **FR47**: Java client SDK
- **FR48**: All SDKs support enqueue, streaming lease, ack, and nack

### Deployment & Distribution

- **FR49**: Single binary process
- **FR50**: Docker image
- **FR51**: `cargo install` installation
- **FR52**: Shell script installation (`curl | bash`)
- **FR53**: Full crash recovery with zero message loss

### Documentation & LLM Readiness

- **FR54**: Comprehensive, example-rich documentation for all fila concepts
- **FR55**: Working code examples for every SDK in every supported language
- **FR56**: Guided tutorials for common use cases (multi-tenant fairness, per-provider throttling, retry policies)
- **FR57**: API reference generated from Protobuf definitions
- **FR58**: Structured `llms.txt` for LLM agent consumption
- **FR59**: Copy-paste Lua hook examples for common patterns
- **FR60**: Documentation as the primary onboarding experience (docs-as-product)

## Non-Functional Requirements

### Performance

- **NFR1**: 100,000+ msg/s single-node throughput on commodity hardware
- **NFR2**: < 5% throughput cost for fair scheduling vs raw FIFO
- **NFR3**: Fairness accuracy within 5% of fair share under sustained load
- **NFR4**: Lua `on_enqueue` < 50μs p99
- **NFR5**: Lua `on_failure` < 50μs p99
- **NFR6**: Enqueue-to-lease latency < 1ms p50 when consumer is waiting
- **NFR7**: Token bucket decisions < 1μs overhead per scheduling loop iteration

### Reliability

- **NFR8**: Full state recovery after crash, zero message loss
- **NFR9**: Atomic enqueue persistence (RocksDB WriteBatch)
- **NFR10**: Expired visibility timeouts resolved within one scheduling cycle
- **NFR11**: Lua circuit breaker activates within 3 consecutive failures
- **NFR12**: At-least-once delivery under all failure conditions

### Operability

- **NFR13**: Single engineer can deploy, configure, and monitor without tribal knowledge
- **NFR14**: Download to fair-scheduling demo in under 10 minutes
- **NFR15**: Single binary, no external runtime dependencies
- **NFR16**: All state queryable via `fila-cli` or OTel metrics
- **NFR17**: Runtime config changes without restart or downtime

### Integration

- **NFR18**: OTel-compatible metrics exportable to Prometheus, Grafana, Datadog
- **NFR19**: gRPC best practices (status codes, metadata propagation, deadlines)
- **NFR20**: Protobuf backward compatibility across minor versions
- **NFR21**: Idiomatic SDK conventions per target language
