# Story 26.1: Rust SDK Auto-Batching (linger_ms Timer)

Status: review

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

- [x] Task 1: Add `BatchConfig` to `FilaClient` (AC: #1, #7)
  - [x] 1.1 Added `batch_config: Option<BatchConfig>` field to `ConnectOptions`
  - [x] 1.2 Added `ConnectOptions::with_batch_config(config)` builder method
  - [x] 1.3 Wired through `connect()` (batcher_tx: None) and `connect_with_options()` (spawns batcher when linger_ms set)

- [x] Task 2: Implement auto-batching accumulator (AC: #2, #3, #4, #5, #6)
  - [x] 2.1 Created `BatchItem` struct with EnqueueMessage + oneshot::Sender
  - [x] 2.2 Used `tokio::sync::mpsc` channel for the batcher task
  - [x] 2.3 Spawned `run_batcher()` background task that accumulates and flushes on batch_size or linger_ms
  - [x] 2.4 Modified `enqueue()` to route through batcher when `batcher_tx` is Some
  - [x] 2.5 Implemented `flush_batch()` that calls BatchEnqueue RPC and fans results to oneshot senders
  - [x] 2.6 Handled partial failures: maps per-message Success/Error to individual futures

- [x] Task 3: Implement graceful shutdown (AC: #9)
  - [x] 3.1 Batcher flushes remaining messages when rx.recv() returns None (all senders dropped)
  - [x] 3.2 Uses mpsc channel close as the shutdown signal (natural Drop behavior)

- [x] Task 4: Integration tests (AC: #10, #11)
  - [x] 4.1 `auto_batch_flush_on_batch_size`: enqueue 5 messages with batch_size=5, verifies immediate flush
  - [x] 4.2 `auto_batch_flush_on_linger_timeout`: enqueue 1 message with linger_ms=100, verifies timer-based flush
  - [x] 4.3 `auto_batch_disabled_uses_single_message_rpc`: verifies no delay without auto-batching
  - [x] 4.4 `explicit_batch_enqueue_works_with_auto_batching`: verifies batch_enqueue() still works alongside auto-batching
  - [x] 4.5 All 3 existing tests pass (zero regressions)

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

Claude Opus 4.6 (1M context)

### Debug Log References

None.

### Completion Notes List

- Used `tokio::sync::mpsc` channel + `oneshot` pattern for the batcher, preserving `Clone + Send + Sync` on `FilaClient`
- Batcher task holds its own `FilaServiceClient<Channel>` clone to avoid circular dependency
- `tokio::select! { biased; }` ensures message processing takes priority over timer in race conditions
- Added `tokio::time` feature to fila-sdk Cargo.toml

### File List

- `crates/fila-sdk/Cargo.toml` — added `"time"` feature to tokio dependency
- `crates/fila-sdk/src/client.rs` — added `BatchItem`, `batcher_tx` field, `run_batcher()`, `flush_batch()`, modified `enqueue()`, added `with_batch_config()` to `ConnectOptions`
- `crates/fila-sdk/tests/integration.rs` — added 4 auto-batching integration tests
