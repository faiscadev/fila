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
---

# Fila - Epic Breakdown

## Overview

This document provides the complete epic and story breakdown for Fila, decomposing the requirements from the PRD and Architecture into implementable stories.

## Requirements Inventory

### Functional Requirements

FR1: Producers can enqueue messages with arbitrary headers and opaque byte payloads
FR2: Consumers can receive ready messages via a persistent server-streaming connection
FR3: Consumers can acknowledge successful message processing
FR4: Consumers can negatively acknowledge failed processing, triggering failure handling logic
FR5: The broker automatically re-makes available messages whose visibility timeout has expired
FR6: The broker routes failed messages to a dead-letter queue based on failure handling rules
FR7: Operators can redrive messages from a dead-letter queue back to the source queue
FR8: The broker assigns messages to fairness groups based on message properties
FR9: The broker schedules delivery using Deficit Round Robin across fairness groups
FR10: Higher-weighted fairness groups receive proportionally more throughput
FR11: Only active fairness groups (those with pending messages) participate in scheduling
FR12: Each fairness group's actual throughput stays within 5% of its fair share under sustained load
FR13: The broker enforces per-key rate limits using token bucket rate limiting
FR14: Messages can be subject to multiple independent throttle keys simultaneously
FR15: The broker skips throttled keys during scheduling and serves the next ready key
FR16: Operators can set and update throttle rates at runtime without restart
FR17: Hierarchical throttling via multiple throttle key assignment
FR18: Operators can attach Lua scripts to queues that execute on message enqueue
FR19: `on_enqueue` computes and assigns fairness key, weight, and throttle keys from message headers
FR20: Operators can attach Lua scripts that execute on message failure
FR21: `on_failure` decides between retry (with delay) and dead-letter based on properties and attempt count
FR22: Lua scripts can read runtime configuration values via `fila.get()`
FR23: The broker enforces execution timeouts and memory limits on Lua scripts
FR24: Circuit breaker falls back to safe defaults on Lua error or timeout
FR25: Lua scripts are pre-compiled and cached for repeated execution
FR26: External systems can set key-value config entries via the API
FR27: Lua scripts read config at execution time via `fila.get()`
FR28: Config changes take effect on subsequent enqueues without restart
FR29: Operators can view current configuration state
FR30: Operators can create named queues with Lua scripts and configuration
FR31: Operators can delete queues and their messages
FR32: The broker auto-creates a dead-letter queue for each queue
FR33: Operators can view queue stats: depth, per-key throughput, scheduling state
FR34: All queue management available through `fila-cli`
FR35: The broker exports OpenTelemetry-compatible metrics for all operations
FR36: Per-fairness-key throughput metrics (actual vs fair share)
FR37: Per-throttle-key hit/pass rate metrics and token bucket state
FR38: DRR scheduling round metrics
FR39: Lua execution time histograms and error counters
FR40: Tracing spans for enqueue, lease, ack, and nack
FR41: Operators can diagnose "why was key X delayed?" from broker metrics alone
FR42: Go client SDK
FR43: Python client SDK
FR44: JavaScript/Node.js client SDK
FR45: Ruby client SDK
FR46: Rust client SDK
FR47: Java client SDK
FR48: All SDKs support enqueue, streaming consume, ack, and nack
FR49: Single binary process
FR50: Docker image
FR51: `cargo install` installation
FR52: Shell script installation (`curl | bash`)
FR53: Full crash recovery with zero message loss
FR54: Comprehensive, example-rich documentation for all fila concepts
FR55: Working code examples for every SDK in every supported language
FR56: Guided tutorials for common use cases (multi-tenant fairness, per-provider throttling, retry policies)
FR57: API reference generated from Protobuf definitions
FR58: Structured `llms.txt` for LLM agent consumption
FR59: Copy-paste Lua hook examples for common patterns
FR60: Documentation as the primary onboarding experience (docs-as-product)

### NonFunctional Requirements

NFR1: 100,000+ msg/s single-node throughput on commodity hardware
NFR2: < 5% throughput cost for fair scheduling vs raw FIFO
NFR3: Fairness accuracy within 5% of fair share under sustained load
NFR4: Lua `on_enqueue` < 50μs p99
NFR5: Lua `on_failure` < 50μs p99
NFR6: Enqueue-to-lease latency < 1ms p50 when consumer is waiting
NFR7: Token bucket decisions < 1μs overhead per scheduling loop iteration
NFR8: Full state recovery after crash, zero message loss
NFR9: Atomic enqueue persistence (RocksDB WriteBatch)
NFR10: Expired visibility timeouts resolved within one scheduling cycle
NFR11: Lua circuit breaker activates within 3 consecutive failures
NFR12: At-least-once delivery under all failure conditions
NFR13: Single engineer can deploy, configure, and monitor without tribal knowledge
NFR14: Download to fair-scheduling demo in under 10 minutes
NFR15: Single binary, no external runtime dependencies
NFR16: All state queryable via `fila-cli` or OTel metrics
NFR17: Runtime config changes without restart or downtime
NFR18: OTel-compatible metrics exportable to Prometheus, Grafana, Datadog
NFR19: gRPC best practices (status codes, metadata propagation, deadlines)
NFR20: Protobuf backward compatibility across minor versions
NFR21: Idiomatic SDK conventions per target language

### Additional Requirements

- Architecture specifies a Cargo workspace foundation with 4 crates: fila-proto, fila-core, fila-server, fila-cli — this is the starter/greenfield template (impacts Epic 1, Story 1)
- Redis-inspired single-threaded scheduler core with lock-free crossbeam IO channels — concurrency model is a critical architectural decision
- RocksDB with 6 column families (default, messages, leases, lease_expiry, queues, state) — specific key encoding scheme with big-endian composite keys
- gRPC only via tonic — single protocol for hot path and admin operations
- Protobuf organized in 3 files: messages.proto, service.proto, admin.proto
- mlua (Lua 5.4) with pre-compiled scripts, execution timeouts, memory limits, and circuit breaker
- Two-layer configuration: static TOML file + runtime gRPC key-value store in RocksDB state CF
- thiserror for typed errors in fila-core, mapped to gRPC status codes at server boundary
- Four-layer testing: unit tests, integration tests (cargo nextest), property-based (proptest), benchmarks (criterion)
- CI/CD via GitHub Actions: PR checks (fmt, clippy, test, bench) + Docker builds + release binaries
- Distribution channels: Docker (ghcr.io), cargo install, curl|bash shell script, GitHub Releases
- UUIDv7 for message IDs — time-ordered, globally unique, no coordination
- Graceful shutdown sequence: stop accepting connections → drain scheduler → flush RocksDB WAL → exit
- In-memory state rebuilt on startup: DRR state, token buckets, consumer registry, Lua VMs
- AGPLv3 license required; all dependencies must be AGPL-compatible
- Startup recovery sequence: open RocksDB → load queues → rebuild active keys → restore leases → reclaim expired leases

### FR Coverage Map

