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

This document provides the complete epic and story breakdown for Fila's performance optimization phase: fixing tracing overhead and replacing gRPC with a custom binary protocol. This work is motivated by profiling data showing gRPC/HTTP2 consumes 46-62% of server CPU, tracing `#[instrument]` Debug-formats payloads on hot paths (+15% overhead), and per-message command round-trips penalize non-batch operations (+30% overhead from batching).

## Requirements Inventory

### Functional Requirements

**New (Protocol & Batch)**

- **FR-P1**: The server accepts client connections over a custom binary protocol on a configurable TCP port (replacing gRPC port 5555)
- **FR-P2**: All operations are batch-native — every request accepts multiple items, single message = batch of 1
- **FR-P3**: Enqueue accepts multiple messages in a single request and returns per-message results (ID or error)
- **FR-P4**: Ack accepts multiple message IDs in a single request and returns per-message results
- **FR-P5**: Nack accepts multiple message IDs in a single request and returns per-message results
- **FR-P6**: Consume delivers messages via a persistent streaming connection (server-push), delivering batches of ready messages
- **FR-P7**: All admin operations work over the binary protocol: CreateQueue, DeleteQueue, SetConfig, GetConfig, GetStats, Redrive
- **FR-P8**: The protocol supports TLS encryption (same mTLS model as current gRPC)
- **FR-P9**: The protocol supports API key authentication (same auth model as current gRPC)
- **FR-P10**: Per-queue ACLs are enforced on the binary protocol identically to gRPC
- **FR-P11**: Cluster inter-node communication (Raft forwarding, leader routing) uses the binary protocol
- **FR-P12**: All 6 SDKs (Rust, Go, Python, JavaScript, Ruby, Java) support the binary protocol
- **FR-P13**: gRPC is fully removed from the codebase — no dual-protocol support
- **FR-P14**: The wire format is documented with enough detail for third-party client implementations
- **FR-P15**: The `fila` CLI uses the binary protocol for all operations

**Existing (Impacted — must work identically on new protocol)**

- FR1-7: Message lifecycle (enqueue, consume, ack, nack, visibility timeout, DLQ, redrive)
- FR30-34: Queue management (CRUD, auto-DLQ, stats, CLI)
- FR26-29: Runtime configuration (set/get config, Lua-readable)
- FR42-48: Client SDKs (all 6 languages)
- FR71-73: Authentication (mTLS, API keys, per-queue ACLs)
- FR67-70: Clustering (Raft consensus, transparent routing, failover)

**Existing (Unchanged — internal, not affected by transport)**

- FR8-12: Fair scheduling (DRR, fairness keys, weights)
- FR13-17: Throttling (token buckets, skip-throttled)
- FR18-25: Rules engine (Lua hooks)
- FR35-41: Observability (OTel metrics, tracing — tracing fix is pre-work)

### NonFunctional Requirements

**New (Protocol Performance)**

- **NFR-P1**: Binary protocol throughput >= 1.8x current gRPC throughput (based on profiling: gRPC uses 46-62% CPU)
- **NFR-P2**: Batch enqueue of N messages has lower per-message overhead than N individual enqueues
- **NFR-P3**: Wire format overhead < 16 bytes per message beyond payload (no HTTP/2 framing, no protobuf field tags on payload bytes)
- **NFR-P4**: Connection establishment < 1ms (no HTTP/2 SETTINGS exchange)
- **NFR-P5**: The protocol is length-prefixed for zero-copy parsing — no scanning for delimiters

**Existing (Motivating — targets for this work)**

- NFR1: 100,000+ msg/s single-node throughput
- NFR6: Enqueue-to-consume latency < 1ms p50

**Existing (Must preserve)**

- NFR8: Full state recovery after crash, zero message loss
- NFR12: At-least-once delivery
- NFR15: Single binary, no external runtime dependencies
- NFR27: mTLS handshake < 5ms overhead per connection
- NFR28: API key validation < 100us per request

### Additional Requirements

**From Architecture:**

- The single-threaded scheduler core design is unchanged — the binary protocol replaces only the IO/transport layer (tonic gRPC -> custom TCP)
- Channel architecture (crossbeam inbound, tokio mpsc outbound) is unchanged — protocol handlers feed the same `SchedulerCommand` enum
- The `fila-proto` crate is removed or repurposed — protobuf codegen is no longer needed for wire format
- `fila-server` service/admin_service layers are replaced by binary protocol handlers
- Cluster gRPC on port 5556 is also replaced by binary protocol
- All existing tests (432) must pass after migration (adapted to new protocol)
- Benchmark suite (`fila-bench`) must be updated to use binary protocol

