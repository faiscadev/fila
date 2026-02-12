# Story 2.3: Nack & Message Retry

Status: done

## Story

As a consumer,
I want to negatively acknowledge (nack) a failed message,
so that it re-enters the ready pool and can be retried by any consumer.

## Acceptance Criteria

1. The message's attempt count is incremented
2. The message re-enters the ready pool for the same fairness key
3. The lease is removed from the `leases` CF
4. The lease expiry entry is removed from the `lease_expiry` CF
5. The updated message (with new attempt count) is written atomically via WriteBatch
6. Nacking an unknown message ID returns `NOT_FOUND` status (idempotent)
7. The default retry behavior (without Lua) is immediate requeue with no maximum attempt limit
8. An integration test verifies: enqueue → lease → nack → lease again → verify attempt count is incremented

## Tasks / Subtasks

- [x] Task 1: Implement `handle_nack()` in Scheduler
  - [x] 1.1 Look up lease, parse expiry, find message key
  - [x] 1.2 Retrieve message, increment attempt_count, clear leased_at
  - [x] 1.3 Atomically update message + delete lease + delete lease_expiry via WriteBatch
  - [x] 1.4 Re-add fairness key to DRR active set for scheduling

- [x] Task 2: Wire Nack command handler
  - [x] 2.1 Replace stub with `handle_nack()` call
  - [x] 2.2 Call `drr_deliver_queue` after successful nack for immediate redelivery

- [x] Task 3: Implement gRPC Nack handler
  - [x] 3.1 Validate request fields (queue, message_id)
  - [x] 3.2 Send SchedulerCommand::Nack via broker
  - [x] 3.3 Add IntoStatus mapping for NackError

- [x] Task 4: Integration tests
  - [x] 4.1 `nack_requeues_message_with_incremented_attempt_count` (AC#1, #2, #7, #8)
  - [x] 4.2 `nack_removes_lease_and_lease_expiry` (AC#3, #4, #5)
  - [x] 4.3 `nack_unknown_message_returns_not_found` (AC#6)
  - [x] 4.4 `nack_then_ack_completes_message_lifecycle` (full lifecycle)

## Dev Agent Record

### Agent Model Used

Claude Opus 4.6

### Debug Log References

- nack_removes_lease_and_lease_expiry initially failed because nack handler calls drr_deliver_queue, which immediately re-delivers the message (creating a new lease). Fixed by unregistering the consumer before nacking.

### Completion Notes List

- `handle_nack()` follows the same pattern as `handle_ack()` but updates the message instead of deleting it
- Message attempt_count is incremented and leased_at is cleared
- Fairness key is re-added to DRR active set after nack so the message can be rescheduled
- gRPC handler follows identical pattern to ack handler
- No maximum attempt limit — immediate requeue (Lua rules engine in Epic 3 will add retry policies)

### File List

- `crates/fila-core/src/broker/scheduler.rs` — added `handle_nack()`, wired Nack command, added 5 integration tests
- `crates/fila-server/src/service.rs` — replaced nack stub with full gRPC handler
- `crates/fila-server/src/error.rs` — added IntoStatus for NackError
