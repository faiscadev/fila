---
stepsCompleted:
  - step-01-validate-prerequisites
  - step-02-design-epics
  - step-03-create-stories
  - step-04-final-validation
status: complete
inputDocuments:
  - '_bmad-output/planning-artifacts/research/technical-closing-throughput-gap-kafka-parity-research-2026-03-24.md'
  - '_bmad-output/planning-artifacts/performance-optimization-epics.md'
---

# Fila - Epic Breakdown

## Overview

This document provides the epic and story breakdown for the batch pipeline performance work, closing the throughput gap to Kafka parity (400K+ msg/s from current 22K). Based on the closing-throughput-gap research (2026-03-24). Fila is pre-alpha — no backward compatibility concerns, old paths are replaced freely.

## Requirements Inventory

### Functional Requirements

FR-B1: Unified API surface — one code path per operation, batch is the only path (single = batch of 1), no "batch" prefixes/suffixes
FR-B2: All 6 SDKs (Rust + 5 external) use the unified API surface
FR-B3: Scheduler `Enqueue` command accepts `Vec<Message>` with a single reply channel (replaces single-message variant)
FR-B4: gRPC handlers submit messages through the batch-capable `SchedulerCommand::Enqueue`, eliminating sequential round-trips
FR-B5: `StreamEnqueue` carries `repeated` messages per stream write for batch-within-stream transport
FR-B6: Rust SDK provides adaptive client-side accumulator that auto-tunes flush timing and batch size for optimal throughput
FR-B7: SDK flushes via `StreamEnqueue` with pipelined acks; users can override with explicit config

### NonFunctional Requirements

NFR-B1: Latency must not regress for light load (single producer) — p50 <= 1ms
NFR-B2: Per-queue message ordering within a batch must be preserved
NFR-B3: Partial failure handling — one bad message in a batch must not fail the entire batch
NFR-B4: Profile after Tier 0 before proceeding to Tier 1 — no speculative optimization
NFR-B5: SDK adaptive batching should beat Kafka out of the box without manual tuning

### Additional Requirements

- Cluster mode compatibility — batch `Enqueue` must work with Raft clustering
- Benchmark-driven validation — run suite before/after each change
- Stories 30.6-30.8 are conditional on profiling results from Story 30.5
- No backward compatibility concern — Fila is pre-alpha, replace old paths freely
- All existing 508+ tests must pass after every story
- CI bench-regression workflow must detect improvements and catch regressions
- External SDK updates use parallel agents (one per SDK, validated in Epic 26)

### FR Coverage Map

FR-B1: Epic 30, Story 30.1 — Proto, server, Rust SDK unification
FR-B2: Epic 30, Story 30.2 — External SDK unification (Go, Python, JS, Ruby, Java)
FR-B3: Epic 30, Story 30.3 — SchedulerCommand::Enqueue refactored to Vec<Message>
FR-B4: Epic 30, Story 30.4 — Unified enqueue handler throughput fix
FR-B5: Epic 30, Story 30.6 — StreamEnqueue batch-within-stream (conditional on 30.5)
FR-B6: Epic 30, Story 30.7 — SDK adaptive accumulator (conditional on 30.5)
FR-B7: Epic 30, Story 30.7 — Pipelined acks + user overrides (conditional on 30.5)

## Epic List

### Epic 30: Batch Pipeline — Scheduler Batching & Batch-Aware Protocol
Close the throughput gap to Kafka parity by propagating batches end-to-end from gRPC handlers through to the scheduler. First, unify the API surface so batch is the only path (stories 30.1-30.2). Then fix the handler-to-scheduler bridge — the measured root cause of the 19x gap (stories 30.3-30.4). A profile checkpoint (story 30.5) validates and identifies the next bottleneck. Tier 1 (stories 30.6-30.7) adds batch-within-stream gRPC and adaptive SDK batching — conditional on profiling results. A final profile checkpoint (story 30.8) quantifies the full result.
**FRs covered:** FR-B1, FR-B2, FR-B3, FR-B4, FR-B5, FR-B6, FR-B7
**NFRs addressed:** NFR-B1, NFR-B2, NFR-B3, NFR-B4, NFR-B5

---

## Epic 30: Batch Pipeline — Scheduler Batching & Batch-Aware Protocol

### Story 30.1: API Surface Unification — Proto, Server, Rust SDK

As a developer,
I want all RPCs and scheduler commands to treat batches as the only path (single = batch of 1),
So that the codebase has one code path per operation, no "batch" prefixes/suffixes, and a clean surface for throughput optimization.

**Acceptance Criteria:**

**Given** the proto defines separate `Enqueue` (unary, 1 msg) and `BatchEnqueue` (unary, N msgs) RPCs
**When** the proto is unified
**Then** one `Enqueue` RPC accepts `repeated EnqueueRequest messages` and returns `repeated EnqueueResult results`
**And** `BatchEnqueue` RPC is removed
**And** `BatchEnqueueRequest`, `BatchEnqueueResponse`, `BatchEnqueueResult` message types are removed or renamed (no "Batch" prefix)

