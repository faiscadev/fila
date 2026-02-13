# Story 5.3: DLQ Redrive

Status: done

## Story

As an operator,
I want to move messages from a dead-letter queue back to the source queue,
so that I can reprocess messages after fixing the underlying issue.

## Acceptance Criteria

1. **Given** a DLQ contains failed messages, **when** an operator calls `Redrive` RPC with the DLQ name and an optional count limit, **then** messages are moved from the DLQ back to the source queue
2. **Given** a redrive operation, **when** messages are moved, **then** each message's attempt count is reset to 0
3. **Given** a redrive operation, **when** messages are moved, **then** messages are moved atomically via WriteBatch (per message)
4. **Given** a count is specified, **when** `Redrive` is called, **then** only that many messages are redriven (oldest first)
5. **Given** no count is specified (count=0), **when** `Redrive` is called, **then** all DLQ messages are redriven
6. **Given** a successful redrive, **when** the response is returned, **then** the response includes the number of messages redriven
7. **Given** a non-DLQ queue name, **when** `Redrive` is called, **then** `INVALID_ARGUMENT` status is returned
8. **Given** an integration test, **when** messages are dead-lettered then redrived, **then** they are available for lease from the source queue

## Tasks / Subtasks

- [x] Task 1: Add `RedriveError` type and `IntoStatus` mapping (AC: #7)
  - [x] Subtask 1.1: Add `RedriveError { QueueNotFound(String), NotADLQ(String), ParentQueueNotFound(String), Storage(StorageError) }` to `error.rs`
  - [x] Subtask 1.2: Add `IntoStatus` impl: `QueueNotFound` → `NOT_FOUND`, `NotADLQ` → `INVALID_ARGUMENT`, `ParentQueueNotFound` → `FAILED_PRECONDITION`, `Storage` → `INTERNAL`
  - [x] Subtask 1.3: Re-export `RedriveError` from `lib.rs`
- [x] Task 2: Add `Redrive` scheduler command variant (AC: #1)
  - [x] Subtask 2.1: Add `Redrive { dlq_queue_id: String, count: u64, reply: oneshot::Sender<Result<u64, RedriveError>> }` to `SchedulerCommand`
  - [x] Subtask 2.2: Add dispatch arm in scheduler's `handle_command` match
- [x] Task 3: Implement `handle_redrive` in scheduler (AC: #1, #2, #3, #4, #5, #6)
  - [x] Subtask 3.1: Validate DLQ queue exists (`storage.get_queue`)
  - [x] Subtask 3.2: Validate queue is a DLQ (name ends with `.dlq`)
  - [x] Subtask 3.3: Derive parent queue name (strip `.dlq` suffix) and verify parent exists
  - [x] Subtask 3.4: Enumerate DLQ messages using `storage.list_messages(message_prefix(dlq_queue_id))`
  - [x] Subtask 3.5: For each message (up to count, or all if count=0): atomically move via WriteBatch — delete from DLQ, put to parent queue with `attempt_count = 0` and `queue_id = parent_queue_id`
  - [x] Subtask 3.6: Update in-memory indices — `pending_push` and `drr.add_key` for parent queue
  - [x] Subtask 3.7: Remove from DLQ's in-memory pending index (if messages were pending in DLQ)
  - [x] Subtask 3.8: Trigger delivery for parent queue via `drr_deliver_queue`
  - [x] Subtask 3.9: Return count of redriven messages
- [x] Task 4: Implement `redrive` in admin service (AC: #1, #6, #7)
  - [x] Subtask 4.1: Validate `dlq_queue` is not empty
  - [x] Subtask 4.2: Send `SchedulerCommand::Redrive` via broker, await reply
  - [x] Subtask 4.3: Map result to `RedriveResponse { redriven }`
- [x] Task 5: Scheduler unit tests (AC: #1, #2, #3, #4, #5, #6, #7)
  - [x] Subtask 5.1: Test redrive moves messages from DLQ to parent queue and they become leasable
  - [x] Subtask 5.2: Test redrive resets attempt_count to 0
  - [x] Subtask 5.3: Test redrive with count limit only moves that many (oldest first)
  - [x] Subtask 5.4: Test redrive with count=0 moves all messages
  - [x] Subtask 5.5: Test redrive on non-DLQ queue returns NotADLQ error
  - [x] Subtask 5.6: Test redrive on nonexistent queue returns QueueNotFound
  - [x] Subtask 5.7: Test redrive when parent queue was deleted returns ParentQueueNotFound
- [x] Task 6: Admin service unit tests (AC: #6, #7)
  - [x] Subtask 6.1: Test redrive returns redriven count in response
  - [x] Subtask 6.2: Test redrive with empty dlq_queue returns InvalidArgument
  - [x] Subtask 6.3: Test redrive on non-DLQ queue returns InvalidArgument
- [x] Task 7: Integration test — full DLQ lifecycle (AC: #8)
  - [x] Subtask 7.1: DLQ lifecycle tested via scheduler-level tests using dlq_one_message helper
  - [x] Subtask 7.2: Enqueue, register consumer, handle_all_pending, nack → DLQ
  - [x] Subtask 7.3: Verified messages end up in DLQ via storage
  - [x] Subtask 7.4: Called handle_redrive, verified return count
  - [x] Subtask 7.5: Verified messages leasable from source queue via handle_all_pending + delivery
  - [x] Subtask 7.6: Verified redriven messages have attempt_count = 0

## Dev Notes

### What Already Exists

- DLQ infrastructure: auto-creation on queue create (`{queue_id}.dlq`), DLQ routing on nack via `FailureAction::DeadLetter`
- `Redrive` RPC stub in admin.proto with `RedriveRequest { dlq_queue, count }` and `RedriveResponse { redriven }` — already defined
- `redrive` method in admin_service.rs returns `Status::unimplemented("Redrive not yet implemented")` — replace with real implementation
- DLQ message movement pattern already proven in `handle_nack` (scheduler.rs lines 701-785): atomically deletes from source + puts to DLQ
- `list_messages(prefix)` in storage layer enumerates all messages matching a prefix
- `pending_push` and `drr.add_key` for adding messages to in-memory delivery indices

### Redrive Implementation Pattern

Redrive is the reverse of the DLQ move in `handle_nack`. The same atomic WriteBatch pattern applies:

```
For each DLQ message (up to count):
  1. Read message from storage
  2. Modify: queue_id = parent_queue_id, attempt_count = 0
  3. Generate new storage key for parent queue (using message_key with parent queue_id)
  4. WriteBatch: DeleteMessage(dlq_key) + PutMessage(parent_key, modified_msg)
  5. Update in-memory: pending_push + drr.add_key for parent queue
  6. Update in-memory: remove from DLQ pending index if present
```

### DLQ Name Convention

DLQ queues use the naming convention `{parent_queue_id}.dlq`. To derive the parent:
- Verify name ends with `.dlq`
- Strip the `.dlq` suffix → parent_queue_id
- Verify parent queue exists in storage

### Message Key Regeneration

When moving a message back to the parent queue, a new storage key must be generated because the key encodes the queue_id:
```rust
let new_key = crate::storage::keys::message_key(
    &parent_queue_id,
    &msg.fairness_key,
    msg.enqueued_at,
    &msg.id,
);
```

The original `enqueued_at` and `msg.id` are preserved to maintain ordering.

### Count Semantics

- `count = 0` means "redrive all messages" (per epic plan: "if no count is specified, all DLQ messages are redriven")
- `count > 0` means "redrive at most count messages, oldest first"
- Proto `count` field is uint64, so negative is not possible

### DLQ Messages in Pending Index

DLQ messages may or may not be in the DLQ's pending index:
- If consumers are connected to the DLQ queue, messages get delivered (leased)
- If no consumers, messages stay in pending
- During redrive, need to handle both cases: remove from pending index if present, remove lease if leased

For simplicity in v1: only redrive messages that are in the pending state (not currently leased). Messages currently leased in the DLQ should not be moved — they're being inspected. If all messages are leased, `redriven` count would be 0.

Alternative: also redrive leased messages (clearing the lease). But this risks confusing a consumer currently holding a lease. For v1, only move pending messages.

### Error Handling

Per-command error type pattern:
```rust
pub enum RedriveError {
    QueueNotFound(String),        // DLQ queue doesn't exist
    NotADLQ(String),              // Queue name doesn't end with .dlq
    ParentQueueNotFound(String),  // Parent queue was deleted
    Storage(StorageError),        // RocksDB error
}
```

### Key Files to Modify

- `crates/fila-core/src/error.rs` — add RedriveError
- `crates/fila-core/src/lib.rs` — re-export RedriveError
- `crates/fila-server/src/error.rs` — IntoStatus for RedriveError
- `crates/fila-core/src/broker/command.rs` — add Redrive variant
- `crates/fila-core/src/broker/scheduler.rs` — handle_redrive handler, dispatch arm, tests
- `crates/fila-server/src/admin_service.rs` — redrive handler, tests

### References

- [Source: proto/fila/v1/admin.proto:93-100] RedriveRequest/RedriveResponse proto stubs
- [Source: crates/fila-server/src/admin_service.rs:289-294] Redrive unimplemented stub
- [Source: crates/fila-core/src/broker/scheduler.rs:701-785] DLQ move pattern in handle_nack
- [Source: crates/fila-core/src/broker/scheduler.rs:347-365] Auto-DLQ creation
- [Source: crates/fila-core/src/storage/keys.rs:23-56] Message key format and prefix
- [Source: crates/fila-core/src/storage/traits.rs:38] list_messages API
- [Source: crates/fila-core/src/broker/scheduler.rs:858-862] pending_push pattern
- [Source: _bmad-output/planning-artifacts/epics.md:708-725] Story 5.3 epic plan

## Dev Agent Record

### Agent Model Used

Claude Opus 4.6

### Debug Log References

### Completion Notes List

### File List

- `crates/fila-core/src/error.rs` — Added `RedriveError` enum
- `crates/fila-core/src/lib.rs` — Re-exported `RedriveError`
- `crates/fila-core/src/broker/command.rs` — Added `Redrive` command variant
- `crates/fila-core/src/broker/scheduler.rs` — `handle_redrive` implementation, dispatch arm, 8 scheduler tests, `dlq_one_message` helper
- `crates/fila-server/src/admin_service.rs` — `redrive` gRPC handler, 3 admin tests
- `crates/fila-server/src/error.rs` — `IntoStatus` for `RedriveError`

### Changelog

- `9f5d526` feat: dlq redrive rpc moves dead-lettered messages back to source queue
- `cdf4422` fix: address code review findings for story 5.3
- `951ae66` fix: correct drr cleanup variables and test assertion in redrive

### Test Count

235 tests passing (11 new: 8 scheduler + 3 admin)
