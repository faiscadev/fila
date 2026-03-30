# Story 19.2: Batch-Native Scheduler Internals

Status: ready-for-dev

## Story

As an operator,
I want the scheduler to process message batches end-to-end,
So that batch operations have lower per-message overhead than individual operations.

## Acceptance Criteria

1. **Given** the current scheduler command channel sends one `SchedulerCommand` per message
   **When** the scheduler command enum is updated to accept batch variants
   **Then** a batch of N messages traverses the channel as a single command, not N commands

2. **Given** a batch enqueue command with N messages
   **When** the scheduler processes it
   **Then** it writes all N messages in a single RocksDB WriteBatch and returns a single reply with N per-item results (message_id or error for each)

3. **Given** a batch ack command with N message IDs
   **When** the scheduler processes it
   **Then** it processes all N acks in a single loop iteration with batched storage mutations and returns N per-item results

4. **Given** a batch nack command with N message IDs
   **When** the scheduler processes it
   **Then** it processes all N nacks with batched storage mutations and returns N per-item results

5. **Given** a single-message enqueue via the gRPC API
   **When** the handler creates a scheduler command
   **Then** it sends a batch of 1 — no separate single-message code path exists

6. **Given** batch enqueue of 100 1KB messages
   **When** `cargo bench` runs the throughput benchmarks
   **Then** per-message overhead is measurably lower than 100 individual enqueues (numbers pasted in PR)

7. **Given** the batch refactor is complete
   **When** the full test suite runs
   **Then** all existing tests pass (single-message paths now go through batch-of-1)

## Tasks / Subtasks

- [ ] Task 1: Refactor SchedulerCommand for batch operations (AC: 1, 5)
  - [ ] Replace `Enqueue { message, reply }` with `Enqueue { messages: Vec<Message>, reply }` where reply carries `Vec<Result<Uuid, EnqueueError>>`
  - [ ] Replace `Ack { queue_id, msg_id, reply }` with `Ack { items: Vec<AckItem>, reply }` where `AckItem` is `{ queue_id, msg_id }` and reply carries `Vec<Result<(), AckError>>`
  - [ ] Replace `Nack { queue_id, msg_id, error, reply }` with `Nack { items: Vec<NackItem>, reply }` where `NackItem` is `{ queue_id, msg_id, error }` and reply carries `Vec<Result<(), NackError>>`
  - [ ] Update all callers (service.rs, admin_service.rs, cluster) to send batch-of-1
- [ ] Task 2: Implement batch enqueue handler (AC: 2)
  - [ ] Create `handle_enqueue_batch` in handlers.rs
  - [ ] For each message: validate queue, run Lua on_enqueue, route, construct key
  - [ ] Accumulate all `Mutation::PutMessage` into a single `Vec<Mutation>`
  - [ ] Call `apply_mutations()` once for the entire batch (single WriteBatch)
  - [ ] Update in-memory pending index and DRR for all messages
  - [ ] Return per-item results preserving input order
  - [ ] Individual message failures (e.g., QueueNotFound) don't fail the whole batch
- [ ] Task 3: Implement batch ack handler (AC: 3)
  - [ ] Create `handle_ack_batch` in handlers.rs
  - [ ] For each ack item: look up lease, construct expiry key, find message key
  - [ ] Accumulate all delete mutations (lease, lease_expiry, message) across all items
  - [ ] Call `apply_mutations()` once for the entire batch
  - [ ] Return per-item results (success or MessageNotFound)
- [ ] Task 4: Implement batch nack handler (AC: 4)
  - [ ] Create `handle_nack_batch` in handlers.rs
  - [ ] For each nack item: look up lease, run Lua on_failure, decide retry or DLQ
  - [ ] Accumulate all mutations (delete lease, delete expiry, put/delete message) across all items
  - [ ] Call `apply_mutations()` once for the entire batch
  - [ ] Update in-memory pending index and DRR for all nacked messages
  - [ ] Return per-item results
- [ ] Task 5: Update gRPC handlers to use batch commands (AC: 5)
  - [ ] `service.rs::enqueue()` — wrap single message in `vec![message]`, unwrap `results[0]`
  - [ ] `service.rs::ack()` — wrap single ack in `vec![AckItem { .. }]`, unwrap `results[0]`
  - [ ] `service.rs::nack()` — wrap single nack in `vec![NackItem { .. }]`, unwrap `results[0]`
- [ ] Task 6: Update cluster paths (AC: 5)
  - [ ] `ClusterRequest::Enqueue` carries `Vec<Message>` (add `#[serde(default)]` for backward compat)
  - [ ] `ClusterRequest::Ack` carries `Vec<AckItem>`, `ClusterRequest::Nack` carries `Vec<NackItem>`
  - [ ] Raft state machine apply function dispatches to batch handlers
