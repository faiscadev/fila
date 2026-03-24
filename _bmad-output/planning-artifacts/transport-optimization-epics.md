---
stepsCompleted:
  - step-01-validate-prerequisites
  - step-02-design-epics
  - step-03-create-stories
  - step-04-final-validation
status: complete
inputDocuments:
  - '_bmad-output/planning-artifacts/research/post-optimization-profiling-analysis-2026-03-24.md'
  - '_bmad-output/planning-artifacts/performance-optimization-epics.md'
  - '_bmad-output/implementation-artifacts/sprint-status.yaml'
---

# Fila - Transport Optimization Epic (29)

## Overview

Post-optimization profiling (2026-03-24) revealed that **94% of per-message time (~120µs of ~127µs) is gRPC/HTTP2 transport overhead**. RocksDB has 20x headroom (160K ops/s vs 7.8K delivered). The competitive throughput benchmark is also unfair: Kafka uses `linger.ms=5` + batch 1000 messages per network call, while Fila sends 1 msg per RPC. Fila already has `BatchEnqueue` RPC and SDK auto-batching (Epic 26), but the competitive benchmark doesn't use them.

In **lifecycle benchmarks** (unbatched, fair comparison), Fila already beats Kafka 7.6x and RabbitMQ 4.1x. The throughput gap is exclusively a batching + transport amortization story.

**Baseline (commit bc539a9):**
- Single-producer enqueue: 7,856 msg/s (1KB)
- Competitive throughput (Docker, unbatched): 2,637 msg/s
- Competitive throughput Kafka (Docker, batched): 143,278 msg/s
- Lifecycle (unbatched): Fila 2,724 > Kafka 356 > RabbitMQ 658

**Priority actions from profiling:**

| Priority | Action | Impact | Story |
|----------|--------|--------|-------|
| P0 | Fix competitive benchmark to use Fila's batching | Fair comparison | 29.1 |
| P0 | Validate SDK auto-batching works well under load | Prerequisite for fair numbers | 29.1 |
| P1 | Streaming enqueue RPC | 5-15x throughput (amortize HTTP/2 framing) | 29.2, 29.3 |
| P1 | SDK streaming integration | SDK uses streaming transparently | 29.3 |

## Requirements

### Functional Requirements

FR-T1: Competitive benchmark uses Fila's existing `BatchEnqueue` RPC and SDK auto-batching for throughput scenarios (matching Kafka's `linger.ms=5` + batch 1000 approach)
FR-T2: Competitive benchmark retains unbatched lifecycle scenario as a separate, clearly labeled comparison
FR-T3: Server supports a bidirectional streaming `StreamEnqueue` RPC that accepts a persistent message stream and returns acknowledgments
FR-T4: Streaming enqueue amortizes HTTP/2 stream creation — one HTTP/2 stream handles many messages instead of one stream per message
FR-T5: SDK transparently uses streaming enqueue when available, falling back to unary `BatchEnqueue` for compatibility
FR-T6: Streaming enqueue preserves per-message error semantics (each message gets an individual ack or error)

### Non-Functional Requirements

NFR-T1: Competitive throughput benchmark with batching shows Fila within 3x of Kafka (currently 54x gap is unfair comparison)
NFR-T2: Streaming enqueue throughput >= 30K msg/s single-producer (1KB) — 4x improvement over current 7.8K
NFR-T3: Streaming enqueue latency p50 <= 2ms under sustained load
NFR-T4: Existing unary `Enqueue` and `BatchEnqueue` RPCs remain fully functional (backward compatible)
NFR-T5: Memory overhead from streaming connections <= 10MB per stream

---

## Epic 29: Transport Optimization — Fair Benchmarks & Streaming Enqueue

Close the transport gap identified by profiling. The competitive benchmark currently understates Fila by 10-50x because it doesn't use batching. Streaming enqueue amortizes the HTTP/2 per-message overhead (~120µs) that dominates the critical path.

**Prerequisites:** Epic 26 (SDK auto-batching), Epic 27 (profiling infrastructure), Epic 12 (benchmark suite)

### Story 29.1: Fair Competitive Benchmark

As a developer evaluating Fila against other brokers,
I want the competitive benchmark to use each broker's recommended high-throughput configuration,
So that the comparison reflects real-world performance rather than penalizing Fila for not using its own batching features.

**Acceptance Criteria:**

1. **Given** the competitive benchmark (`bench/competitive/`) currently sends 1 Fila message per RPC while Kafka uses `linger.ms=5` + `batch.num.messages=1000`
   **When** the benchmark is updated for fair comparison
   **Then** the throughput scenario uses the Rust SDK with `BatchMode::Auto` (default) and multiple concurrent producers, matching how each broker recommends high-throughput usage

2. **And** the multi-producer scenario also uses `BatchMode::Auto` for Fila

3. **And** the lifecycle scenario (enqueue→consume→ack) remains unbatched for all brokers — this is the latency-focused, fair serial comparison where Fila already leads

4. **And** results JSON includes a `batching` field per scenario indicating the batching strategy used (e.g., `"auto"`, `"none"`, `"linger_ms=5"`)

5. **And** `METHODOLOGY.md` is updated to document: why throughput scenarios use each broker's recommended batching, why lifecycle stays unbatched, and what "fair comparison" means

