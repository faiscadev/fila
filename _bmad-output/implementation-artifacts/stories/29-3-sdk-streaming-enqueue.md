# Story 29.3: SDK Streaming Enqueue Integration

Status: backlog

## Story

As a developer using the Rust SDK,
I want `enqueue()` to transparently use streaming when it improves throughput,
So that I get streaming performance without changing my code.

## Acceptance Criteria

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

## Tasks / Subtasks

- [ ] Task 1: Streaming batcher implementation (AC: #1, #2, #3)
  - [ ] 1.1 Open `StreamEnqueue` bidirectional stream on first enqueue
  - [ ] 1.2 Replace `flush_batch_owned` with stream-based send (write messages to stream, read acks)
  - [ ] 1.3 Implement sequence number assignment and response correlation via HashMap
  - [ ] 1.4 Spawn a response reader task that matches acks to pending oneshot senders
  - [ ] 1.5 Keep stream alive across batcher drain cycles (lazy open, persistent)

- [ ] Task 2: Stream lifecycle management (AC: #4, #5, #6, #7)
  - [ ] 2.1 Detect stream failure (send error, response reader exits)
  - [ ] 2.2 Error all in-flight pending messages on stream break
  - [ ] 2.3 Automatically reopen stream on next enqueue
  - [ ] 2.4 Wire `BatchMode::Linger` to use the same stream
  - [ ] 2.5 Keep `BatchMode::Disabled` on unary path
  - [ ] 2.6 Keep `batch_enqueue()` on unary `BatchEnqueue` path

- [ ] Task 3: Backward compatibility fallback (AC: #8)
  - [ ] 3.1 On first stream open, catch gRPC `UNIMPLEMENTED` status
  - [ ] 3.2 Set a connection-level flag to permanently fall back to unary RPCs
  - [ ] 3.3 Test against a mock/older server that returns `UNIMPLEMENTED`

- [ ] Task 4: Integration tests (AC: #9)
  - [ ] 4.1 Basic streaming: enqueue N messages via `BatchMode::Auto`, verify all stored
  - [ ] 4.2 Stream reconnection: restart server mid-stream, verify recovery
  - [ ] 4.3 Fallback: test against server without `StreamEnqueue` (if feasible, or mock)
  - [ ] 4.4 Sequence number correlation: verify IDs match across concurrent enqueues
  - [ ] 4.5 Concurrent tasks: multiple `tokio::spawn` enqueuing simultaneously

- [ ] Task 5: Benchmark validation (AC: #10, #11)
  - [ ] 5.1 Run single-producer streaming benchmark, verify >= 25K msg/s (1KB)
  - [ ] 5.2 Run existing test suite, verify zero regressions

## Design Notes

### Architecture

The auto-batcher currently works as:
1. `rx.recv().await` — wait for a message
2. `rx.try_recv()` — drain available messages
3. `tokio::spawn(flush_batch_owned(...))` — send via unary RPC

With streaming, this becomes:
1. `rx.recv().await` — wait for a message
2. `rx.try_recv()` — drain available messages
3. Write all messages to the persistent stream (assign sequence numbers)
4. A separate response reader task matches acks to pending futures

The stream replaces the per-flush `tokio::spawn` — instead of spawning N concurrent unary RPCs, all messages flow through a single persistent stream. This eliminates HTTP/2 stream creation overhead entirely.

### Sequence number correlation

Each `enqueue()` call gets a oneshot channel for its result. The batcher stores `sequence_number -> oneshot::Sender` in a HashMap. The response reader task receives `StreamEnqueueResponse`, looks up the sequence number, and sends the result through the oneshot.

### Fallback strategy

On `UNIMPLEMENTED`:
- Set `self.streaming_supported = false`
- All future flushes use the existing unary path (`flush_batch_owned`)
- No retry — if the server doesn't support streaming, it won't suddenly support it

### Key Files to Modify

- `crates/fila-sdk/src/client.rs` — `run_auto_batcher`, stream management, sequence tracking
- `crates/fila-sdk/src/error.rs` — any new error variants for stream failures
- `crates/fila-sdk/tests/integration.rs` — streaming integration tests

### References

- [Source: crates/fila-sdk/src/client.rs — BatchMode, run_auto_batcher, flush_batch_owned]
- [Source: crates/fila-proto/proto/fila/v1/service.proto — StreamEnqueue RPC (added in 29.2)]
- [Research: post-optimization-profiling-analysis-2026-03-24.md — strategy 1A: bidirectional streaming]