FR1: Epic 1 — Enqueue messages with headers and payload
FR2: Epic 1 — Receive ready messages via streaming lease
FR3: Epic 1 — Acknowledge successful processing
FR4: Epic 2 — Negatively acknowledge failed processing
FR5: Epic 2 — Visibility timeout expiry and redelivery
FR6: Epic 3 — Route failed messages to dead-letter queue
FR7: Epic 5 — Redrive messages from DLQ to source queue
FR8: Epic 2 — Assign messages to fairness groups
FR9: Epic 2 — Deficit Round Robin scheduling across fairness groups
FR10: Epic 2 — Weighted throughput for fairness groups
FR11: Epic 2 — Only active groups participate in scheduling
FR12: Epic 2 — Fairness accuracy within 5% of fair share
FR13: Epic 4 — Per-key token bucket rate limiting
FR14: Epic 4 — Multiple simultaneous throttle keys
FR15: Epic 4 — Skip throttled keys during scheduling
FR16: Epic 4 — Runtime throttle rate updates
FR17: Epic 4 — Hierarchical throttling via multiple keys
FR18: Epic 3 — Attach Lua scripts for on_enqueue
FR19: Epic 3 — on_enqueue computes fairness key, weight, throttle keys
FR20: Epic 3 — Attach Lua scripts for on_failure
FR21: Epic 3 — on_failure decides retry vs dead-letter
FR22: Epic 3 — Lua reads runtime config via fila.get()
FR23: Epic 3 — Execution timeouts and memory limits on Lua
FR24: Epic 3 — Circuit breaker with safe defaults
FR25: Epic 3 — Pre-compiled and cached Lua scripts
FR26: Epic 5 — Set key-value config entries via API
FR27: Epic 5 — Lua reads config at execution time
FR28: Epic 5 — Config changes without restart
FR29: Epic 5 — View current configuration state
FR30: Epic 1 — Create named queues with config
FR31: Epic 1 — Delete queues and messages
FR32: Epic 3 — Auto-create dead-letter queue per queue
FR33: Epic 5 — View queue stats and scheduling state
FR34: Epic 5 — All queue management via fila-cli
FR35: Epic 6 — OTel-compatible metrics for all operations
FR36: Epic 6 — Per-fairness-key throughput metrics
FR37: Epic 6 — Per-throttle-key hit/pass rate metrics
FR38: Epic 6 — DRR scheduling round metrics
FR39: Epic 6 — Lua execution time histograms and error counters
FR40: Epic 6 — Tracing spans for enqueue, lease, ack, nack
FR41: Epic 6 — Diagnose "why was key X delayed?" from metrics
FR42: Epic 9 — Go client SDK
FR43: Epic 9 — Python client SDK
FR44: Epic 9 — JavaScript/Node.js client SDK
FR45: Epic 9 — Ruby client SDK
FR46: Epic 7 — Rust client SDK
FR47: Epic 9 — Java client SDK
FR48: Epic 7 + Epic 9 — All SDKs support enqueue, streaming consume, ack, nack
FR49: Epic 1 — Single binary process
FR50: Epic 10 — Docker image
FR51: Epic 10 — cargo install installation
FR52: Epic 10 — Shell script installation
FR53: Epic 1 — Full crash recovery with zero message loss
FR54: Epic 10 — Comprehensive, example-rich documentation
FR55: Epic 10 — Working code examples per SDK per language
FR56: Epic 10 — Guided tutorials for common use cases
FR57: Epic 10 — API reference generated from Protobuf
FR58: Epic 10 — Structured llms.txt for LLM agents
FR59: Epic 10 — Copy-paste Lua hook examples
FR60: Epic 10 — Documentation as primary onboarding experience

## Epic List

### Epic 1: Foundation & End-to-End Messaging
Producers can enqueue messages and consumers can receive and acknowledge them through gRPC. The broker runs as a single binary with persistent RocksDB storage, crash recovery, and basic queue management. This establishes the end-to-end skeleton that all subsequent epics build upon.
**FRs covered:** FR1, FR2, FR3, FR30, FR31, FR49, FR53

### Epic 2: Fair Scheduling & Message Reliability
The broker delivers messages fairly across groups using Deficit Round Robin. Consumers can nack failed messages, and visibility timeouts enable at-least-once delivery. This is the core differentiator — the scheduling primitive that no other open-source broker provides.
**FRs covered:** FR4, FR5, FR8, FR9, FR10, FR11, FR12

### Epic 3: Lua Rules Engine & Dead-Letter Handling
Users define scheduling policy through Lua scripts. `on_enqueue` assigns fairness keys, weights, and throttle keys from message headers. `on_failure` controls retry behavior and routes exhausted messages to dead-letter queues. Scripts are sandboxed with timeouts, memory limits, and circuit breaker fallback.
**FRs covered:** FR6, FR18, FR19, FR20, FR21, FR22, FR23, FR24, FR25, FR32

### Epic 4: Throttling & Rate Limiting
Per-key token bucket rate limiting integrated into the scheduler. The broker skips throttled keys during DRR scheduling and serves the next ready key. Messages can have multiple simultaneous throttle keys for hierarchical rate limiting. Zero wasted work — consumers never receive throttled messages.
**FRs covered:** FR13, FR14, FR15, FR16, FR17

### Epic 5: Operator Experience (Admin, Config & CLI)
Operators manage runtime configuration via the gRPC API, redrive DLQ messages, inspect queue stats and per-key metrics, and perform all admin tasks through `fila-cli`. Config changes take effect without restart, enabling live operational adjustments.
**FRs covered:** FR7, FR26, FR27, FR28, FR29, FR33, FR34

### Epic 6: Observability & Diagnostics
Full observability via OpenTelemetry metrics and distributed tracing. Per-fairness-key throughput, per-throttle-key hit rates, DRR scheduling rounds, Lua execution histograms. Operators can answer "why was key X delayed?" from broker metrics alone.
**FRs covered:** FR35, FR36, FR37, FR38, FR39, FR40, FR41

### Epic 7: Rust Client SDK
Developers integrate Fila into Rust applications using an idiomatic client SDK built as a thin wrapper over tonic gRPC client. This SDK also serves as the client for the blackbox e2e test suite in Epic 8.
**FRs covered:** FR46, FR48 (partial — Rust only)

### Epic 8: E2E Tests & Scheduler Refactoring
Build a true blackbox e2e test suite using the Rust SDK (for producer/consumer operations) and the `fila` CLI binary (for admin operations), then use it as a safety net to decompose the monolithic `scheduler.rs` (6,800+ lines) into focused submodules. The observability layer from Epic 6 provides runtime verification during restructuring.

### Epic 9: Client SDKs
Developers integrate Fila into applications using idiomatic client SDKs in five additional languages: Go, Python, JavaScript/Node.js, Ruby, and Java. All SDKs support the full hot-path API: enqueue, streaming consume, ack, and nack.
**FRs covered:** FR42, FR43, FR44, FR45, FR47, FR48 (partial — 5 languages)

### Epic 10: Distribution & Documentation
Users install Fila via Docker, `cargo install`, or `curl | bash` shell script. Comprehensive documentation — tutorials, API reference, Lua hook examples, and `llms.txt` — enables onboarding in under 10 minutes. Documentation is the primary adoption driver.
**FRs covered:** FR50, FR51, FR52, FR54, FR55, FR56, FR57, FR58, FR59, FR60

---

## Epic 1: Foundation & End-to-End Messaging

Producers can enqueue messages and consumers can receive and acknowledge them through gRPC. The broker runs as a single binary with persistent RocksDB storage, crash recovery, and basic queue management. This establishes the end-to-end skeleton that all subsequent epics build upon.

### Story 1.1: Cargo Workspace & Protobuf Definitions

As a developer,
I want the project workspace and protobuf API definitions to be established,
So that all crates have a shared foundation and generated types to build upon.

**Acceptance Criteria:**

**Given** no project structure exists
**When** the workspace is initialized
**Then** a Cargo workspace root exists with four member crates: `fila-proto`, `fila-core`, `fila-server`, `fila-cli`
**And** `proto/fila/v1/messages.proto` defines the `Message` envelope with id, headers, payload, timestamps, and metadata fields
**And** `proto/fila/v1/service.proto` defines `FilaService` with `Enqueue`, `Lease`, `Ack`, and `Nack` RPCs
**And** `proto/fila/v1/admin.proto` defines `FilaAdmin` with `CreateQueue`, `DeleteQueue`, `SetConfig`, `GetConfig`, `GetStats`, and `Redrive` RPCs
**And** `fila-proto` has a `build.rs` that runs `prost-build` and `tonic-build` to generate Rust types and gRPC service/client code
**And** `fila-proto/src/lib.rs` re-exports all generated types and services
**And** all four crates compile successfully with `cargo build`
**And** the workspace `Cargo.toml` uses workspace-level dependency management
**And** a `LICENSE` file with AGPLv3 text is present at the workspace root

### Story 1.2: CI/CD Pipeline

As a developer,
I want automated CI for every PR from the start of the project,
So that code quality is enforced continuously and regressions are caught immediately.

**Acceptance Criteria:**

**Given** the Cargo workspace compiles successfully
**When** a GitHub Actions CI workflow is created
**Then** `.github/workflows/ci.yml` runs on every pull request and push to main
**And** the workflow runs `cargo fmt --check` to verify code formatting
**And** the workflow runs `cargo clippy` with warnings as errors
**And** the workflow runs `cargo nextest run` to execute all tests
**And** the workflow runs `cargo bench` (informational, no regression gate)
**And** the CI pipeline passes on the initial workspace with all four crates

**Given** a commit is merged to main
**When** the main CI runs
**Then** all PR checks run (fmt, clippy, test, bench)

**Given** a version tag is pushed
**When** the release workflow runs
**Then** all checks pass
**And** release binaries are built for all target platforms (linux-amd64, linux-arm64, darwin-amd64, darwin-arm64)
**And** a GitHub Release is created with the binaries and changelog
**And** Docker image is tagged with the version and pushed

### Story 1.3: Core Domain Types & Storage Layer

As a developer,
I want message domain types and a persistent storage layer with RocksDB,
So that messages can be durably stored and retrieved with correct ordering guarantees.

