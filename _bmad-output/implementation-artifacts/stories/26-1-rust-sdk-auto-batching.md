# Story 26.1: Rust SDK Auto-Batching (linger_ms Timer)

Status: ready-for-dev

## Story

As a developer using the Rust SDK,
I want `enqueue()` to automatically accumulate messages and flush in batches when auto-batching is configured,
so that high-throughput producers get batch performance without manually calling `batch_enqueue()`.

## Acceptance Criteria

1. **Given** `BatchConfig` with `linger_ms` and `batch_size` is defined at `crates/fila-sdk/src/client.rs:33-48` but not wired into `enqueue()`
   **When** auto-batching is implemented
   **Then** `FilaClient` accepts `BatchConfig` via a builder method and buffers `enqueue()` calls when auto-batching is enabled

2. **And** when auto-batching is enabled, `enqueue()` buffers messages in an internal accumulator instead of sending immediately

3. **And** the buffer is flushed via `BatchEnqueue` RPC when either `batch_size` messages are accumulated OR `linger_ms` milliseconds have elapsed since the first message entered the buffer — whichever comes first

4. **And** `enqueue()` returns a future that resolves with the message ID when the batch containing that message is flushed and acknowledged by the server

5. **And** if the batch flush fails (transport error), all buffered `enqueue()` futures resolve with the appropriate error

6. **And** partial batch failures (some messages succeed, some fail) propagate individual results to their corresponding `enqueue()` futures

7. **And** when auto-batching is disabled (default, `linger_ms = None`), `enqueue()` uses the existing single-message `Enqueue` RPC — zero behavior change

8. **And** `batch_enqueue()` remains available for explicit manual batching regardless of auto-batching configuration

9. **And** `Drop` on the client flushes any pending buffered messages before disconnecting

10. **And** new integration tests verify: auto-batch flush on `batch_size` threshold, auto-batch flush on `linger_ms` timeout, partial failure propagation, disabled auto-batching uses single-message RPC

11. **And** all existing tests pass (zero regressions)

## Tasks / Subtasks

- [ ] Task 1: Add `BatchConfig` to `FilaClient` (AC: #1, #7)
  - [ ] 1.1 Add `batch_config: Option<BatchConfig>` field to `FilaClient`
  - [ ] 1.2 Add `ConnectOptions::with_batch_config(config)` builder method
  - [ ] 1.3 Wire `batch_config` through `connect()` and `connect_with_options()`

- [ ] Task 2: Implement auto-batching accumulator (AC: #2, #3, #4, #5, #6)
  - [ ] 2.1 Create internal `BatchAccumulator` struct with: pending messages vec, oneshot senders for per-message results, batch timer handle
  - [ ] 2.2 Wrap accumulator in `Arc<Mutex<>>` or use a dedicated `tokio::sync::mpsc` channel for the batcher task
  - [ ] 2.3 Spawn a background tokio task that owns the accumulator, receives enqueue requests, and flushes on batch_size or linger_ms
  - [ ] 2.4 Modify `enqueue()`: when auto-batching enabled, send message to batcher task via channel, return a `oneshot::Receiver` wrapped as the result future
  - [ ] 2.5 Implement flush: collect buffered messages, call `batch_enqueue()`, map results back to individual oneshot senders
  - [ ] 2.6 Handle partial failures: match `BatchEnqueueResult::Success`/`Error` per message, send appropriate result on each oneshot

- [ ] Task 3: Implement graceful shutdown (AC: #9)
  - [ ] 3.1 On `Drop`, signal the batcher task to flush remaining messages and shut down
  - [ ] 3.2 Consider using `tokio::sync::watch` or closing the sender channel as the shutdown signal

- [ ] Task 4: Integration tests (AC: #10, #11)
  - [ ] 4.1 Test: enqueue N messages with batch_size=N, verify batch flushes immediately and all messages are stored
  - [ ] 4.2 Test: enqueue 1 message with batch_size=100 and linger_ms=50, verify flush happens within ~50ms
  - [ ] 4.3 Test: enqueue with auto-batching disabled, verify single-message RPC used (existing behavior)
  - [ ] 4.4 Test: explicit `batch_enqueue()` still works when auto-batching is enabled
  - [ ] 4.5 Run full existing test suite to verify zero regressions

## Dev Notes

### Architecture

The `FilaClient` at `crates/fila-sdk/src/client.rs:143-151` currently has `inner: FilaServiceClient<Channel>`, `api_key: Option<String>`, and `connect_options: Option<ConnectOptions>`. The auto-batcher will add a new field for the batcher task handle/channel.

**Recommended pattern**: Use a `tokio::sync::mpsc` channel to decouple `enqueue()` from the batch flush logic. The batcher runs as a background task that:
1. Receives `(EnqueueMessage, oneshot::Sender<Result<String, EnqueueError>>)` tuples
2. Accumulates in a `Vec`
3. Starts a `tokio::time::sleep(linger_ms)` timer on first message
4. Flushes when `batch_size` reached or timer fires (use `tokio::select!`)
5. Calls `self.batch_enqueue()` and fans results back via oneshot senders

**Important**: `FilaClient` is `Clone + Send + Sync`. The batcher channel sender is `Clone`, so this property is preserved. The background task holds the receiver end.

### Existing Code to Reuse

- `batch_enqueue()` at `client.rs:265-303` — the flush path calls this directly
- `BatchEnqueueResult` enum at `client.rs:63-68` — map `Success(id)` → resolve oneshot with `Ok(id)`, `Error(msg)` → resolve with `Err(EnqueueError::...)`
- `EnqueueMessage` at `client.rs:50-56` — reuse as the accumulator item type
- `EnqueueError` at `error.rs` — the auto-batched `enqueue()` returns the same error type

### Error Type Consideration

The current `enqueue()` returns `Result<String, EnqueueError>`. The auto-batched path will also return `Result<String, EnqueueError>`. If the batch flush has a transport-level failure, all messages in that batch get `EnqueueError::Transport(...)`. If an individual message fails (partial failure from `BatchEnqueueResult::Error`), that specific future gets an appropriate `EnqueueError` variant.

### Testing

Integration tests live in `crates/fila-sdk/tests/`. Use `TestServer` helper from `crates/fila-e2e/src/lib.rs` to spin up a real server for testing.

### What NOT to Do

- Do NOT change the `enqueue()` method signature — it must remain backward compatible
- Do NOT make auto-batching the default — it must be opt-in via `BatchConfig`
- Do NOT buffer across queues in separate batches — the `BatchEnqueue` RPC handles multi-queue messages in one call
- Do NOT add a separate `enqueue_batched()` method — the existing `enqueue()` transparently switches behavior based on config

### References

- [Source: crates/fila-sdk/src/client.rs — full client implementation]
- [Source: crates/fila-sdk/src/error.rs — per-operation error types]
- [Source: crates/fila-proto/proto/fila/v1/service.proto — BatchEnqueue RPC definition]
- [Source: performance-optimization-epics.md — Story 26.1 AC definition]

## Dev Agent Record

### Agent Model Used

### Debug Log References

### Completion Notes List

### File List
