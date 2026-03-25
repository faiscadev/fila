# Story 30.1: API Surface Unification — Proto, Server, Rust SDK

Status: ready-for-dev

## Story

As a developer,
I want all RPCs and scheduler commands to treat batches as the only path (single = batch of 1),
so that the codebase has one code path per operation, no "batch" prefixes/suffixes, and a clean surface for throughput optimization.

## Acceptance Criteria

1. **Given** the proto defines separate `Enqueue` (unary, 1 msg) and `BatchEnqueue` (unary, N msgs) RPCs
   **When** the proto is unified
   **Then** one `Enqueue` RPC accepts `repeated EnqueueMessage messages` and returns `repeated EnqueueResult results`
   **And** `BatchEnqueue` RPC is removed
   **And** `BatchEnqueueRequest`, `BatchEnqueueResponse`, `BatchEnqueueResult` message types are removed

2. **Given** `StreamEnqueue` sends one message per stream write
   **When** the proto is unified
   **Then** `StreamEnqueueRequest` contains `repeated EnqueueMessage messages` and `uint64 sequence_number`
   **And** `StreamEnqueueResponse` contains `repeated EnqueueResult results`

3. **Given** service.rs has four enqueue functions: `enqueue()`, `batch_enqueue()`, `enqueue_single()`, `enqueue_single_standalone()`
   **When** the handlers are unified
   **Then** there is one enqueue handler that processes `Vec<Message>` (still sends individual `SchedulerCommand::Enqueue` per message — batch scheduler command is Story 30.3)

4. **Given** `SchedulerCommand::Ack` and `SchedulerCommand::Nack` take a single message ID
   **When** they are unified
   **Then** `Ack` takes `Vec<(queue_id, msg_id)>` and returns `Vec<Result<(), AckError>>`
   **And** `Nack` takes `Vec<(queue_id, msg_id, error)>` and returns `Vec<Result<(), NackError>>`

5. **Given** consume delivery has both single-message and batched paths
   **When** delivery is unified
   **Then** consume always delivers via `repeated Message messages` (single message = batch of 1, drop the old `message` singular field)

6. **Given** the Rust SDK has separate `enqueue()` and `batch_enqueue()` methods and a `BatchMode` enum
   **When** the SDK is unified
   **Then** `enqueue()` accepts one or more messages, `batch_enqueue()` is removed
   **And** `BatchMode` is renamed (no "batch" prefix in public API)
   **And** `BatchEnqueueResult`/`BatchEnqueueError` SDK types are removed

7. **Given** the current `BatchEnqueueResult` uses `oneof { EnqueueResponse success, string error }` which loses structured error codes (issue #103)
   **When** the unified `EnqueueResult` proto type is designed
   **Then** it uses a typed `EnqueueErrorCode` enum (`QUEUE_NOT_FOUND`, `STORAGE`, `LUA`, `PERMISSION_DENIED`)
   **And** issue #103 is closed when this story ships

8. All existing tests are updated to use the unified API and all tests pass
9. No "batch" prefix/suffix remains in public API names (proto, SDK, CLI)

## Tasks / Subtasks

- [ ] Task 1: Proto changes (AC: 1, 2, 4, 5, 7)
  - [ ] Rename `EnqueueRequest` → `EnqueueMessage`, new `EnqueueRequest { repeated EnqueueMessage }`
  - [ ] Add `EnqueueResult`, `EnqueueError`, `EnqueueErrorCode` types
  - [ ] Remove `BatchEnqueue` RPC and `BatchEnqueueRequest/Response/Result`
  - [ ] Unify `StreamEnqueueRequest` with `repeated EnqueueMessage`
  - [ ] Unify `AckRequest/Response` with `repeated AckMessage` / `repeated AckResult`
  - [ ] Unify `NackRequest/Response` with `repeated NackMessage` / `repeated NackResult`
  - [ ] Unify `ConsumeResponse` to `repeated Message messages` only
  - [ ] Update `build.rs` bytes config (EnqueueMessage.payload)
- [ ] Task 2: Server handler unification (AC: 3)
  - [ ] Remove `batch_enqueue()` handler
  - [ ] Unify `enqueue()` to process repeated messages
  - [ ] Consolidate `enqueue_single`/`enqueue_single_standalone` into one helper
  - [ ] Update `stream_enqueue()` for new request/response types
  - [ ] Update ack/nack handlers for batch proto
- [ ] Task 3: SchedulerCommand Ack/Nack batch variants (AC: 4)
  - [ ] Change `Ack` to take `Vec<(String, Uuid)>` with `Vec<Result<(), AckError>>` reply
  - [ ] Change `Nack` to take `Vec<(String, Uuid, String)>` with `Vec<Result<(), NackError>>` reply
  - [ ] Update scheduler handler: `handle_ack` processes Vec, `handle_nack` processes Vec
- [ ] Task 4: Consume delivery unification (AC: 5)
  - [ ] Server delivery uses only `repeated Message messages`
  - [ ] SDK consumer handles only `messages` field
- [ ] Task 5: Rust SDK unification (AC: 6, 9)
  - [ ] Remove `batch_enqueue()` method
  - [ ] `enqueue()` builds `EnqueueRequest { messages: vec![msg] }`
  - [ ] Rename `BatchMode` → `AccumulatorMode`
  - [ ] Update batcher and stream manager for new proto types
  - [ ] Remove `BatchEnqueueResult`/`BatchEnqueueError` types
- [ ] Task 6: Update CLI, e2e tests, benchmarks (AC: 8)
  - [ ] CLI enqueue/ack/nack commands
  - [ ] E2E test suite
  - [ ] Benchmark harness (fila-bench)
- [ ] Task 7: Full test suite passes (AC: 8)

## Dev Notes

- **Proto is the foundation** — all other changes flow from it. Change proto first, then fix compile errors outward.
- **Root `proto/` directory** is stale (last updated Feb 19). Canonical source is `crates/fila-proto/proto/fila/v1/`. The root copy is used by external SDKs and needs updating too.
- **Cluster proto unchanged** — `ClusterAck` and `ClusterNack` in `cluster.proto` still take single messages. These are internal Raft types; batch cluster writes are out of scope.
- **SchedulerCommand::Enqueue stays single-message** for this story. The batch scheduler command refactor is Story 30.3.
- **No backward compatibility** — Fila is pre-alpha. Replace old paths, don't preserve them.

### Project Structure Notes

- `crates/fila-proto/proto/fila/v1/service.proto` — proto definitions
- `crates/fila-proto/build.rs` — prost code generation config
- `crates/fila-server/src/service.rs` — gRPC handler implementations
- `crates/fila-core/src/broker/command.rs` — SchedulerCommand enum
- `crates/fila-core/src/broker/scheduler/handlers.rs` — handle_ack, handle_nack
- `crates/fila-core/src/broker/scheduler/delivery.rs` — consume delivery
- `crates/fila-sdk/src/client.rs` — SDK client (BatchMode, enqueue, batch_enqueue, batcher)
- `crates/fila-sdk/src/error.rs` — SDK error types
- `crates/fila-e2e/` — E2E tests
- `crates/fila-bench/` — Benchmark harness
- `crates/fila-server/src/cli.rs` — CLI commands

### References

- [Source: _bmad-output/planning-artifacts/batch-pipeline-epics.md#Story 30.1]
- [Source: crates/fila-proto/proto/fila/v1/service.proto]

## Dev Agent Record

### Agent Model Used

Claude Opus 4.6 (1M context)

### Debug Log References

### Completion Notes List

### File List