**From Performance Research:**

- Tracing `#[instrument]` on hot-path functions Debug-formats request payloads (including 1KB message bodies as hex) twice per request — fix before protocol work for +15% on any transport
- Batch-native API design means the scheduler command channel carries batches, not individual messages — scheduler processes batches end-to-end
- Profile before and after each major change — flamegraphs, not estimates
- Benchmark checkpoints must run `cargo bench` and paste real measured numbers

### FR Coverage Map

| FR | Epic | Description |
|----|------|-------------|
| FR35-41 | Epic 18 | Tracing hot-path fix (observability) |
| FR-P2 | Epic 19 | Batch-native operations (design) |
| FR-P14 | Epic 19 | Wire format documentation |
| FR-P1 | Epic 20 | Binary protocol server |
| FR-P3 | Epic 20 | Batch enqueue |
| FR-P4 | Epic 20 | Batch ack |
| FR-P5 | Epic 20 | Batch nack |
| FR-P6 | Epic 20 | Streaming consume |
| FR-P7 | Epic 20 | Admin ops over binary protocol |
| FR-P8 | Epic 20 | TLS support |
| FR-P9 | Epic 20 | API key auth |
| FR-P10 | Epic 20 | Per-queue ACLs |
| FR-P15 | Epic 20 | CLI migration |
| FR1-7 | Epic 20 | Message lifecycle (preserved) |
| FR26-34 | Epic 20 | Queue management + runtime config (preserved) |
| FR46 | Epic 20 | Rust SDK |
| FR71-73 | Epic 20 | Auth (preserved) |
| FR-P11 | Epic 20 | Cluster inter-node communication |
| FR-P13 | Epic 20 | gRPC removal |
| FR67-70 | Epic 20 | Clustering (preserved) |
| FR-P12 | Epic 20+21 | All 6 SDKs (Rust in 20, external 5 in 21) |
| FR42-48 | Epic 21 | External SDK migration (Go, Python, JS, Ruby, Java) |

## Epic List

### Epic 18: Tracing Hot-Path Fix
Operators get +15% throughput with zero behavior changes. Fix `#[instrument]` macros that Debug-format request payloads on hot-path functions. Benchmark before and after with flamegraphs.
**FRs covered:** FR35-41
**NFRs covered:** NFR1, NFR-P1

### Epic 19: Wire Format Design & Batch-Native Scheduler
The protocol wire format is designed and documented, and the scheduler processes batches end-to-end. No new transport yet — the internal pipeline is batch-ready and the wire format spec exists for all subsequent stories.
**FRs covered:** FR-P2, FR-P14
**NFRs covered:** NFR-P3, NFR-P5

### Epic 20: Binary Protocol Server, Rust SDK & gRPC Removal
Full protocol migration in one epic: binary protocol server for all operations, cluster inter-node communication, Rust SDK, CLI, and gRPC removal. Single protocol when done — no dual-protocol maintenance window. The wire format codec is extracted into a shared `crates/fila-fibp/` crate (frame types, opcodes, encode/decode) so the server and Rust SDK share a single implementation.
**FRs covered:** FR-P1, FR-P3, FR-P4, FR-P5, FR-P6, FR-P7, FR-P8, FR-P9, FR-P10, FR-P11, FR-P13, FR-P15, FR1-7, FR26-34, FR46, FR67-73
**NFRs covered:** NFR-P1, NFR-P2, NFR-P4

### Epic 21: External SDK Migration
All 5 external SDKs (Go, Python, JS, Ruby, Java) support the binary protocol. SDK users in all languages can use Fila's new protocol. Server only speaks binary protocol — no gRPC fallback.
**FRs covered:** FR-P12, FR42-48

---

## Epic 18: Tracing Hot-Path Fix

Operators get +15% throughput with zero behavior changes. Fix `#[instrument]` macros that Debug-format request payloads on hot-path functions. Benchmark before and after with flamegraphs.

### Story 18.1: Baseline Benchmark & Flamegraph

As an operator,
I want a documented performance baseline with flamegraph profiling,
So that subsequent optimizations can be measured against real data.