**Acceptance Criteria:**

**Given** the fila-proto crate provides generated protobuf types
**When** the storage layer is implemented in fila-core
**Then** a `Storage` trait is defined with methods for message CRUD, lease management, queue config, and state operations
**And** a `RocksDbStorage` implementation opens RocksDB with column families: `default`, `messages`, `leases`, `lease_expiry`, `queues`, `state`
**And** message keys in the `messages` CF use the format `{queue_id}:{fairness_key}:{enqueue_ts_ns}:{msg_id}` with big-endian numeric encoding
**And** lease keys use `{queue_id}:{msg_id}` format in the `leases` CF
**And** lease expiry keys use `{expiry_ts_ns}:{queue_id}:{msg_id}` format in the `lease_expiry` CF for efficient timeout scanning
**And** message IDs are UUIDv7 (time-ordered, globally unique)
**And** all multi-key mutations use RocksDB `WriteBatch` for atomicity
**And** a `FilaError` enum using `thiserror` is defined with variants: `QueueNotFound`, `MessageNotFound`, `LuaError`, `StorageError`, `InvalidConfig`, `QueueAlreadyExists`
**And** unit tests verify key encoding produces correct lexicographic ordering
**And** unit tests verify WriteBatch atomicity (all-or-nothing writes)

### Story 1.4: Broker Core & Scheduler Loop

As a developer,
I want the single-threaded scheduler core with channel-based communication,
So that all state mutations happen on a dedicated thread without lock contention.

**Acceptance Criteria:**

**Given** the storage layer is available
**When** the broker core is implemented
**Then** a `Broker` struct spawns a dedicated `std::thread` for the scheduler core
**And** `crossbeam-channel` bounded channels connect IO threads (inbound) to the scheduler (outbound)
**And** a `SchedulerCommand` enum defines all commands: `Enqueue`, `Ack`, `Nack`, `RegisterConsumer`, `UnregisterConsumer`, `Admin`
**And** each command variant includes a `oneshot::Sender` for request-response patterns (where applicable)
**And** the scheduler core runs a tight event loop: drain inbound commands (non-blocking `try_recv`), then park with timeout if idle
**And** the scheduler loop contains no async code and never blocks on IO
**And** the `Broker` provides a public API for sending commands that internally sends via the crossbeam channel
**And** `BrokerConfig` struct deserializes from TOML with sections for server, scheduler, lua, and telemetry
**And** graceful shutdown is supported: signal → stop accepting commands → drain in-flight → flush RocksDB WAL → exit
**And** a `tracing` subscriber is configured for structured stdout logging (JSON in release, pretty-print in debug) so all subsequent code can use `tracing::info!`, `tracing::warn!`, etc. from day one
**And** all public functions in the broker module use `#[tracing::instrument]` or manual spans per the Architecture enforcement guidelines
**And** unit tests verify the scheduler processes commands in order

### Story 1.5: gRPC Server & Queue Management

As an operator,
I want to start the broker and create/delete queues via gRPC,
So that I can set up the message infrastructure for my application.

**Acceptance Criteria:**

**Given** the broker core is running with the scheduler thread
**When** the gRPC server starts
**Then** a tonic gRPC server listens on the configured address (default `0.0.0.0:5555`)
**And** `CreateQueue` RPC accepts a queue name and configuration, persists it to the `queues` CF, and returns success
**And** `CreateQueue` returns `ALREADY_EXISTS` status if the queue name is taken
**And** `DeleteQueue` RPC removes the queue definition and all its messages from storage
**And** `DeleteQueue` returns `NOT_FOUND` status if the queue does not exist
**And** gRPC handlers send `SchedulerCommand::Admin` variants to the scheduler via crossbeam channel
**And** the scheduler processes admin commands and replies via oneshot channel
**And** gRPC error responses use standard status codes: `NOT_FOUND`, `ALREADY_EXISTS`, `INVALID_ARGUMENT`, `INTERNAL`
**And** the server binary (`fila-server`) reads config from `fila.toml` or `/etc/fila/fila.toml`, with env var overrides (`FILA_SERVER__LISTEN_ADDR`)
**And** an integration test creates a queue, verifies it exists, deletes it, and verifies it is gone

### Story 1.6: Message Enqueue

As a producer,
I want to enqueue messages with headers and payload to a named queue,
So that my messages are durably stored and available for consumers.

**Acceptance Criteria:**

**Given** a queue has been created
**When** a producer calls the `Enqueue` RPC with a queue name, headers map, and payload bytes
**Then** the scheduler assigns a UUIDv7 message ID and a default fairness key (value `"default"`)
**And** the message is persisted to the `messages` CF via WriteBatch with the composite key format
**And** the `EnqueueResponse` returns the assigned message ID
**And** calling `Enqueue` on a non-existent queue returns `NOT_FOUND` status
**And** headers can contain arbitrary string key-value pairs
**And** payload is stored as opaque bytes (broker never inspects content)
**And** an integration test enqueues 100 messages and verifies all are persisted with unique, time-ordered IDs

### Story 1.7: Consumer Lease & Message Delivery

As a consumer,
I want to receive ready messages from a queue via a streaming connection,
So that I can process messages as they become available without polling.

**Acceptance Criteria:**

**Given** messages have been enqueued to a queue
**When** a consumer calls the `Lease` RPC with a queue name
**Then** a server-streaming gRPC connection is established
**And** the scheduler registers the consumer with a per-consumer `mpsc::Sender`
**And** pending messages are delivered to the consumer through the stream
**And** each delivered message includes: message ID, headers, payload, metadata (fairness_key, attempt count)
**And** a lease is created in the `leases` CF with `{queue_id}:{msg_id}` → `{consumer_id}:{expiry_ts_ns}`
**And** a lease expiry entry is created in the `lease_expiry` CF with `{expiry_ts_ns}:{queue_id}:{msg_id}`
**And** the visibility timeout is configurable per queue (default 30 seconds)
**And** when the consumer disconnects, the scheduler unregisters the consumer
**And** multiple consumers can lease from the same queue simultaneously (each gets different messages)
**And** an integration test enqueues 10 messages, opens a lease stream, and receives all 10 messages

### Story 1.8: Message Acknowledgment

As a consumer,
I want to acknowledge successfully processed messages,
So that they are permanently removed from the queue.

**Acceptance Criteria:**

**Given** a consumer has leased a message
**When** the consumer calls the `Ack` RPC with the queue name and message ID
**Then** the message is removed from the `messages` CF
**And** the lease is removed from the `leases` CF
**And** the lease expiry entry is removed from the `lease_expiry` CF
**And** all three removals happen atomically via WriteBatch
**And** acknowledging an unknown message ID returns `NOT_FOUND` status (idempotent — no side effects)
**And** acknowledging a message on a non-existent queue returns `NOT_FOUND` status
**And** an integration test verifies the full lifecycle: enqueue → lease → ack → verify message is gone
**And** an integration test verifies that acking the same message twice returns `NOT_FOUND` on the second call

### Story 1.9: Crash Recovery & Graceful Shutdown

As an operator,
I want the broker to recover all state after a crash with zero message loss,
So that I can trust the system to be durable without manual intervention.

**Acceptance Criteria:**

**Given** the broker has messages, leases, and queue definitions persisted in RocksDB
**When** the broker process is killed and restarted
**Then** all queue definitions are loaded from the `queues` CF
**And** the scheduler rebuilds its in-memory state by scanning the `messages` CF (active keys, pending message counts)
**And** in-flight leases are restored from the `leases` CF
**And** already-expired leases (identified by scanning `lease_expiry` CF) are reclaimed immediately — messages re-enter the ready pool
**And** the broker is ready to accept gRPC connections after recovery completes
**And** no messages are lost during the recovery process
**And** graceful shutdown flushes the RocksDB WAL before exit
**And** an integration test enqueues messages, kills the broker process, restarts, and verifies all messages are still available for leasing

---

## Epic 2: Fair Scheduling & Message Reliability

The broker delivers messages fairly across groups using Deficit Round Robin. Consumers can nack failed messages, and visibility timeouts enable at-least-once delivery. This is the core differentiator — the scheduling primitive that no other open-source broker provides.

### Story 2.1: Deficit Round Robin Scheduler

As a platform engineer,
I want the broker to schedule message delivery fairly across fairness groups using DRR,
So that no single high-volume tenant starves other tenants of throughput.

**Acceptance Criteria:**