**Given** `StreamEnqueue` sends one message per stream write
**When** the proto is unified
**Then** `StreamEnqueueRequest` contains `repeated EnqueueRequest messages`

**Given** service.rs has four enqueue functions: `enqueue()`, `batch_enqueue()`, `enqueue_single()`, `enqueue_single_standalone()`
**When** the handlers are unified
**Then** there is one enqueue handler that processes `Vec<Message>`

**Given** `SchedulerCommand::Ack` and `SchedulerCommand::Nack` take a single message ID
**When** they are unified
**Then** `Ack` takes `Vec<(queue_id, msg_id)>` and returns `Vec<Result<(), AckError>>`
**And** `Nack` takes `Vec<(queue_id, msg_id, error)>` and returns `Vec<Result<(), NackError>>`

**Given** consume delivery has both single-message and batched paths
**When** delivery is unified
**Then** consume always delivers via `repeated Message` (single message = batch of 1)

**Given** the Rust SDK has separate `enqueue()` and `batch_enqueue()` methods and a `BatchMode` enum
**When** the SDK is unified
**Then** `enqueue()` accepts one or more messages
**And** `batch_enqueue()` is removed
**And** `BatchMode` is renamed to remove "batch" terminology (e.g. `AccumulatorMode` or folded into client config)

**And** all existing tests are updated to use the unified API
**And** all tests pass
**And** no "batch" prefix/suffix remains in public API names (proto, SDK, CLI) — only internal implementation details where semantically accurate

### Story 30.2: External SDK Unification — Go, Python, JS, Ruby, Java

As a developer,
I want all 5 external SDKs to use the unified API surface from Story 30.1,
So that every SDK has one code path per operation with no "batch" naming.

**Acceptance Criteria:**

**Given** the proto changes from Story 30.1 (unified `Enqueue` with `repeated`, `BatchEnqueue` removed)
**When** each external SDK is updated
**Then** `batch_enqueue()` is removed, `enqueue()` accepts one or more messages
**And** consume uses the unified `repeated Message` delivery
**And** ack/nack accept one or more message IDs
**And** no "batch" prefix/suffix remains in the public API

**Given** the 5 SDKs are independent repos
**When** the updates are made
**Then** one agent per SDK runs in parallel (Go, Python, JS, Ruby, Java)
**And** each opens a PR in its respective repo

**And** each SDK's integration tests pass against a server built from Story 30.1
**And** each SDK's CI runs and passes

### Story 30.3: SchedulerCommand::Enqueue Batch Refactor & Scheduler Handler

As a developer,
I want the scheduler's `Enqueue` command to accept a batch of messages with a single reply channel,
So that gRPC handlers can submit entire batches in one round-trip instead of N sequential round-trips.

**Acceptance Criteria:**

**Given** the current `SchedulerCommand::Enqueue` takes a single `Message` and a single `oneshot::Sender<Result<Uuid, EnqueueError>>`
**When** it is refactored
**Then** `SchedulerCommand::Enqueue` takes `messages: Vec<Message>` and `reply: oneshot::Sender<Vec<Result<Uuid, EnqueueError>>>`

**Given** `flush_coalesced_enqueues()` (scheduler/mod.rs:220) implements the complete 4-phase batch pattern
**When** the `Enqueue` handler is updated
**Then** the core batch logic is extracted into a shared method callable by both `flush_coalesced_enqueues` and the `Enqueue` match arm

**Given** a batch contains messages targeting multiple queues that route to different shards
**When** the broker routes the batch
**Then** messages are grouped by shard, one `Enqueue` is sent per shard, and results are reordered to match the original input order

**Given** a batch where some messages fail `prepare_enqueue()`
**When** the batch is processed
**Then** failed messages get per-message error results without failing the rest of the batch

**Given** a batch where `apply_mutations()` fails
**When** the storage error occurs
**Then** all callers in the batch receive the storage error

**And** all callers of the old single-message `Enqueue` are updated to wrap in a `Vec` of 1 (compiler will enforce this)
**And** all existing tests pass (508+ tests)
**And** new unit tests verify: single-message batch, multi-message same-queue batch, multi-queue batch with shard routing, partial failure, storage failure propagation

### Story 30.4: Unified Enqueue Handler — Throughput Fix

As a developer,
I want the unified enqueue handler to submit batches to the scheduler as a single `Enqueue` command,
So that the sequential per-message round-trip bottleneck (328 msg/s = 3ms x N) is eliminated.

**Acceptance Criteria:**

**Given** the unified enqueue handler from Story 30.1 receives a request with one or more messages
**When** it processes the messages
**Then** ACL checks are performed per-message, passing messages are submitted as a single `SchedulerCommand::Enqueue`, and the `Vec<Result>` is mapped to the response

**Given** `StreamEnqueue` drains messages from the stream
**When** it processes a batch
**Then** the `repeated` messages from each stream write are submitted as a single `SchedulerCommand::Enqueue`

**Given** cluster mode routes through `ClusterHandle`
**When** the unified enqueue is used in cluster mode
**Then** cluster writes are handled (document the approach)

