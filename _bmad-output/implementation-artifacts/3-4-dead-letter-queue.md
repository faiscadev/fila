# Story 3.4: Dead-Letter Queue

Status: ready-for-dev

## Story

As an operator,
I want failed messages to be automatically routed to a dead-letter queue,
so that I can inspect and potentially redrive unprocessable messages.

## Acceptance Criteria

1. **Given** a queue is created, **when** queue creation completes, **then** a corresponding dead-letter queue is automatically created with the name `{queue_name}.dlq` and the DLQ queue ID is stored in the parent queue's `dlq_queue_id` field
2. **Given** a queue name that already ends with `.dlq`, **when** queue creation completes, **then** no DLQ-of-a-DLQ is created (DLQ queues do not get their own DLQ)
3. **Given** the on_failure Lua script returns `action = "dlq"` for a nacked message, **when** the scheduler processes the nack, **then** the message is moved to the DLQ via atomic WriteBatch (already implemented in Story 3.3)
4. **Given** a DLQ contains messages, **when** a consumer leases from the DLQ, **then** DLQ messages are delivered like any other queue's messages
5. **Given** a queue is deleted, **when** the queue had an auto-created DLQ, **then** the DLQ is also deleted
6. **Given** the broker restarts, **when** recovery runs, **then** the auto-created DLQ and its `dlq_queue_id` reference survive (persisted in queue config)

## Tasks / Subtasks

- [ ] Task 1: Update `handle_create_queue` to auto-create `{queue_name}.dlq` and set `dlq_queue_id` (AC: #1, #2)
- [ ] Task 2: Update `handle_delete_queue` to also delete the auto-created DLQ (AC: #5)
- [ ] Task 3: Integration tests for full DLQ flow (AC: #1, #3, #4, #5, #6)

## Dev Notes

### Key Design Decisions

- DLQ auto-creation happens inside `handle_create_queue` after the parent queue is persisted
- DLQ queues are regular queues — no special behavior. They don't get on_enqueue/on_failure scripts
- Queues ending in `.dlq` are treated as DLQ queues and don't get their own DLQ (prevents infinite chain)
- The `dlq_queue_id` field in `QueueConfig` is set automatically; manual configuration is no longer needed
- DLQ message routing was already implemented in Story 3.3's `handle_nack`

### Implementation Notes

- **`handle_create_queue`**: After `storage.put_queue(name, config)`, if the queue name doesn't end with `.dlq`:
  1. Create a DLQ `QueueConfig` with name `{name}.dlq`
  2. Store it via `storage.put_queue`
  3. Update the parent config's `dlq_queue_id` to `{name}.dlq`
  4. Re-persist the parent config with the updated `dlq_queue_id`
- **`handle_delete_queue`**: If the queue has a `dlq_queue_id`, also delete that queue from storage and clean up DRR/Lua state
- Admin API (`admin_service.rs`) should NOT need changes — the DLQ creation is transparent to the API caller
- Recovery doesn't need changes — `dlq_queue_id` is persisted in `QueueConfig`, so it survives restarts

### File Structure

```
crates/fila-core/src/broker/scheduler.rs  # handle_create_queue, handle_delete_queue changes
```

## Post-PR Review Fixes

- **Cubic P2 — DLQ-of-DLQ not prevented** (identified by cubic): `.dlq` queues did not have `dlq_queue_id` cleared before persisting, so callers could still configure a nested DLQ contradicting the stated behavior. Fixed by explicitly clearing `dlq_queue_id = None` for `.dlq` queues in `handle_create_queue`.
- **Cubic P2 — cascade-delete too aggressive** (identified by cubic): `handle_delete_queue` blindly deleted whatever `dlq_queue_id` pointed to, including custom or shared DLQs. Fixed by only cascade-deleting when `dlq_queue_id` matches the auto-created naming convention (`{queue_id}.dlq`).
- **Dev agent miss**: cubic's automated review flagged both issues on the PR, but the dev agent did not check cubic's review findings before marking the story complete.