**Given** messages are enqueued with fairness keys assigned (currently defaulting to `"default"`, but supporting distinct keys via message metadata)
**When** the DRR scheduler runs its scheduling round
**Then** each active fairness key receives a deficit allocation of `weight * quantum` per round
**And** messages are delivered from each key until its deficit is exhausted, then the scheduler moves to the next key
**And** only active keys (those with pending messages) participate in scheduling rounds (FR11)
**And** when a key's pending messages are exhausted, it is removed from the active set
**And** when new messages arrive for an inactive key, it is added back to the active set
**And** the DRR data structures (active keys, deficit counters, round position) are stored in-memory on the scheduler thread
**And** unit tests verify round-robin behavior: with 3 keys of equal weight, each gets ~33% of delivered messages
**And** a benchmark compares DRR scheduling throughput vs raw FIFO to verify <5% overhead (NFR2)

### Story 2.2: Weighted Fairness & Accuracy Verification

As a platform engineer,
I want higher-weighted fairness groups to receive proportionally more throughput,
So that I can prioritize important tenants while still guaranteeing fair shares for all.

**Acceptance Criteria:**

**Given** messages are enqueued across multiple fairness keys with different weights
**When** the DRR scheduler operates under sustained load
**Then** a key with weight 2 receives approximately 2x the throughput of a key with weight 1
**And** each key's actual throughput stays within 5% of its calculated fair share (FR12)
**And** the default weight for keys without explicit assignment is 1
**And** weight changes take effect on the next scheduling round
**And** property-based tests (proptest) verify the fairness invariant: for any combination of keys and weights under sustained load, each key receives within 5% of `(key_weight / total_weight) * total_throughput`
**And** the fairness accuracy test runs with at least 10,000 messages across 5+ keys with varying weights

### Story 2.3: Nack & Message Retry

As a consumer,
I want to negatively acknowledge a message that I failed to process,
So that it is retried and not lost.

**Acceptance Criteria:**

**Given** a consumer has leased a message
**When** the consumer calls the `Nack` RPC with the queue name, message ID, and an error description
**Then** the message's attempt count is incremented
**And** the message re-enters the ready pool for the same fairness key
**And** the lease is removed from the `leases` CF
**And** the lease expiry entry is removed from the `lease_expiry` CF
**And** the updated message (with new attempt count) is written atomically via WriteBatch
**And** nacking an unknown message ID returns `NOT_FOUND` status (idempotent)
**And** the default retry behavior (without Lua) is immediate requeue with no maximum attempt limit
**And** an integration test verifies: enqueue → lease → nack → lease again → verify attempt count is incremented

### Story 2.4: Visibility Timeout & Lease Expiry

As an operator,
I want messages with expired visibility timeouts to automatically become available again,
So that stuck or crashed consumers don't block message processing.

**Acceptance Criteria:**

**Given** a message has been leased to a consumer
**When** the visibility timeout expires without an ack or nack
**Then** the scheduler's expiry check (scanning `lease_expiry` CF from earliest) identifies the expired lease
**And** the message re-enters the ready pool for its fairness key
**And** the expired lease and lease expiry entries are removed via WriteBatch
**And** expired leases are resolved within one scheduling cycle (NFR10)
**And** the visibility timeout is configurable per queue via `CreateQueue` configuration
**And** the scheduler checks for expired leases on every loop iteration using the timestamp-ordered `lease_expiry` CF
**And** at-least-once delivery is guaranteed: no message is lost due to consumer failure (NFR12)
**And** an integration test leases a message, waits for timeout expiry, and verifies the message is available for re-lease

---

## Epic 3: Lua Rules Engine & Dead-Letter Handling

Users define scheduling policy through Lua scripts. `on_enqueue` assigns fairness keys, weights, and throttle keys from message headers. `on_failure` controls retry behavior and routes exhausted messages to dead-letter queues. Scripts are sandboxed with timeouts, memory limits, and circuit breaker fallback.

### Story 3.1: Lua Sandbox & on_enqueue Hook

As a platform engineer,
I want to write Lua scripts that assign fairness keys, weights, and throttle keys from message headers at enqueue time,
So that I can define custom scheduling policy without modifying the broker.

**Acceptance Criteria:**

**Given** a queue is created with an `on_enqueue` Lua script
**When** a message is enqueued to that queue
**Then** the Lua script receives a `msg` table with: `msg.headers` (read-only table), `msg.payload_size` (number), `msg.queue` (string)
**And** the script returns a table with: `fairness_key` (string), `weight` (number, optional, default 1), `throttle_keys` (array of strings, optional)
**And** the broker uses the returned values to assign scheduling metadata to the message
**And** the Lua sandbox has access to `fila.get(key)` for reading runtime config from the `state` CF
**And** the sandbox provides standard Lua string, math, and table libraries
**And** the sandbox does NOT provide IO, OS, filesystem, or network access
**And** scripts are pre-compiled to bytecode at queue creation time and cached (FR25)
**And** pre-compiled scripts are reused for every message — no parse overhead per enqueue
**And** an integration test creates a queue with an on_enqueue script that reads `msg.headers["tenant_id"]` and assigns it as the fairness key, then verifies messages are assigned the correct keys

### Story 3.2: Lua Safety: Timeouts, Memory Limits & Circuit Breaker

As an operator,
I want Lua scripts to be safely sandboxed with execution limits and automatic fallback,
So that a buggy script cannot crash or slow down the broker.

**Acceptance Criteria:**

**Given** a queue has a Lua script attached
**When** the script exceeds its execution timeout
**Then** the script is terminated via mlua instruction count hook
**And** the circuit breaker counter is incremented
**And** the default timeout is 10ms, configurable per queue (FR23)

**Given** a queue has a Lua script attached
**When** the script exceeds its memory limit
**Then** the script is terminated via mlua allocator hook
**And** the circuit breaker counter is incremented
**And** the default memory limit is 1MB, configurable per queue (FR23)

**Given** a Lua script has failed 3 consecutive times (configurable threshold)
**When** the next message is enqueued
**Then** the circuit breaker is active: Lua is bypassed entirely
**And** safe defaults are applied: `fairness_key = "default"`, `weight = 1`, no throttle keys (FR24)
**And** a warning is logged and an error counter is incremented
**And** the circuit breaker remains active for a configurable cooldown period (default 10 seconds)
**And** after the cooldown, the next enqueue attempts Lua execution again
**And** a successful Lua execution resets the consecutive failure counter
**And** unit tests verify circuit breaker activation at exactly the threshold count
**And** unit tests verify the cooldown period behavior

### Story 3.3: on_failure Hook & Retry Decisions

As a platform engineer,
I want to write a Lua script that decides whether a failed message should be retried or dead-lettered,
So that I can implement custom retry policies like exponential backoff with max attempts.

**Acceptance Criteria:**

**Given** a queue is created with an `on_failure` Lua script
**When** a consumer nacks a message
**Then** the Lua script receives: `msg.headers` (table), `msg.id` (string), `msg.attempts` (number), `msg.queue` (string), and `error` (string)
**And** the script returns a table with: `action` ("retry" or "dlq") and `delay_ms` (number, optional, for delayed retry)
**And** if action is "retry": the message is requeued with incremented attempt count and optional delay
**And** if action is "dlq": the message is moved to the queue's dead-letter queue
**And** if no on_failure script is attached, the default behavior is immediate retry (backward compatible with Epic 2)
**And** the same safety limits (timeout, memory, circuit breaker) apply to on_failure as to on_enqueue
**And** on circuit breaker fallback for on_failure, the default action is "retry" with no delay
**And** an integration test creates a queue with on_failure that dead-letters after 3 attempts, enqueues a message, nacks it 3 times, and verifies it appears in the DLQ

### Story 3.4: Dead-Letter Queue

As an operator,
I want failed messages to be automatically routed to a dead-letter queue,
So that I can inspect and potentially redrive unprocessable messages.

**Acceptance Criteria:**

**Given** a queue is created
**When** queue creation completes
**Then** a corresponding dead-letter queue is automatically created with the name `{queue_name}.dlq` (FR32)
**And** the DLQ queue ID is stored in the parent queue's configuration

**Given** the on_failure Lua script returns `action = "dlq"` for a nacked message
**When** the scheduler processes the nack
**Then** the message is moved from the source queue to its DLQ via WriteBatch
**And** the original message metadata (headers, payload, all attempt history) is preserved
**And** the message is removed from the source queue's messages CF
**And** the lease and lease expiry entries are removed

**Given** a DLQ contains messages
**When** a consumer leases from the DLQ
**Then** DLQ messages are delivered like any other queue's messages (DLQ is a regular queue)
**And** an integration test verifies the full flow: enqueue → nack with dlq action → verify message in DLQ → lease from DLQ