**And** the benchmark suite runs before and after with results in the PR description
**And** all existing tests pass
**And** new integration tests verify: single-message enqueue, multi-message enqueue, stream with multiple messages per write, mixed success/failure

### Story 30.5: Profile Checkpoint — Post-Tier 0 Analysis

As a developer,
I want to profile the system after the throughput fix to identify the new bottleneck,
So that Tier 1 work (stories 30.6-30.8) is justified by measurement, not speculation.

**Acceptance Criteria:**

**Given** stories 30.3 and 30.4 have been merged
**When** profiling is performed
**Then** a flamegraph is generated for the enqueue hot path under sustained load (4 producers, 1KB, 1000-message batches)

**Given** subsystem-level benchmarks exist (Epic 27)
**When** they are run post-Tier 0
**Then** gRPC/HTTP2 overhead, scheduler processing time, RocksDB write time, and Lua hook time are isolated and reported

**Given** the competitive benchmark suite exists at `bench/competitive/`
**When** it is re-run with identical parameters
**Then** the Fila-to-Kafka ratio is updated

**Given** the benchmark results
**When** analysis is complete
**Then** `docs/benchmarks.md` is updated with the new results
**And** a brief analysis is written at `_bmad-output/planning-artifacts/research/post-tier0-profiling-analysis.md` identifying: (a) where time is now spent, (b) whether gRPC protocol overhead is the new bottleneck, (c) a go/no-go recommendation for stories 30.6-30.8

**And** all existing tests pass

### Story 30.6: StreamEnqueue with Batch-Within-Stream (Conditional)

> **Gate:** Proceeds only if Story 30.5's profiling identifies gRPC protocol overhead as the bottleneck and recommends Tier 1.

As a developer,
I want `StreamEnqueue` to carry multiple messages per stream write via `repeated` fields,
So that per-message HTTP/2 DATA frame overhead is amortized across the batch.

**Acceptance Criteria:**

**Given** the current `StreamEnqueue` sends one message per stream write (even after 30.1's `repeated` field, clients may still send 1 at a time)
**When** the server and SDK stream handling are optimized
**Then** the SDK groups messages into stream writes of N messages (configurable), reducing the number of HTTP/2 DATA frames

**Given** stream responses contain results for a batch
**When** the server responds
**Then** responses are keyed by `sequence_number` so clients can correlate which batch each response covers

**Given** a stream write contains messages that partially fail
**When** the server processes the batch
**Then** per-message results are returned — successful and failed — within the same response

**And** all existing tests pass
**And** new integration tests verify: batch-within-stream round-trip, sequence number correlation, partial failure within a stream batch

### Story 30.7: SDK Adaptive Client-Side Accumulator (Conditional)

> **Gate:** Proceeds only if Story 30.5's profiling recommends Tier 1. Depends on Story 30.6.

As a developer using the Fila SDK,
I want the SDK to automatically accumulate and flush messages at the optimal rate,
So that throughput is maximized without manual tuning.

**Acceptance Criteria:**

**Given** the SDK has an accumulator
**When** messages are enqueued
**Then** the SDK adaptively adjusts flush timing and batch size based on observed throughput (e.g. measuring acks/sec, adjusting linger up when throughput increases, down when latency spikes)

**Given** the adaptive algorithm is running
**When** workload characteristics change (burst vs steady, large vs small payloads)
**Then** the algorithm converges to a new optimal point without manual intervention

**Given** a user has a specific workload requirement
**When** they provide explicit config overrides (max_batch_size, linger, max_batch_bytes)
**Then** the overrides cap or pin the adaptive algorithm's parameters

**Given** a caller wants to force-flush
**When** `Client::flush()` is called
**Then** all pending messages are flushed immediately

**And** the adaptive defaults beat Kafka's `linger.ms=5, batch.size=1MB` in competitive benchmarks without manual tuning
**And** all existing tests pass
**And** new integration tests verify: adaptive convergence under sustained load, response to workload changes, override behavior, `flush()` correctness
**And** the benchmark suite shows the adaptive algorithm matching or beating hand-tuned static configs

### Story 30.8: Profile Checkpoint & Competitive Benchmark Update (Conditional)

> **Gate:** Proceeds only if stories 30.6 and 30.7 were implemented.

As a developer,
I want to profile the system after Tier 1 and update competitive benchmarks,
So that the throughput improvement is quantified and the next optimization target is identified.

**Acceptance Criteria:**

**Given** stories 30.6 and 30.7 have been merged
**When** profiling is performed
**Then** a flamegraph is generated for the `StreamEnqueue` path under sustained load (4 producers, default SDK adaptive config)

**Given** the competitive benchmark suite at `bench/competitive/`
**When** it is re-run with Fila using default adaptive SDK config
**Then** `docs/benchmarks.md` is updated with the new results including the Fila-to-Kafka ratio

**Given** the profiling results
**When** analysis is complete
**Then** a brief analysis is appended to the profiling document identifying remaining bottlenecks and whether Tier 2 (per-message CPU elimination) is the next priority

**And** all existing tests pass