**Acceptance Criteria:**

**Given** the current codebase at Epic 17 head
**When** `cargo bench` runs the throughput benchmarks
**Then** the results (msg/s for 1KB messages, p50/p99 latency) are recorded in the PR description
**And** a flamegraph is captured showing the `#[instrument]` / `Debug` formatting time on hot-path functions (enqueue, ack, nack, consume delivery)

### Story 18.2: Fix Tracing Overhead on Hot-Path Functions

As an operator,
I want hot-path tracing to not Debug-format message payloads,
So that throughput improves without losing observability.

**Acceptance Criteria:**

**Given** hot-path functions in fila-server and fila-core (enqueue handler, ack handler, nack handler, consume delivery path)
**When** `#[instrument]` attributes are updated to skip or use compact formatting for request payloads, message bodies, and large fields
**Then** `cargo bench` throughput is measurably higher than Story 18.1 baseline (numbers pasted in PR)
**And** a flamegraph confirms Debug formatting is no longer a significant hot-path contributor
**And** tracing still emits useful span fields (queue name, message ID, operation type) — observability is not degraded
**And** all 432 existing tests pass

---

## Epic 19: Wire Format Design & Batch-Native Scheduler

The protocol wire format is designed and documented, and the scheduler processes batches end-to-end. No new transport yet — the internal pipeline is batch-ready and the wire format spec exists for all subsequent stories.

### Story 19.1: Wire Format Design & Protocol Specification

As a developer (implementing Fila clients or contributing to the server),
I want a complete protocol specification for Fila's binary wire format,
So that server and client implementations can be built independently from a shared spec.

**Acceptance Criteria:**

**Given** the full set of Fila operations (enqueue, consume, ack, nack, CreateQueue, DeleteQueue, SetConfig, GetConfig, GetStats, Redrive) and the batch-native requirement (every operation accepts multiple items)
**When** the protocol spec is written to `docs/protocol.md`
**Then** the spec defines: frame format (length-prefixed), opcodes for every operation, request/response serialization for each opcode, error codes, streaming consume frame semantics, connection handshake (version negotiation, auth), and TLS layering
**And** the wire format overhead is < 16 bytes per message beyond payload (NFR-P3)
**And** the format is length-prefixed for zero-copy parsing — no delimiter scanning (NFR-P5)
**And** the spec includes a serialization format decision (e.g., raw binary with field IDs, msgpack, bincode, or similar) with rationale
**And** the spec includes batch semantics: how multiple items are encoded in a single frame, how per-item results are returned
**And** the spec is reviewed and approved by Lucas before proceeding to implementation

### Story 19.2: Batch-Native Scheduler Internals

As an operator,
I want the scheduler to process message batches end-to-end,
So that batch operations have lower per-message overhead than individual operations.

**Acceptance Criteria:**

**Given** the current scheduler command channel sends one `SchedulerCommand` per message (e.g., one `Enqueue` per message)
**When** the scheduler command enum and processing loop are updated to accept batch variants (e.g., `EnqueueBatch { messages: Vec<...>, reply }`)
**Then** a batch of N messages traverses the channel as a single command, not N commands
**And** the scheduler processes the batch in a single loop iteration (single RocksDB WriteBatch for N enqueues, single reply with N results)
**And** ack and nack also support batch variants with the same single-command, single-WriteBatch pattern
**And** existing single-message operations work by sending a batch of 1 — no separate single-message code path
**And** `cargo bench` shows batch enqueue of 100 messages has lower per-message overhead than 100 individual enqueues (numbers pasted in PR)
**And** all existing tests pass (single-message paths now go through batch-of-1)

---

## Epic 20: Binary Protocol Server, Rust SDK & gRPC Removal

Full protocol migration in one epic: binary protocol server for all operations, cluster inter-node communication, Rust SDK, CLI, and gRPC removal. Single protocol when done — no dual-protocol maintenance window.

### Story 20.1: Binary Protocol Server — Hot-Path Operations

As a producer/consumer,
I want to connect to Fila over a custom binary protocol for hot-path operations,
So that enqueue/consume/ack/nack have minimal transport overhead.

**Acceptance Criteria:**