- [ ] Task 7: Add batch benchmark and verify tests (AC: 6, 7)
  - [ ] Add benchmark: batch enqueue 100 messages vs 100 individual enqueues
  - [ ] Run `cargo bench -p fila-bench --bench system` — paste results in PR
  - [ ] Run full test suite — all existing tests must pass via batch-of-1 path

## Dev Notes

### Architecture: Replace, Don't Add

The batch commands **replace** the single-message commands entirely. There is no `Enqueue` (single) + `EnqueueBatch` (batch) — just `Enqueue { messages: Vec<..> }`. When a single message is enqueued, it becomes `Enqueue { messages: vec![msg], reply }`. This eliminates duplicate code paths.

### Key Files to Modify

| File | Change |
|------|--------|
| `crates/fila-core/src/broker/command.rs` | Refactor SchedulerCommand variants, add AckItem/NackItem types |
| `crates/fila-core/src/broker/scheduler/handlers.rs` | New batch handlers replacing single-message ones |
| `crates/fila-core/src/broker/scheduler/mod.rs` | Update handle_command dispatch |
| `crates/fila-server/src/service.rs` | Wrap single ops in batch-of-1 |
| `crates/fila-core/src/cluster/types.rs` | ClusterRequest variants carry Vec |
| `crates/fila-core/src/cluster/mod.rs` | State machine apply dispatches to batch handlers |
| `crates/fila-core/src/error.rs` | May need no changes — per-item results use existing error types |
| `crates/fila-bench/` | New batch benchmark |

### Storage Layer: apply_mutations() Is Already Batch-Ready

`apply_mutations()` in `crates/fila-core/src/storage/rocksdb.rs` already builds a RocksDB `WriteBatch` from a `Vec<Mutation>`. The single-message handlers call it with 1-4 mutations per message. The batch handlers will feed it N * (1-4) mutations in one call — same API, just bigger Vec.

### Per-Item Error Semantics

A batch of 100 enqueues may have 99 successes and 1 failure (e.g., queue not found for one message targeting a different queue). The reply is `Vec<Result<Uuid, EnqueueError>>` — each item has its own result. **Partial success is normal.** The batch is not transactional across items.

### Lua on_enqueue in Batch Context

Each message in a batch independently runs through the Lua on_enqueue hook (if the target queue has one). The Lua hook may set different fairness_key/weight/throttle_keys per message. This is sequential (Lua VM is single-threaded) — batch doesn't parallelize Lua.

### Cluster Backward Compatibility (Raft Log)

Per CLAUDE.md rule: new fields on ClusterRequest variants need `#[serde(default)]` for backward compat. The `messages` field in `ClusterRequest::Enqueue` changes from `message: Message` to `messages: Vec<Message>`. Old log entries with a single `message` field must still deserialize — handle the migration explicitly: if `messages` is empty, check for legacy `message` field.

### Benchmark: What to Measure

The benchmark should compare:
1. **Individual enqueue**: Send 100 separate `SchedulerCommand::Enqueue` commands (100 channel sends, 100 WriteBatch commits)
2. **Batch enqueue**: Send 1 `SchedulerCommand::Enqueue { messages: vec![..100..] }` (1 channel send, 1 WriteBatch commit)

Expected improvement: batch should have lower per-message overhead because:
- 1 crossbeam channel send vs 100
- 1 RocksDB WriteBatch commit (1 WAL write) vs 100
- 1 oneshot reply vs 100

### What Does NOT Change

- Delivery/consume: already pushes messages one-at-a-time via mpsc. Batch delivery is a future protocol-level concern (Story 20.1).
- Admin operations: remain single-item (no batch CreateQueue, etc.)
- Scheduler event loop structure: still drains commands and runs DRR
- Storage engine trait: `apply_mutations()` signature unchanged

### References

- [Source: crates/fila-core/src/broker/command.rs] — current SchedulerCommand enum
- [Source: crates/fila-core/src/broker/scheduler/handlers.rs] — current single-message handlers
- [Source: crates/fila-core/src/broker/scheduler/mod.rs:114-167] — scheduler event loop
- [Source: crates/fila-core/src/storage/rocksdb.rs:321-367] — apply_mutations WriteBatch
- [Source: crates/fila-core/src/cluster/types.rs] — ClusterRequest enum
- [Source: docs/protocol.md] — wire format spec for batch encoding

## Dev Agent Record

### Agent Model Used

### Debug Log References

### Completion Notes List

### File List
