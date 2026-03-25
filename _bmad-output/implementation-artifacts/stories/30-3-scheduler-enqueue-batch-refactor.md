# Story 30.3: SchedulerCommand::Enqueue Batch Refactor & Scheduler Handler

Status: review

## Story

As a developer,
I want the scheduler's `Enqueue` command to accept a batch of messages with a single reply channel,
so that gRPC handlers can submit entire batches in one round-trip instead of N sequential round-trips.

## Acceptance Criteria

1. `SchedulerCommand::Enqueue` takes `messages: Vec<Message>` and `reply: Sender<Vec<Result<Uuid, EnqueueError>>>`
2. `flush_coalesced_enqueues` processes multi-message commands with per-command result tracking
3. Partial failure: failed prepare_enqueue gets per-message error without failing the batch
4. Storage failure: all callers in the batch receive the storage error
5. `handle_enqueue` removed — all enqueue paths go through `flush_coalesced_enqueues`
6. All 534 tests pass

## Tasks / Subtasks

- [x] Task 1: Change SchedulerCommand::Enqueue to batch variant
- [x] Task 2: Rewrite flush_coalesced_enqueues for multi-message commands
- [x] Task 3: Route handle_command Enqueue through flush_coalesced_enqueues
- [x] Task 4: Remove dead handle_enqueue method
- [x] Task 5: Update all callers (service.rs, cluster, broker, 80+ test sites)
- [x] Task 6: Fix clippy (type_complexity with EnqueueBatch alias)

## Dev Agent Record

### Agent Model Used

Claude Opus 4.6 (1M context)

### Completion Notes List

- `handle_enqueue` removed — single code path for all enqueues through `flush_coalesced_enqueues`
- `EnqueueBatch` type alias introduced for readability
- Per-command result tracking via `PreparedItem { cmd_idx, msg_idx }` indices
- All production callers wrap single message in `vec![msg]` (throughput fix in 30.4 will send actual batches)

### File List

- crates/fila-core/src/broker/command.rs
- crates/fila-core/src/broker/scheduler/mod.rs
- crates/fila-core/src/broker/scheduler/handlers.rs
- crates/fila-core/src/broker/mod.rs
- crates/fila-server/src/service.rs
- crates/fila-core/src/cluster/grpc_service.rs
- crates/fila-core/src/cluster/tests.rs
- crates/fila-core/src/broker/scheduler/tests/*.rs (14 test files)