---

## Epic 4: Throttling & Rate Limiting

Per-key token bucket rate limiting integrated into the scheduler. The broker skips throttled keys during DRR scheduling and serves the next ready key. Messages can have multiple simultaneous throttle keys for hierarchical rate limiting. Zero wasted work — consumers never receive throttled messages.

### Story 4.1: Token Bucket Implementation

As a developer,
I want a token bucket rate limiter implementation,
So that per-key rate limits can be enforced in the scheduler.

**Acceptance Criteria:**

**Given** a token bucket is configured with a rate (tokens per second) and burst size
**When** the bucket is checked for token availability
**Then** tokens are refilled based on elapsed time since last refill
**And** `try_consume(n)` returns true and decrements tokens if sufficient tokens are available
**And** `try_consume(n)` returns false without modification if insufficient tokens are available
**And** tokens never exceed the burst size (max capacity)
**And** token bucket decisions execute in <1μs (NFR7)
**And** a `ThrottleManager` struct owns all token buckets keyed by throttle key string
**And** the manager supports creating, removing, and updating bucket configurations at runtime
**And** unit tests verify: refill timing accuracy, burst cap, consume/reject behavior, rate accuracy over 1-second windows

### Story 4.2: Throttle-Aware Scheduling

As a platform engineer,
I want the scheduler to skip throttled keys and deliver only ready messages,
So that consumers never receive messages they cannot process due to rate limits.

**Acceptance Criteria:**

**Given** messages have been assigned throttle keys via Lua on_enqueue (from Epic 3)
**When** the DRR scheduler evaluates a fairness key for delivery
**Then** the scheduler checks all throttle keys associated with the next message
**And** if ANY throttle key's bucket is exhausted, that message is skipped (FR15)
**And** the scheduler moves to the next fairness key in the DRR round
**And** a message can have multiple simultaneous throttle keys (FR14) — ALL must have available tokens
**And** hierarchical throttling works: a message with keys `["provider:aws", "region:us-east-1"]` is throttled if either bucket is empty (FR17)
**And** token buckets are refilled in the scheduler loop before each DRR round
**And** skipped keys remain in the active set (they still have pending messages)
**And** the O(n²) message scan in `drr_deliver_queue` is replaced with a per-fairness-key in-memory pending message index, eliminating quadratic scanning during delivery (technical debt from Epic 2, deferred from Epic 3)
**And** the previously `#[ignore]`d 10k fairness test runs in CI after the scan optimization
**And** an integration test creates a queue with Lua assigning throttle keys, sets a low rate limit, enqueues rapidly, and verifies consumers receive messages at the throttled rate

### Story 4.3: Runtime Throttle Rate Management

As an operator,
I want to set and update throttle rates at runtime without restarting the broker,
So that I can respond to production conditions by adjusting rate limits.

**Acceptance Criteria:**

**Given** the broker is running with active throttle keys
**When** an operator sets a throttle rate via the admin API (using `SetConfig` with a throttle-specific key convention)
**Then** the ThrottleManager creates or updates the token bucket for that throttle key
**And** the new rate takes effect on the next scheduler refill cycle
**And** no restart is required (FR16)
**And** removing a throttle rate config removes the token bucket (messages with that key become unthrottled)
**And** throttle rates are persisted in the `state` CF so they survive restart
**And** Lua scripts can read throttle config via `fila.get()` for dynamic decisions
**And** an integration test sets a throttle rate, verifies it is enforced, updates the rate, and verifies the new rate takes effect

---

## Epic 5: Operator Experience (Admin, Config & CLI)

Operators manage runtime configuration via the gRPC API, redrive DLQ messages, inspect queue stats and per-key metrics, and perform all admin tasks through `fila-cli`. Config changes take effect without restart, enabling live operational adjustments.

### Story 5.1: Configuration Listing & Operator Visibility

As an operator,
I want to list and inspect all runtime configuration entries,
So that I can understand the current broker configuration state without guessing key names.

**Note:** Core `SetConfig`/`GetConfig` RPCs were implemented in Epic 4, Story 4.3 (including persistence to state CF, Lua `fila.get()` integration, throttle-prefix side effects, and crash recovery). This story covers the remaining operator visibility gaps.

**Acceptance Criteria:**

**Given** the broker has runtime configuration entries (set via `SetConfig` in Story 4.3)
**When** an operator calls `ListConfig` RPC with an optional prefix filter
**Then** all matching key-value pairs from the `state` CF are returned
**And** if no prefix is specified, all config entries are returned
**And** if a prefix is specified (e.g., `throttle.`), only matching entries are returned
**And** the response includes the total count of matching entries
**And** calling `ListConfig` with no entries returns an empty list (not an error)
**And** an integration test sets multiple config values (throttle and non-throttle), lists all, lists by prefix, and verifies correct filtering
**And** an integration test verifies non-throttle config keys are readable by Lua scripts via `fila.get()` (end-to-end: SetConfig → Lua fila.get → verify value)

### Story 5.2: Queue Stats & Inspection

As an operator,
I want to view queue stats including depth, per-key throughput, and scheduling state,
So that I can understand the health and behavior of my message infrastructure.

**Acceptance Criteria:**

**Given** a queue has messages and active consumers
**When** an operator calls `GetStats` RPC for that queue
**Then** the response includes: total message count (queue depth), number of in-flight leases, number of active fairness keys
**And** per-fairness-key stats are included: pending message count, delivered count, current deficit
**And** throttle state is included: per-throttle-key token count, hit rate, pass rate
**And** DRR scheduling state is included: current round position, quantum, active keys count
**And** calling GetStats on a non-existent queue returns `NOT_FOUND` status
**And** an integration test enqueues messages across multiple fairness keys, leases some, and verifies GetStats returns accurate counts

### Story 5.3: DLQ Redrive

As an operator,
I want to move messages from a dead-letter queue back to the source queue,
So that I can reprocess messages after fixing the underlying issue.

**Acceptance Criteria:**

**Given** a DLQ contains failed messages
**When** an operator calls `Redrive` RPC with the DLQ name and an optional count limit
**Then** messages are moved from the DLQ back to the source queue
**And** each message's attempt count is reset to 0
**And** messages are moved atomically via WriteBatch (per message)
**And** if a count is specified, only that many messages are redriven (oldest first)
**And** if no count is specified, all DLQ messages are redriven
**And** the response includes the number of messages redriven
**And** calling Redrive on a non-DLQ queue returns `INVALID_ARGUMENT` status
**And** an integration test dead-letters messages, redrives them, and verifies they are available for lease from the source queue
**And** redrive only moves pending (non-leased) messages — currently leased DLQ messages are not moved to avoid confusing active consumers

### Story 5.4: fila-cli

As an operator,
I want a command-line interface for all admin operations,
So that I can manage the broker from the terminal without writing code.

**Acceptance Criteria:**

**Given** the broker is running
**When** an operator uses `fila-cli` (binary name: `fila`)
**Then** the following commands are available:
**And** `fila queue create <name> [--on-enqueue <script>] [--on-failure <script>] [--visibility-timeout <ms>]` creates a queue
**And** `fila queue delete <name>` deletes a queue
**And** `fila queue list` lists all queues with basic stats
**And** `fila queue inspect <name>` shows detailed queue stats (same as GetStats)
**And** `fila config set <key> <value>` sets a runtime config value
**And** `fila config get <key>` reads a runtime config value
**And** `fila stats <queue>` shows queue statistics
**And** `fila redrive <dlq-name> [--count <n>]` redrives DLQ messages
**And** all commands connect to the broker via gRPC (default `localhost:5555`, configurable via `--addr` flag)
**And** error messages are human-friendly (translated from gRPC status codes to actionable messages, e.g., `NOT_FOUND` → `Error: queue "foo" does not exist`, `INVALID_ARGUMENT` → `Error: "bar" is not a dead-letter queue`)
**And** successful operations produce concise confirmation output (e.g., `Created queue "orders"`, `Redrived 42 messages from "orders.dlq" to "orders"`)
**And** `fila queue list` and `fila queue inspect` output is formatted as aligned tables for terminal readability
**And** `fila --help` shows usage for all commands with brief descriptions and examples
**And** each subcommand supports `--help` with detailed usage (e.g., `fila queue create --help`)
**And** the CLI binary compiles as `fila` (distinct from `fila-server`)

---

## Epic 6: Observability & Diagnostics