**Given** the wire format spec from Story 19.1
**When** a TCP listener is added to fila-server on a configurable port (default 5555)
**Then** clients can connect over TCP and perform: batch enqueue, streaming consume, batch ack, batch nack using the binary wire format
**And** the server decodes binary frames, translates to `SchedulerCommand` batch variants, and encodes responses back to binary frames
**And** TLS is supported (configurable, same cert/key config as current gRPC TLS)
**And** connection handshake includes protocol version negotiation per the spec
**And** gRPC listener remains temporarily on a secondary port for cluster comms (removed in Story 20.5)
**And** integration tests verify all hot-path operations over binary protocol
**And** wire format codec extracted into `crates/fila-fibp/` crate (frame types, opcodes, encode/decode) so the Rust SDK can reuse it
**And** `cargo bench` compares binary protocol vs gRPC throughput (numbers pasted in PR)

### Story 20.2: Admin Operations & Auth on Binary Protocol

As an operator,
I want admin operations and authentication to work over the binary protocol,
So that all Fila functionality is available on a single transport.

**Acceptance Criteria:**

**Given** the binary protocol server from Story 20.1
**When** admin opcodes are implemented (CreateQueue, DeleteQueue, SetConfig, GetConfig, GetStats, Redrive)
**Then** all admin operations work over the binary protocol with the same semantics as gRPC
**And** API key authentication is enforced on the binary protocol (key sent during connection handshake, validated per-request)
**And** per-queue ACLs are enforced identically to gRPC (Produce/Consume/Admin permissions, glob patterns, superadmin bypass)
**And** mTLS mutual authentication works on binary protocol connections
**And** integration tests verify admin ops, auth rejection (bad key, missing key, insufficient permissions), and mTLS

### Story 20.3: Rust SDK Binary Protocol Client

As a Rust developer using fila-sdk,
I want the SDK to communicate over the binary protocol,
So that my application benefits from the lower-overhead transport.

**Acceptance Criteria:**

**Given** the binary protocol server from Stories 20.1-20.2
**When** fila-sdk is updated to use the binary protocol instead of gRPC
**Then** all existing SDK operations work: `enqueue()` (single and batch), `consume()` (streaming), `ack()` (single and batch), `nack()` (single and batch), and all admin operations
**And** TLS and API key auth work through the SDK
**And** the SDK's public API is unchanged or improved (batch operations are first-class, single = batch of 1)
**And** the SDK handles connection errors, reconnection, and leader hints the same as before
**And** Rust SDK uses `fila-fibp` crate for frame encoding/decoding — no duplicate codec implementation
**And** all existing e2e tests in `crates/fila-e2e/` pass using the binary protocol SDK
**And** `cargo bench` end-to-end throughput (SDK -> server -> SDK) shows improvement vs gRPC baseline (numbers pasted in PR)

### Story 20.4: CLI & Cluster Inter-Node Migration

As an operator,
I want the CLI and cluster inter-node communication to use the binary protocol,
So that all Fila communication uses a single transport.

**Acceptance Criteria:**

**Given** the binary protocol server and Rust SDK from Stories 20.1-20.3
**When** fila-cli is updated to use fila-sdk (binary protocol) instead of raw gRPC calls
**Then** all CLI commands work: queue create/delete/list/inspect, config get/set, stats, redrive
**And** CLI supports `--tls` and `--api-key` flags the same as before
**And** cluster inter-node communication (Raft log replication, leader forwarding, queue group management) is migrated from gRPC to the binary protocol
**And** cluster TLS works (inter-node mTLS)
**And** all cluster e2e tests pass (`crates/fila-e2e/tests/cluster.rs`)
**And** all single-node e2e tests pass over binary protocol

### Story 20.5: gRPC Removal & Final Benchmarks

As a developer/operator,
I want gRPC fully removed from the codebase,
So that there is a single protocol with no dead code, reduced binary size, and fewer dependencies.

**Acceptance Criteria:**

**Given** all communication (client, admin, CLI, cluster) uses the binary protocol from Stories 20.1-20.4
**When** gRPC is removed: tonic, prost, tonic-build dependencies removed; fila-proto crate removed or converted to pure type definitions; gRPC service/admin_service handlers removed; protoc build dependency removed
**Then** `cargo build` succeeds with no gRPC-related dependencies
**And** binary size is smaller than the dual-protocol build
**And** all tests pass (unit, integration, e2e, cluster e2e)
**And** the benchmark suite (`fila-bench`) is updated to use the binary protocol
**And** `cargo bench` full throughput suite runs and results are pasted in PR — this is the final performance number for the entire protocol migration
**And** a flamegraph confirms gRPC/HTTP2 is no longer present in the hot path
**And** docs updated: README, architecture.md, configuration.md, API reference reflect binary protocol as the sole transport
**And** `docs/protocol.md` is the canonical protocol reference