6. **And** `docs/benchmarks.md` is updated with the new results and clearly labels which scenarios use batching vs unbatched

7. **And** the benchmark is run end-to-end (all 4 brokers) and produces valid results JSON

8. **And** SDK auto-batching (`BatchMode::Auto`) is validated to work correctly under the benchmark's concurrent producer load — if any issues are found, they are fixed in `fila-sdk` as part of this story

9. **And** all existing tests pass (zero regressions)

### Story 29.2: Streaming Enqueue RPC (Server-Side)

As a high-throughput producer,
I want to stream messages over a persistent gRPC connection without per-message HTTP/2 stream overhead,
So that enqueue throughput is limited by storage speed, not transport framing.

**Acceptance Criteria:**

1. **Given** each unary `Enqueue` RPC creates a new HTTP/2 stream (~120µs overhead per message)
   **When** streaming enqueue is implemented
   **Then** a new `StreamEnqueue` RPC is added to `service.proto` as a bidirectional streaming RPC: `rpc StreamEnqueue(stream StreamEnqueueRequest) returns (stream StreamEnqueueResponse)`

2. **And** `StreamEnqueueRequest` contains the same fields as `EnqueueRequest` (queue, headers, payload) plus a `sequence_number` field (uint64) for correlating responses

3. **And** `StreamEnqueueResponse` contains the `sequence_number`, a `message_id` on success, or an `error` string on failure — per-message granularity

4. **And** the server handler reads from the client stream, processes messages through the existing scheduler pipeline (Lua hooks, DRR, storage), and writes responses to the server stream

5. **And** the server batches incoming stream messages using the same write coalescing logic as `BatchEnqueue` — draining available messages from the stream before flushing to storage

6. **And** per-queue ordering is preserved: messages from the same stream to the same queue are stored in stream order

7. **And** stream errors (client disconnect, transport failure) are handled gracefully — in-flight messages that were already committed get their responses sent; uncommitted messages are dropped without side effects

8. **And** backpressure is handled via gRPC flow control — if the server falls behind, the client's stream send blocks naturally

9. **And** the server supports multiple concurrent `StreamEnqueue` streams (one per producer connection)

10. **And** existing unary `Enqueue` and `BatchEnqueue` RPCs are unchanged (fully backward compatible)

11. **And** new integration tests verify: basic stream enqueue (send N messages, receive N acks), sequence number correlation, per-message error handling (bad queue name mid-stream), client disconnect handling, concurrent streams

12. **And** benchmark comparison: streaming enqueue throughput vs unary enqueue throughput at 1KB, demonstrating >= 3x improvement from stream reuse

13. **And** all existing tests pass (zero regressions)

### Story 29.3: SDK Streaming Enqueue Integration

As a developer using the Rust SDK,
I want `enqueue()` to transparently use streaming when it improves throughput,
So that I get streaming performance without changing my code.

**Acceptance Criteria:**

1. **Given** the Rust SDK's `BatchMode::Auto` batcher currently flushes via unary `Enqueue`/`BatchEnqueue` RPCs
   **When** streaming integration is implemented
   **Then** `BatchMode::Auto` opens a persistent `StreamEnqueue` stream and routes all enqueue traffic through it

2. **And** the batcher assigns monotonically increasing sequence numbers to each message and correlates responses by sequence number

3. **And** the stream is lazily opened on first `enqueue()` call and kept alive for the connection lifetime

4. **And** if the stream breaks (transport error, server restart), the batcher automatically reconnects and opens a new stream — in-flight messages receive transport errors, subsequent messages use the new stream

5. **And** `BatchMode::Linger` also uses streaming when available (same stream, timer-based flush sends a burst of messages)

6. **And** `BatchMode::Disabled` continues to use unary `Enqueue` RPC (no streaming) — for users who want simple request-response semantics

7. **And** `batch_enqueue()` (explicit manual batch) continues to use the unary `BatchEnqueue` RPC — streaming is for the auto-batcher path only

8. **And** the SDK gracefully handles servers that don't support `StreamEnqueue` (older versions): detects the "unimplemented" gRPC status on first attempt and falls back to unary RPCs permanently for that connection

9. **And** new integration tests verify: streaming enqueue basic flow, stream reconnection after server restart, fallback to unary on unsupported server, sequence number correlation, concurrent enqueue from multiple tasks

10. **And** benchmark: SDK streaming throughput >= 25K msg/s single-producer (1KB), demonstrating the transport gap closure

11. **And** all existing tests pass (zero regressions)

---

## Deferred / Out of Scope

The following optimizations from the profiling analysis are **not in this epic** — they are lower priority and independent:

- **P1: Scheduler sharding** (multi-queue scaling) — `shard_count` config already exists. Tuning + validation is a separate concern from transport.
- **P2: Decouple delivery from enqueue** — separate delivery thread/timer. Independent optimization.
- **P2: Async ack / pipelining** — new protocol semantics for consume path. Large effort, independent from enqueue streaming.
- **P3: Lazy DRR round + MultiGet** — high cardinality optimization. Targeted, independent.
- **Defer: Purpose-built storage (FR66/Epic 25)** — not justified until throughput exceeds ~50K msg/s (RocksDB ceiling at 160K ops/s).