Full observability via OpenTelemetry metrics and distributed tracing. Per-fairness-key throughput, per-throttle-key hit rates, DRR scheduling rounds, Lua execution histograms. Operators can answer "why was key X delayed?" from broker metrics alone.

### Story 6.1: OTel Infrastructure & Core Metrics

As an operator,
I want the broker to export OpenTelemetry-compatible metrics and traces,
So that I can monitor broker health in Prometheus, Grafana, or Datadog.

**Acceptance Criteria:**

**Given** the broker is configured with an OTLP endpoint
**When** the broker processes messages
**Then** metrics are exported via OTLP protocol to the configured endpoint (NFR18)
**And** core counters are emitted: `fila.messages.enqueued`, `fila.messages.leased`, `fila.messages.acked`, `fila.messages.nacked`
**And** gauge metrics are emitted: `fila.queue.depth` (per queue), `fila.leases.active` (per queue)
**And** tracing spans are created for all RPC operations: Enqueue, Lease, Ack, Nack (FR40)
**And** spans include structured fields: `queue_id`, `msg_id`, `fairness_key`
**And** trace context is propagated via gRPC metadata headers
**And** the telemetry setup uses `tracing` + `tracing-opentelemetry` + `opentelemetry-otlp` crates
**And** OTLP endpoint and service name are configurable via `fila.toml` `[telemetry]` section
**And** metrics export interval is configurable (default 10 seconds)
**And** log levels follow the convention: ERROR for unrecoverable, WARN for circuit breaker, INFO for lifecycle, DEBUG for per-message, TRACE for scheduler internals
**And** message payloads and potentially sensitive headers are NEVER logged
**And** OTel crate versions are verified for compatibility before implementation begins (`opentelemetry` + `opentelemetry-otlp` + `tracing-opentelemetry` — pin versions that work together)
**And** a test-harness for asserting on emitted metrics is established (in-memory exporter or mock collector) so Stories 6.2 and 6.3 can reuse it
**And** the `[telemetry]` section in `BrokerConfig` is implemented (OTLP endpoint, service name, metrics interval) if not already present

### Story 6.2: Scheduler & Fairness Metrics

As an operator,
I want per-fairness-key throughput and DRR scheduling metrics,
So that I can verify fair share allocation and identify scheduling anomalies.

**Acceptance Criteria:**

**Given** the DRR scheduler is delivering messages across multiple fairness keys
**When** metrics are exported
**Then** `fila.fairness.throughput` counter is emitted per fairness key with labels `queue_id` and `fairness_key` (FR36)
**And** `fila.fairness.fair_share_ratio` gauge shows each key's actual throughput divided by its fair share target
**And** `fila.scheduler.drr.rounds` counter tracks completed DRR rounds (FR38)
**And** `fila.scheduler.drr.active_keys` gauge shows the current number of active fairness keys
**And** `fila.scheduler.drr.keys_processed` counter tracks keys processed per round
**And** metrics are labeled to enable "why was key X delayed?" queries (FR41)
**And** an integration test verifies that per-key throughput metrics are emitted and directionally correct

### Story 6.3: Throttle, Lua & Diagnostic Metrics

As an operator,
I want per-throttle-key hit rates and Lua execution metrics,
So that I can diagnose throttling behavior and script performance.

**Acceptance Criteria:**

**Given** throttling and Lua hooks are active
**When** metrics are exported
**Then** `fila.throttle.decisions` counter is emitted per throttle key with labels `throttle_key` and `result` (hit/pass) (FR37)
**And** `fila.throttle.tokens_remaining` gauge shows current token count per throttle key
**And** `fila.lua.execution_duration_us` histogram tracks Lua hook execution time with labels `queue_id` and `hook` (on_enqueue/on_failure) (FR39)
**And** `fila.lua.errors` counter tracks Lua execution errors per queue
**And** `fila.lua.circuit_breaker.activations` counter tracks circuit breaker activations per queue
**And** all metric names follow the `fila.*` naming convention with `snake_case` labels
**And** structured JSON logging is used in production mode; pretty-print in development mode
**And** the combination of fairness, throttle, and scheduling metrics enables an operator to answer "why was key X delayed?" from metrics alone without reading code or logs (FR41)

---

## Epic 7: Rust Client SDK

Developers integrate Fila into Rust applications using an idiomatic client SDK built as a thin wrapper over tonic gRPC client. This SDK also serves as the client for the blackbox e2e test suite in Epic 8.

**FRs covered:** FR46, FR48 (partial — Rust only)

**Prerequisite:** Epic 6 complete.

### Story 7.1: Rust Client SDK

As a Rust developer,
I want an idiomatic Rust client SDK for Fila,
So that I can integrate message enqueue, lease, ack, and nack into my Rust application.

**Acceptance Criteria:**

**Given** the Fila proto definitions are available
**When** the Rust SDK is built
**Then** the SDK is published as a crate (e.g., `fila-client`) using tonic gRPC client
**And** the API provides: `client.enqueue(queue, headers, payload) -> Result<MessageId>`
**And** the API provides: `client.lease(queue) -> Result<Stream<LeaseMessage>>` for streaming consumption
**And** the API provides: `client.ack(queue, msg_id) -> Result<()>`
**And** the API provides: `client.nack(queue, msg_id, error) -> Result<()>`
**And** connection configuration accepts address, optional TLS config, and timeouts
**And** the SDK follows Rust conventions: `Result<T, E>` errors, async/await, `Send + Sync` types
**And** integration tests verify all four operations against a running broker

---

## Epic 8: E2E Tests & Scheduler Refactoring

Build a true blackbox e2e test suite using the Rust SDK (for producer/consumer operations) and the `fila` CLI binary (for admin operations), then use it as a safety net to decompose the monolithic `scheduler.rs` (6,800+ lines) into focused submodules. The observability layer from Epic 6 provides runtime verification during restructuring.

**Identified in:** Epic 4 Retrospective (2026-02-13). **Restructured by:** Sprint Change Proposal (2026-02-17) — Rust SDK pulled forward as prerequisite for meaningful e2e tests.

**Prerequisite:** Epic 7 (Rust Client SDK) complete. Epic 6 complete (metrics provide runtime verification during refactoring).

### Story 8.1: Blackbox End-to-End Test Suite

As a developer,
I want a comprehensive blackbox e2e test suite that exercises the full system through the SDK and CLI,
So that I can refactor scheduler internals safely with behavioral regression coverage.

**Acceptance Criteria:**

**Given** a running `fila-server` instance started as a subprocess in the test harness
**When** the e2e test suite runs
**Then** producer/consumer operations use the `fila-client` Rust SDK (no raw gRPC calls)
**And** admin operations use the `fila` CLI binary executed as a subprocess (no raw gRPC calls)
**And** the test crate (`fila-e2e`) depends only on `fila-client` and `fila-proto` — no internal server or core types
**And** the following flows are tested end-to-end:
- Enqueue → Lease → Ack lifecycle (basic message flow via SDK)
- Enqueue → Lease → Nack → re-Lease (retry with attempt count increment via SDK)
- Lua `on_enqueue` assigns fairness key, weight, and throttle keys from headers
- Lua `on_failure` decides retry vs DLQ based on attempt count
- DLQ flow: nack to exhaustion → message in DLQ → Redrive via CLI → re-Lease from source queue
- DRR fairness: multi-key weighted delivery (higher-weight key gets proportionally more)
- Throttle: rate-limited key skipped, unthrottled keys served immediately
- Config: `fila config set` → `fila config get` → ListConfig with prefix filter → Lua `fila.get()` reads value
- Queue management: `fila queue create` → `fila queue list` → `fila queue inspect` → `fila queue delete`
- Crash recovery: enqueue messages → kill server → restart → verify all messages available for lease
- Visibility timeout: lease message → wait for expiry → message available for re-lease
**And** tests are independent and can run in parallel (separate ports, separate temp data dirs)
**And** a shared test helper starts/stops `fila-server` instances and creates SDK clients
**And** all existing unit/integration tests continue to pass alongside the new e2e tests

### Story 8.2: Scheduler Decomposition

As a developer,
I want `scheduler.rs` decomposed into focused submodules,
So that each module is under 500 lines and has a clear single responsibility.

**Acceptance Criteria:**

