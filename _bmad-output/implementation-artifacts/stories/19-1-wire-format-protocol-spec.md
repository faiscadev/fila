# Story 19.1: Wire Format Design & Protocol Specification

Status: ready-for-dev

## Story

As a developer (implementing Fila clients or contributing to the server),
I want a complete protocol specification for Fila's binary wire format,
So that server and client implementations can be built independently from a shared spec.

## Acceptance Criteria

1. **Given** the full set of Fila operations (enqueue, consume, ack, nack, CreateQueue, DeleteQueue, SetConfig, GetConfig, GetStats, Redrive) and the batch-native requirement (every operation accepts multiple items)
   **When** the protocol spec is written to `docs/protocol.md`
   **Then** the spec defines: frame format (length-prefixed), opcodes for every operation, request/response serialization for each opcode, error codes, streaming consume frame semantics, connection handshake (version negotiation, auth), and TLS layering

2. **Given** the wire format spec
   **When** overhead is calculated for a single 1KB message enqueue
   **Then** the wire format overhead is < 16 bytes per message beyond payload (NFR-P3)

3. **Given** the wire format spec
   **When** parsing semantics are defined
   **Then** the format is length-prefixed for zero-copy parsing — no delimiter scanning (NFR-P5)

4. **Given** the serialization format options (raw binary with field IDs, msgpack, bincode, postcard, etc.)
   **When** a format is chosen
   **Then** the spec includes the decision with rationale comparing at least 3 options on: overhead per message, zero-copy friendliness, cross-language support, schema evolution

5. **Given** the batch-native API requirement
   **When** batch encoding is specified
   **Then** the spec defines how multiple items are encoded in a single frame and how per-item results (success/error) are returned

6. **Given** the spec is complete
   **When** Lucas reviews it
   **Then** it is approved before Story 19.2 implementation begins

## Tasks / Subtasks

- [ ] Task 1: Research serialization formats (AC: 4)
  - [ ] Evaluate raw binary (hand-rolled TLV), msgpack, bincode, postcard, flatbuffers, cap'n proto
  - [ ] Compare: per-message overhead bytes, zero-copy parse, cross-language codec availability (Go, Python, JS, Ruby, Java), schema evolution story
  - [ ] Make recommendation with rationale
- [ ] Task 2: Design frame format (AC: 1, 2, 3)
  - [ ] Length-prefixed outer frame: `[4-byte length][frame body]` — total frame size known before reading body
  - [ ] Frame header: opcode (1 byte), flags (1 byte), request ID (4 bytes) — 6 bytes fixed overhead per frame
  - [ ] Verify < 16 bytes overhead per message beyond payload
  - [ ] Define endianness (big-endian network byte order)
- [ ] Task 3: Define opcodes for all operations (AC: 1)
  - [ ] Hot-path opcodes: Enqueue (request + response), Consume (subscribe + server-push delivery), Ack (request + response), Nack (request + response)
  - [ ] Admin opcodes: CreateQueue, DeleteQueue, SetConfig, GetConfig, GetStats, ListQueues, Redrive
  - [ ] Control opcodes: Handshake, Ping/Pong, Error, Disconnect
  - [ ] Reserve opcode ranges for future extension
- [ ] Task 4: Specify request/response serialization per opcode (AC: 1, 5)
  - [ ] Enqueue: batch of (queue, headers, payload) → batch of (message_id or error_code)
  - [ ] Ack: batch of (queue, message_id) → batch of (ok or error_code)
  - [ ] Nack: batch of (queue, message_id, error) → batch of (ok or error_code)
  - [ ] Consume: subscribe request → server pushes delivery frames with batch of messages
  - [ ] Admin ops: each with field-level serialization
  - [ ] Define typed error codes (per-operation, matching existing error enums)
- [ ] Task 5: Design connection lifecycle (AC: 1)
  - [ ] Handshake: client sends protocol version + optional API key → server responds with version ack or rejection
  - [ ] TLS: standard TLS wrapping before protocol bytes (same cert/key config as current gRPC)
  - [ ] mTLS: client cert validation during TLS handshake (unchanged from current model)
  - [ ] Multiplexing: request ID allows concurrent requests on a single connection
  - [ ] Consume streaming: after subscribe, server pushes delivery frames until client disconnects or unsubscribes
  - [ ] Ping/Pong keepalive
- [ ] Task 6: Write docs/protocol.md (AC: 1, 2, 3, 4, 5)
  - [ ] Complete specification document
  - [ ] Include frame diagrams (ASCII art)
  - [ ] Include worked examples (enqueue 3 messages, ack 2, consume stream)
  - [ ] Include overhead calculation showing < 16 bytes per message
- [ ] Task 7: Create PR for review (AC: 6)

## Dev Notes

### This Is a Documentation Story

