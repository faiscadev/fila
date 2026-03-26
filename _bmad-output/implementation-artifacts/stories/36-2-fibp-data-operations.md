# Story 36.2: FIBP Data Operations

Status: done

## Story

As a developer,
I want enqueue, consume, ack, and nack operations implemented over the FIBP custom binary protocol,
so that high-throughput producers and consumers can bypass gRPC/HTTP2 overhead for data operations.

## Acceptance Criteria

1. **Given** a FIBP-connected client sends an OP_ENQUEUE frame with a valid queue and messages **Then** the server decodes the binary payload, dispatches to the scheduler via `Arc<Broker>`, and returns a response frame with per-message success/error results.
2. **Given** a FIBP-connected client sends an OP_CONSUME frame **Then** the server registers a consumer with the scheduler, sends an ack frame, and begins pushing delivered messages as FLAG_STREAM push frames.
3. **Given** a consume session is active and the client sends OP_FLOW frames **Then** the server adds the specified credits to the flow-control counter and resumes pushing messages when credits are available.
4. **Given** a FIBP-connected client sends an OP_ACK frame with message IDs **Then** the server dispatches ack commands to the scheduler and returns per-item success/error results.
5. **Given** a FIBP-connected client sends an OP_NACK frame with message IDs and error strings **Then** the server dispatches nack commands to the scheduler, returns per-item results, and the scheduler re-enqueues the messages for redelivery.
6. **Given** a client disconnects while a consume session is active **Then** the consumer is unregistered from the scheduler and the push task is aborted.
7. **Given** the implementation **Then** the wire format module (`wire.rs`) has encode/decode round-trip unit tests for all payload types.
8. **Given** the implementation **Then** e2e integration tests cover: batch enqueue of 100 messages, enqueue-consume-ack lifecycle, and nack with error message and redelivery.
9. **Given** the FIBP connection handler **Then** it receives `Arc<Broker>` via dependency injection (passed through `FibpListener::start()`) and does not depend on gRPC types or the cluster layer.
10. **Given** error mapping **Then** all scheduler error variants (`EnqueueError`, `AckError`, `NackError`) are mapped explicitly to typed FIBP wire error codes (no catch-all string formatting).

## Tasks / Subtasks

- [x] Task 1: Add new error variants to `FibpError` (`InvalidPayload`, `BrokerUnavailable`, `ReplyDropped`)
- [x] Task 2: Add `max_frame_size()` accessor to `FibpCodec`
- [x] Task 3: Create `crates/fila-core/src/fibp/wire.rs` with binary encode/decode for all data payloads
  - [x] 3.1: Enqueue request decode, response encode
  - [x] 3.2: Consume request decode, push frame encode
  - [x] 3.3: Flow frame decode
  - [x] 3.4: Ack/Nack request decode, response encode
  - [x] 3.5: Unit tests for all wire types
- [x] Task 4: Create `crates/fila-core/src/fibp/dispatch.rs` with scheduler command dispatch
  - [x] 4.1: `dispatch_enqueue()` — prepare Messages, send SchedulerCommand::Enqueue, map results
  - [x] 4.2: `register_consumer()` / `unregister_consumer()` — consumer lifecycle
  - [x] 4.3: `dispatch_ack()` — per-item ack dispatch with explicit error mapping
  - [x] 4.4: `dispatch_nack()` — per-item nack dispatch with explicit error mapping
- [x] Task 5: Rewrite `FibpConnection` to use the dispatcher
  - [x] 5.1: Add `Arc<Broker>` and consume state fields
  - [x] 5.2: Implement `handle_enqueue`, `handle_consume`, `handle_ack`, `handle_nack`, `handle_flow`
  - [x] 5.3: Push frame channel pattern (background push task sends encoded bytes via mpsc)
  - [x] 5.4: Select loop in `run()` for concurrent frame reads and push writes
  - [x] 5.5: Consume session cleanup on disconnect
  - [x] 5.6: Manual `Debug` impl (Framed<TcpStream> is not Debug)
- [x] Task 6: Update `FibpListener::start()` to accept `Arc<Broker>`
- [x] Task 7: Update `fila-server/src/main.rs` to pass `Arc::clone(&broker)` to FIBP listener
- [x] Task 8: Update `fibp/mod.rs` to register new modules
- [x] Task 9: E2E integration tests in `crates/fila-e2e/tests/fibp.rs`
  - [x] 9.1: `e2e_fibp_enqueue_batch_100` — 100 messages, all succeed
  - [x] 9.2: `e2e_fibp_enqueue_consume_ack` — full lifecycle with credit flow
  - [x] 9.3: `e2e_fibp_nack_with_error` — nack + redelivery verification

## Dev Notes

- **Push frame architecture**: The consume push loop runs as a separate tokio task that sends pre-encoded `BytesMut` frames through an mpsc channel to the main connection loop. This avoids splitting the `TcpStream` (which would change the `Framed` type) while still allowing concurrent reads and push writes.
- **Credit-based flow control**: The consumer credits are stored in an `AtomicU32` shared between the push task and the main loop. When credits are exhausted, the push task blocks on a `Notify` until OP_FLOW adds more credits.
- **No auth/cluster**: FIBP connections are anonymous (no CallerKey) and local-only (no Raft forwarding). Auth is Story 36.3; cluster integration is future work.
- **Correlation ID**: Client assigns `corr_id`, server echoes it on response frames. Push frames use `corr_id=0`.