---

## Epic 21: External SDK Migration

All 5 external SDKs (Go, Python, JS, Ruby, Java) support the binary protocol. Server only speaks binary protocol — no gRPC fallback. SDK users in all languages can use Fila's new protocol.

### Story 21.1: Go SDK Binary Protocol

As a Go developer using fila-go,
I want the SDK to communicate over Fila's binary protocol,
So that my application benefits from the lower-overhead transport.

**Acceptance Criteria:**

**Given** the wire format spec from Story 19.1
**When** fila-go is updated to implement the binary protocol client (replacing gRPC)
**Then** all existing SDK operations work: Enqueue (single and batch), Consume (streaming), Ack (single and batch), Nack (single and batch), and all admin operations
**And** TLS and API key auth work
**And** the SDK's public API preserves backward compatibility or has a clean migration path
**And** integration tests pass against fila-server's binary protocol port
**And** changes are submitted as a PR in the fila-go repo
**And** CI passes and Cubic review is addressed before the story is marked done

### Story 21.2: Python SDK Binary Protocol

As a Python developer using fila-client,
I want the SDK to communicate over Fila's binary protocol,
So that my application benefits from the lower-overhead transport.

**Acceptance Criteria:**

**Given** the wire format spec from Story 19.1
**When** fila-python is updated to implement the binary protocol client (replacing gRPC)
**Then** all existing SDK operations work: enqueue (single and batch), consume (streaming), ack (single and batch), nack (single and batch), and all admin operations
**And** TLS and API key auth work
**And** the SDK's public API preserves backward compatibility or has a clean migration path
**And** integration tests pass against fila-server's binary protocol port
**And** changes are submitted as a PR in the fila-python repo
**And** CI passes and Cubic review is addressed before the story is marked done

### Story 21.3: JavaScript SDK Binary Protocol

As a JavaScript/Node.js developer using fila-client,
I want the SDK to communicate over Fila's binary protocol,
So that my application benefits from the lower-overhead transport.

**Acceptance Criteria:**

**Given** the wire format spec from Story 19.1
**When** fila-js is updated to implement the binary protocol client (replacing gRPC)
**Then** all existing SDK operations work: enqueue (single and batch), consume (streaming), ack (single and batch), nack (single and batch), and all admin operations
**And** TLS and API key auth work
**And** the SDK's public API preserves backward compatibility or has a clean migration path
**And** integration tests pass against fila-server's binary protocol port
**And** changes are submitted as a PR in the fila-js repo
**And** CI passes and Cubic review is addressed before the story is marked done

### Story 21.4: Ruby SDK Binary Protocol

As a Ruby developer using fila-client,
I want the SDK to communicate over Fila's binary protocol,
So that my application benefits from the lower-overhead transport.

**Acceptance Criteria:**

**Given** the wire format spec from Story 19.1
**When** fila-ruby is updated to implement the binary protocol client (replacing gRPC)
**Then** all existing SDK operations work: enqueue (single and batch), consume (streaming), ack (single and batch), nack (single and batch), and all admin operations
**And** TLS and API key auth work
**And** the SDK's public API preserves backward compatibility or has a clean migration path
**And** integration tests pass against fila-server's binary protocol port
**And** changes are submitted as a PR in the fila-ruby repo
**And** CI passes and Cubic review is addressed before the story is marked done

### Story 21.5: Java SDK Binary Protocol

As a Java developer using fila-client,
I want the SDK to communicate over Fila's binary protocol,
So that my application benefits from the lower-overhead transport.

**Acceptance Criteria:**

**Given** the wire format spec from Story 19.1
**When** fila-java is updated to implement the binary protocol client (replacing gRPC)
**Then** all existing SDK operations work: enqueue (single and batch), consume (streaming), ack (single and batch), nack (single and batch), and all admin operations
**And** TLS and API key auth work
**And** the SDK's public API preserves backward compatibility or has a clean migration path
**And** integration tests pass against fila-server's binary protocol port
**And** changes are submitted as a PR in the fila-java repo
**And** CI passes and Cubic review is addressed before the story is marked done

