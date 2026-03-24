# Story 26.1: Rust SDK Smart Batching (BatchMode)

Status: review

## Story

As a developer using the Rust SDK,
I want `enqueue()` to automatically batch messages when it makes sense, without any configuration,
so that high-throughput producers get batch performance transparently while low-load producers pay zero latency cost.

## Acceptance Criteria

1. **Given** the Rust SDK sends each `enqueue()` as a separate RPC
   **When** smart batching is implemented
   **Then** a `BatchMode` enum controls batching behavior: `Auto` (default), `Linger` (explicit timer), `Disabled` (no batching)

2. **And** `BatchMode::Auto` (default) uses opportunistic batching: drains whatever messages are available in the channel, flushes without blocking the loop, multiple RPCs in flight concurrently

3. **And** at low load (single producer, no contention), each message is sent as an individual concurrent RPC with zero added latency

4. **And** at high load (multiple concurrent producers), messages naturally cluster in the channel and get drained together as `BatchEnqueue` calls

5. **And** `enqueue()` returns a future that resolves with the message ID when the batch/single RPC containing that message completes

6. **And** if the flush fails (transport error), all futures in that batch resolve with the appropriate error

7. **And** partial batch failures propagate individual results to their corresponding `enqueue()` futures

8. **And** `BatchMode::Linger { linger_ms, batch_size }` preserves explicit timer-based batching for users who want forced batching

9. **And** `BatchMode::Disabled` turns off batching — each `enqueue()` is a direct single-message RPC

10. **And** `connect()` uses `Auto` by default — existing code gets smart batching without changes

11. **And** `batch_enqueue()` remains available for explicit manual batching regardless of batch mode

12. **And** single-item flushes use the `Enqueue` RPC (not `BatchEnqueue`) to preserve exact error semantics like `QueueNotFound`

13. **And** new integration tests verify: auto idle send, auto load batching, linger flush, disabled mode, partial failure, explicit batch_enqueue coexistence

14. **And** all existing tests pass (zero regressions)

## Tasks / Subtasks

- [x] Task 1: Design BatchMode enum (AC: #1, #8, #9, #10)
  - [x] 1.1 Replaced `BatchConfig` struct with `BatchMode` enum (`Auto`, `Linger`, `Disabled`)
  - [x] 1.2 Default is `Auto { max_batch_size: 100 }`
  - [x] 1.3 `ConnectOptions::with_batch_mode(mode)` builder method
  - [x] 1.4 `connect()` delegates to `connect_with_options` so it picks up the default Auto mode

- [x] Task 2: Implement opportunistic auto-batcher (AC: #2, #3, #4, #5, #6, #7)
  - [x] 2.1 `run_auto_batcher`: `recv()` one message, `try_recv()` drain, `tokio::spawn` flush, loop
  - [x] 2.2 Flushes are spawned as independent tasks — multiple RPCs in flight concurrently
  - [x] 2.3 `flush_batch_owned`: single-item uses `Enqueue` RPC, multi-item uses `BatchEnqueue`
  - [x] 2.4 Per-message result fanout via oneshot channels
  - [x] 2.5 Result count mismatch handling (server returns fewer results than messages sent)

- [x] Task 3: Preserve linger batcher (AC: #8)
  - [x] 3.1 Renamed to `run_linger_batcher`, unchanged behavior

- [x] Task 4: Integration tests (AC: #13, #14)
  - [x] 4.1 `nagle_auto_batch_sends_immediately_when_idle`: single message, default connect, no delay
  - [x] 4.2 `nagle_auto_batch_buffers_under_load`: 20 concurrent enqueues, all stored with unique IDs
  - [x] 4.3 `auto_batch_flush_on_batch_size`: linger mode, concurrent sends hit batch_size
  - [x] 4.4 `auto_batch_flush_on_linger_timeout`: linger mode, timer-based flush
  - [x] 4.5 `batch_disabled_uses_single_message_rpc`: disabled mode, no delay
  - [x] 4.6 `auto_batch_partial_failure_propagation`: valid + invalid queue in same batch
  - [x] 4.7 `explicit_batch_enqueue_works_with_auto_batching`: manual batch_enqueue alongside auto mode
  - [x] 4.8 All 3 existing tests pass (zero regressions, including `enqueue_to_nonexistent_queue` with QueueNotFound semantics preserved)

## Dev Notes

### Architecture

`BatchMode::Auto` uses an opportunistic batcher (`run_auto_batcher`):
1. `rx.recv().await` — wait for at least one message
2. `rx.try_recv()` in a loop — drain any messages that arrived concurrently
3. `tokio::spawn(flush_batch_owned(...))` — send without blocking the loop
4. Loop back to step 1 immediately

Multiple RPCs can be in flight simultaneously. The channel acts as the natural buffer. Arrival rate is the tuning knob — no timers, no heuristics, no config.

`flush_batch_owned` uses single-message `Enqueue` RPC when only 1 item is in the batch (preserves exact error types like `QueueNotFound`), and `BatchEnqueue` for multiple items.

### References

- [Source: crates/fila-sdk/src/client.rs — BatchMode, run_auto_batcher, flush_batch_owned]
- [Source: crates/fila-sdk/src/error.rs — per-operation error types]
- [Source: crates/fila-proto/proto/fila/v1/service.proto — Enqueue + BatchEnqueue RPCs]

## Dev Agent Record

### Agent Model Used

Claude Opus 4.6 (1M context)

### Debug Log References

None.

### Completion Notes List

- Replaced `BatchConfig` with `BatchMode` enum — breaking change but pre-1.0
- `connect()` now delegates to `connect_with_options()` so default Auto mode is always active
- Opportunistic batcher uses `tokio::spawn` for flushes — fully concurrent, no serialization
- Single-item optimization: 1 message → `Enqueue` RPC (not `BatchEnqueue`) preserves `QueueNotFound` error semantics
- Leader reconnect in `consume()` uses `BatchMode::Disabled` (no batcher needed for consume path)
- Cubic findings addressed: result count mismatch handling, concurrent batch_size test, uniqueness assertion, partial failure test

### File List

- `crates/fila-sdk/Cargo.toml` — added `"time"` feature to tokio dependency
- `crates/fila-sdk/src/lib.rs` — export `BatchMode` instead of `BatchConfig`
- `crates/fila-sdk/src/client.rs` — `BatchMode` enum, `run_auto_batcher`, `run_linger_batcher`, `flush_batch_owned`, `attach_api_key`, modified `enqueue()` routing, `connect()` delegates to `connect_with_options`
- `crates/fila-sdk/tests/integration.rs` — 7 new integration tests (10 total)