The output is `docs/protocol.md` — a specification document. No code changes to the server, SDK, or scheduler. The spec must be complete enough that:
- Story 19.2 can implement batch scheduler commands from this spec
- Stories 20.1-20.5 can implement the full protocol server/client from this spec
- Stories 21.1-21.5 can implement external SDK clients from this spec

### Current Architecture (What the Protocol Replaces)

**Transport layer (replaced):**
- tonic gRPC on port 5555 (client operations) and port 5556 (cluster inter-node)
- HTTP/2 framing + protobuf serialization
- tonic-build codegen from `proto/fila/v1/*.proto`

**Internal architecture (unchanged):**
- `SchedulerCommand` enum in `crates/fila-core/src/broker/command.rs` — the inbound channel API
- crossbeam bounded channel (IO → scheduler), tokio mpsc (scheduler → consumers), tokio oneshot (request-reply)
- Single-threaded scheduler core event loop in `crates/fila-core/src/broker/scheduler/mod.rs`
- `ClusterRequest` enum in `crates/fila-core/src/cluster/types.rs` — Raft log entries

### Current Operations to Map

**Hot-path (high frequency):**
| Operation | Current Proto | Fields |
|-----------|--------------|--------|
| Enqueue | `EnqueueRequest` | queue (string), headers (map<string,string>), payload (bytes) |
| Consume | `ConsumeRequest` → `stream ConsumeResponse` | queue (string) → Message stream |
| Ack | `AckRequest` | queue (string), message_id (string) |
| Nack | `NackRequest` | queue (string), message_id (string), error (string) |

**Admin (low frequency):**
| Operation | Fields |
|-----------|--------|
| CreateQueue | name, config (visibility_timeout, max_retries, dlq, lua scripts, weights) |
| DeleteQueue | queue_id |
| SetConfig | key, value |
| GetConfig | key → value |
| GetStats | queue_id → stats (depth, consumers, etc.) |
| ListQueues | → list of queue IDs |
| Redrive | dlq_queue_id, count → moved count |

### Batch-Native Requirement

Every operation accepts multiple items. Single message = batch of 1. This is settled design from Epic 30's API unification — batch is the only path. The wire format must encode batch count + per-item results efficiently. No separate "batch" opcodes.

### Performance Context

From Epic 18 profiling (commit 10cbedd baseline):
- gRPC/HTTP2 framing: ~13% of enqueue CPU time (`h2/hyper framing` in flamegraph)
- RocksDB WAL write: ~47% of scheduler thread (not addressable by protocol change)
- Tracing overhead: fixed in Epic 18 (+17.4% throughput)
- Target: NFR-P1 says binary protocol throughput >= 1.8x gRPC throughput
- Target: NFR-P4 says connection establishment < 1ms (no HTTP/2 SETTINGS exchange)

### Design Constraints

- **Cross-language**: 6 SDKs (Rust, Go, Python, JS, Ruby, Java) must all implement this protocol. Don't pick a serialization format that only has good Rust support.
- **Zero-copy friendly**: Length-prefixed frames allow reading exact byte counts. Payload bytes should not require copying/transformation.
- **Schema evolution**: Must support adding fields to operations in future versions without breaking older clients.
- **Request multiplexing**: Multiple in-flight requests on one connection (request ID correlation).
- **Streaming**: Consume is server-push after subscribe — not request/response.

### Existing Auth Model (Must Be Preserved)

- TLS: Optional, wraps the TCP connection. Same cert/key files as current gRPC TLS config.
- mTLS: Client cert validation during TLS handshake. No protocol-level change needed.
- API key: Sent during connection handshake. Server validates once per connection. Per-request ACL checks use the authenticated identity.
- ACLs: Per-queue permissions (Produce/Consume/Admin). Glob-style queue patterns. Superadmin bypass. Enforced per-request, not per-connection.

### Cluster Communication

Port 5556 (cluster inter-node) also needs binary protocol. Same frame format, but used for:
- Raft log replication (AppendEntries, Vote, InstallSnapshot)
- Leader forwarding (client request → leader node)
- Queue group management

Consider whether cluster uses the same opcodes as client protocol or has separate cluster-specific opcodes.

### References

- [Source: proto/fila/v1/service.proto] — current hot-path RPC definitions
- [Source: proto/fila/v1/admin.proto] — current admin RPC definitions
- [Source: crates/fila-core/src/broker/command.rs] — SchedulerCommand enum
- [Source: crates/fila-core/src/cluster/types.rs] — ClusterRequest enum
- [Source: _bmad-output/planning-artifacts/research/tracing-hot-path-baseline.md] — profiling data showing gRPC overhead
- [Source: _bmad-output/planning-artifacts/epics.md] — Epic 19-21 requirements and NFRs

## Dev Agent Record

### Agent Model Used

### Debug Log References

### Completion Notes List

### File List