**Given** the blackbox e2e test suite from Story 8.1 is passing
**When** the scheduler is decomposed
**Then** `scheduler.rs` is split into submodules under `broker/scheduler/`:
- `mod.rs` — Scheduler struct, event loop, command dispatch
- `delivery.rs` — DRR delivery logic, `drr_deliver_queue`, consumer management
- `handlers.rs` — Command handlers: enqueue, ack, nack, config, stats, redrive, list_queues, list_config
- `recovery.rs` — Startup recovery, lease expiry scanning, state rebuild
- `leasing.rs` — Lease creation, expiry tracking, visibility timeout
- `metrics_recording.rs` — `record_gauges`, fairness delivery tracking, all metric recording logic
**And** no submodule exceeds ~500 lines (architecture guideline)
**And** all existing tests (unit, integration, metric, e2e) pass with zero changes to test assertions
**And** the public API of `Scheduler` remains unchanged (internal restructure only)
**And** `cargo clippy` and `cargo fmt` pass cleanly

### Story 8.3: Cleanup & Deferred Items

As a developer,
I want accumulated structural debt and deferred items addressed,
So that the codebase is clean for the remaining SDK epics.

**Acceptance Criteria:**

**Given** the scheduler decomposition from Story 8.2 is complete
**When** cleanup is performed
**Then** the deferred tonic interceptor for trace context extraction is wired (Story 6.1 Task 7 completion)
**And** incoming gRPC requests with W3C `traceparent` metadata have their trace context propagated to scheduler spans
**And** any dead code, unused imports, or stale comments identified during decomposition are removed
**And** all tests pass, clippy clean, fmt clean

---

## Epic 9: Client SDKs

Developers integrate Fila into applications using idiomatic client SDKs in five additional languages: Go, Python, JavaScript/Node.js, Ruby, and Java. All SDKs support the full hot-path API: enqueue, streaming consume, ack, and nack.

**FRs covered:** FR42, FR43, FR44, FR45, FR47, FR48 (partial — 5 languages)

### Story 9.1: Go Client SDK

As a Go developer,
I want an idiomatic Go client SDK for Fila,
So that I can integrate Fila into my Go services with minimal effort.

**Acceptance Criteria:**

**Given** the Fila proto definitions are available
**When** the Go SDK is built
**Then** the SDK lives in a separate repository (`fila-go`)
**And** proto files are copied into the repo and generated Go code is committed (no submodules, no Buf)
**And** gRPC stubs are generated from proto files using `protoc-gen-go-grpc`
**And** an ergonomic Go wrapper provides: `client.Enqueue(ctx, queue, headers, payload) (string, error)`
**And** `client.Consume(ctx, queue) (<-chan *ConsumeMessage, error)` returns a channel for streaming consumption
**And** `ConsumeMessage` includes: `ID`, `Headers`, `Payload`, `FairnessKey`, `AttemptCount`, `Queue`
**And** `client.Ack(ctx, queue, msgID) error` and `client.Nack(ctx, queue, msgID, errMsg) error`
**And** all methods accept `context.Context` for cancellation and deadlines
**And** per-operation error types are defined (e.g., `ErrQueueNotFound`, `ErrMessageNotFound`) checkable via `errors.Is`
**And** the SDK follows Go conventions: exported types, error returns, godoc comments
**And** integration tests verify all four operations against a running `fila-server` binary
**And** GitHub Actions CI runs lint (`golangci-lint`), test, and build on every PR
**And** a README with usage examples is included (FR55)

### Story 9.2: Python Client SDK

As a Python developer,
I want an idiomatic Python client SDK for Fila,
So that I can integrate Fila into my Python applications.

**Acceptance Criteria:**

**Given** the Fila proto definitions are available
**When** the Python SDK is built
**Then** the SDK lives in a separate repository (`fila-python`)
**And** proto files are copied into the repo and generated Python code is committed (no submodules, no Buf)
**And** gRPC stubs are generated from proto files using `grpcio-tools`
**And** an ergonomic Python wrapper provides: `client.enqueue(queue, headers, payload) -> str`
**And** `client.consume(queue)` returns an async iterator of `ConsumeMessage` objects
**And** `ConsumeMessage` includes: `id`, `headers`, `payload`, `fairness_key`, `attempt_count`, `queue`
**And** `client.ack(queue, msg_id)` and `client.nack(queue, msg_id, error)`
**And** both sync and async interfaces are supported
**And** per-operation exception classes are defined (e.g., `QueueNotFoundError`, `MessageNotFoundError`)
**And** the SDK follows Python conventions: snake_case methods, type hints, docstrings
**And** the package is installable via `pip install fila-client`
**And** integration tests verify all four operations against a running `fila-server` binary
**And** GitHub Actions CI runs lint, type check, and test on every PR
**And** a README with usage examples is included (FR55)

### Story 9.3: JavaScript/Node.js Client SDK

As a JavaScript/Node.js developer,
I want an idiomatic JS/TS client SDK for Fila,
So that I can integrate Fila into my Node.js services.

**Acceptance Criteria:**

**Given** the Fila proto definitions are available
**When** the JS SDK is built
**Then** the SDK lives in a separate repository (`fila-js`)
**And** proto files are copied into the repo and generated TypeScript code is committed (no submodules, no Buf)
**And** gRPC stubs are generated using `@grpc/proto-loader` or `grpc-js`
**And** the SDK provides: `client.enqueue(queue, headers, payload): Promise<string>`
**And** `client.consume(queue)` returns an async iterable of `ConsumeMessage` objects
**And** `ConsumeMessage` includes: `id`, `headers`, `payload`, `fairnessKey`, `attemptCount`, `queue`
**And** `client.ack(queue, msgId): Promise<void>` and `client.nack(queue, msgId, error): Promise<void>`
**And** per-operation error classes are defined (e.g., `QueueNotFoundError`, `MessageNotFoundError`)
**And** TypeScript type definitions are included
**And** the SDK follows JS/TS conventions: Promise-based API, camelCase methods
**And** the package is installable via `npm install @fila/client`
**And** integration tests verify all four operations against a running `fila-server` binary
**And** GitHub Actions CI runs lint, type check, and test on every PR
**And** a README with usage examples is included (FR55)

### Story 9.4: Ruby Client SDK

As a Ruby developer,
I want an idiomatic Ruby client SDK for Fila,
So that I can integrate Fila into my Ruby applications.

**Acceptance Criteria:**

**Given** the Fila proto definitions are available
**When** the Ruby SDK is built
**Then** the SDK lives in a separate repository (`fila-ruby`)
**And** proto files are copied into the repo and generated Ruby code is committed (no submodules, no Buf)
**And** gRPC stubs are generated using `grpc-tools` gem
**And** the SDK provides: `client.enqueue(queue:, headers:, payload:)` returning the message ID
**And** `client.consume(queue:)` yields `ConsumeMessage` objects via block or returns an Enumerator
**And** `ConsumeMessage` includes: `id`, `headers`, `payload`, `fairness_key`, `attempt_count`, `queue`
**And** `client.ack(queue:, msg_id:)` and `client.nack(queue:, msg_id:, error:)`
**And** per-operation error classes are defined (e.g., `Fila::QueueNotFoundError`, `Fila::MessageNotFoundError`)
**And** the SDK follows Ruby conventions: keyword arguments, snake_case methods, block patterns
**And** the gem is installable via `gem install fila-client`
**And** integration tests verify all four operations against a running `fila-server` binary
**And** GitHub Actions CI runs lint (`rubocop`), and test on every PR
**And** a README with usage examples is included (FR55)

### Story 9.5: Java Client SDK

As a Java developer,
I want an idiomatic Java client SDK for Fila,
So that I can integrate Fila into my Java applications.

**Acceptance Criteria:**

**Given** the Fila proto definitions are available
**When** the Java SDK is built
**Then** the SDK lives in a separate repository (`fila-java`)
**And** proto files are copied into the repo and generated Java code is committed (no submodules, no Buf)
**And** gRPC stubs are generated using `protoc-gen-grpc-java`
**And** the SDK provides a `FilaClient` class with builder pattern for configuration
**And** `client.enqueue(queue, headers, payload)` returns the message ID
**And** `client.consume(queue, observer)` accepts a `StreamObserver<ConsumeMessage>` for streaming consumption
**And** `ConsumeMessage` includes: `id`, `headers`, `payload`, `fairnessKey`, `attemptCount`, `queue`
**And** `client.ack(queue, msgId)` and `client.nack(queue, msgId, error)` for acknowledgment
**And** per-operation exception classes are defined (e.g., `QueueNotFoundException`, `MessageNotFoundException`)
**And** the SDK follows Java conventions: Builder pattern, checked exceptions, Javadoc
**And** the artifact is publishable to Maven Central
**And** integration tests verify all four operations against a running `fila-server` binary
**And** GitHub Actions CI runs build, test, and lint on every PR
**And** a README with usage examples is included (FR55)

---

## Epic 10: Distribution & Documentation

Users install Fila via Docker, `cargo install`, or `curl | bash` shell script. Bleeding-edge releases from main enable SDK CIs and early adopters. Comprehensive documentation — tutorials, API reference, Lua hook examples, and `llms.txt` — enables onboarding in under 10 minutes. Documentation is the primary adoption driver.

**FRs covered:** FR50, FR51, FR52, FR54, FR55, FR56, FR57, FR58, FR59, FR60

**Reshaped in:** Epic 9 Retrospective (2026-02-21). Release pipeline front-loaded as Story 10.1 to unblock SDK CIs, binary distribution, and documentation. SDK package publishing added as Story 10.2.

### Story 10.1: Server & CLI Release Pipeline + Docker Image

As a developer and user,
I want bleeding-edge releases of fila-server and fila CLI from every push to main, plus a Docker image,
So that SDK CIs can download pre-built binaries, and users can try Fila with a single Docker command.

**Acceptance Criteria:**

**Given** a commit is pushed to main
**When** the bleeding-edge release workflow runs
**Then** cross-compiled binaries are built for: linux-amd64, linux-arm64, darwin-amd64, darwin-arm64
**And** both `fila-server` and `fila` (CLI) binaries are included for each platform
**And** a GitHub Release is created, tagged with the short commit hash (e.g., `dev-abc1234`), marked as pre-release
**And** binaries are uploaded as GitHub Release assets with checksums
**And** a `latest` pre-release tag is updated to always point to the most recent build
**And** SDK CI workflows can download the latest pre-built binary instead of building from source

**Given** the release workflow produces binaries
**When** a Docker image is created
**Then** a multi-stage Dockerfile builds from `rust:latest` and produces a `debian:bookworm-slim` runtime image
**And** the image contains both `fila-server` and `fila` (CLI) binaries
**And** the image exposes port 5555
**And** `docker run ghcr.io/faisca/fila` starts the broker with default configuration
**And** the image is published to `ghcr.io/faisca/fila` with `dev` and commit-hash tags
**And** data directory is configurable via volume mount (`-v /data:/var/lib/fila`)
**And** environment variables can override config (`-e FILA_SERVER__LISTEN_ADDR=0.0.0.0:6666`)
**And** the image size is minimized (no build tools in runtime layer)

### Story 10.2: SDK Package Publishing

As a developer,
I want all Fila SDKs published to their respective package registries with bleeding-edge versions,
So that users can install them via standard package managers and SDK CIs use real published artifacts.

**Acceptance Criteria:**

**Given** the SDK repositories exist (fila-go, fila-python, fila-js, fila-ruby, fila-java, fila-sdk)
**When** a publish workflow is configured for each
**Then** the Rust SDK (`fila-sdk`) is published to crates.io
**And** the Go SDK (`fila-go`) is available via `go get` (Go modules use git tags, no registry publish needed)
**And** the Python SDK (`fila-python`) is published to PyPI as `fila-python`
**And** the JS SDK (`fila-js`) is published to npm as `@fila/client`
**And** the Ruby SDK (`fila-ruby`) is published to RubyGems as `fila-client`
**And** the Java SDK (`fila-java`) is published to Maven Central as `dev.faisca:fila-client`
**And** each SDK uses dev/pre-release versioning appropriate to its ecosystem
**And** each SDK's CI workflow is updated to download the fila-server pre-built binary from Story 10.1 (replacing build-from-source)
**And** integration tests in all SDK CIs actually execute against the downloaded binary (no silent skips)

### Story 10.3: Binary Distribution & Installation

As a user,
I want to install Fila via cargo or a shell script,
So that I can run it natively on my machine without Docker.

**Acceptance Criteria:**

**Given** the Fila crates are published
**When** a user installs via cargo
**Then** `cargo install fila-server` installs the broker binary
**And** `cargo install fila-cli` installs the CLI binary as `fila`

**Given** release binaries are available from Story 10.1
**When** a user installs via shell script
**Then** `curl -fsSL https://get.fila.dev | bash` downloads and installs the correct binary for the platform
**And** the script detects OS (linux/darwin) and architecture (amd64/arm64)
**And** binaries are placed in a standard location (`/usr/local/bin` or `~/.local/bin`)

**Given** a semver version is tagged
**When** the release workflow runs
**Then** a stable GitHub Release is created (not pre-release) with the version tag
**And** checksums are generated for verification

### Story 10.4: Core Documentation & API Reference

As a user evaluating Fila,
I want comprehensive documentation that explains concepts, architecture, and API,
So that I can understand and adopt Fila quickly.

**Acceptance Criteria:**

**Given** Fila is ready for public use
**When** documentation is created
**Then** `README.md` includes: project overview, problem statement, quickstart (Docker + CLI), key concepts (fairness, throttling, Lua hooks), and links to detailed docs
**And** API reference documentation is generated from `.proto` files (FR57)
**And** a `llms.txt` file is structured for LLM agent consumption with project context, API surface, and usage patterns (FR58)
**And** documentation covers all core concepts: message lifecycle, fairness groups, DRR scheduling, token bucket throttling, Lua hooks, DLQ, runtime config (FR54)
**And** documentation uses a docs-as-product approach — the primary onboarding experience (FR60)
**And** download to fair-scheduling demo is achievable in under 10 minutes following the docs (NFR14)

### Story 10.5: Tutorials, Examples & Lua Patterns

As a developer adopting Fila,
I want guided tutorials, working code examples, and copy-paste Lua patterns,
So that I can implement common use cases without starting from scratch.

**Acceptance Criteria:**

**Given** the documentation exists
**When** tutorials and examples are created
**Then** guided tutorials cover: multi-tenant fair scheduling, per-provider throttling, exponential backoff retry (FR56)
**And** working `examples/fair_scheduling.rs` demonstrates multi-tenant fairness with Lua hooks
**And** working `examples/throttling.rs` demonstrates per-key rate limiting
**And** each SDK has a working code example showing enqueue → consume → ack flow (FR55)
**And** copy-paste Lua hook examples are provided for common patterns: tenant fairness, provider throttling, exponential backoff, header-based routing (FR59)
**And** each example is tested in CI to prevent documentation rot

---

## Epic 11: Release Activation

Verify that the release infrastructure from Epic 10 actually works end-to-end: bleeding-edge releases fire on merge, SDK CIs download pre-built binaries, and packages are published to all registries. This is operational verification, not new engineering.

**Added in:** Epic 10 Retrospective (2026-03-01). Lucas flagged that Epic 10 built pipelines but never verified them. Operational tasks tracked here instead of memory files.

### Story 11.1: Verify Release Pipeline End-to-End

As a maintainer,
I want to verify that the bleeding-edge release pipeline actually works,
So that SDK CIs can download pre-built binaries and users can pull Docker images.

**Acceptance Criteria:**

**Given** Epic 10 code is merged to main
**When** the bleeding-edge workflow triggers
**Then** binaries are produced for all 4 platforms (linux-amd64, linux-arm64, darwin-amd64, darwin-arm64)
**And** a GitHub Release tagged `dev-{sha}` is created with all assets
**And** the rolling `latest` pre-release tag points to the newest build
**And** the Docker image is published to `ghcr.io/faisca/fila` with `dev` and `dev-{sha}` tags
**And** all 5 external SDK CIs (Go, Python, JS, Ruby, Java) successfully download the binary via `gh release download latest`
**And** integration tests in all 5 SDK CIs actually execute and pass against the downloaded binary

### Story 11.2: Package Registry Publishing

As a maintainer,
I want all SDKs published to their respective package registries,
So that users can install Fila and its SDKs via standard package managers.

**Acceptance Criteria:**

**Given** the publish workflows exist in each SDK repo
**When** secrets are configured and a publish is triggered
**Then** `fila-proto` and `fila-sdk` are published to crates.io
**And** `fila-go` has a dev version tag accessible via `go get`
**And** `fila-python` is published to PyPI (OIDC trusted publisher configured)
**And** `@fila/client` is published to npm
**And** `fila-client` gem is published to RubyGems
**And** `dev.faisca:fila-client` is published to Maven Central
**And** `fila-server` and `fila-cli` are installable via `cargo install`
**And** DNS for `get.fila.dev` is configured (or install.sh updated to use raw GitHub URL)
**And** `curl -fsSL <install-url> | bash` successfully installs the correct binary

---

## Future Work

- Authentication and authorization (API keys, mTLS)
- Distributed clustering (multi-node horizontal scalability)
- Consumer groups (broker-managed consumer coordination)
- Management GUI (web interface for monitoring)
