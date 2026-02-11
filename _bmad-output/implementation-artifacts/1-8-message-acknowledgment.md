# Story 1.8: Message Acknowledgment

Status: in-progress

## Story

As a consumer,
I want to acknowledge successfully processed messages,
So that they are permanently removed from the queue.

## Acceptance Criteria

1. Given a consumer has leased a message, when the consumer calls the `Ack` RPC with the queue name and message ID, the message is removed from the `messages` CF
2. The lease is removed from the `leases` CF
3. The lease expiry entry is removed from the `lease_expiry` CF
4. All three removals happen atomically via WriteBatch
5. Acknowledging an unknown message ID returns `NOT_FOUND` status (idempotent)
6. Acknowledging a message on a non-existent queue returns `NOT_FOUND` status
7. An integration test verifies the full lifecycle: enqueue -> lease -> ack -> verify message is gone
8. An integration test verifies that acking the same message twice returns `NOT_FOUND` on the second call

## Tasks / Subtasks

- [x] Task 1: Add lease value parsing (AC: #3)
  - [x] 1.1 Add `parse_expiry_from_lease_value()` to keys.rs
- [x] Task 2: Implement ack handler in scheduler (AC: #1, #2, #3, #4, #5)
  - [x] 2.1 `handle_ack`: look up lease, parse expiry, find message key, delete all via WriteBatch
  - [x] 2.2 `find_message_key`: scan messages CF by queue prefix to find full key
  - [x] 2.3 Return `MessageNotFound` if lease doesn't exist
- [x] Task 3: Implement Ack gRPC RPC (AC: #5, #6)
  - [x] 3.1 Parse UUID from string message_id, validate inputs
  - [x] 3.2 Send Ack command to scheduler, await reply, map errors
- [x] Task 4: Tests (AC: #5, #7, #8)
  - [x] 4.1 Full lifecycle: enqueue -> lease -> ack -> verify message/lease deleted
  - [x] 4.2 Ack unknown message returns MessageNotFound
  - [x] 4.3 Double ack returns MessageNotFound on second call
  - [x] 4.4 Update old stub test (ack_reply_received -> ack_without_lease_returns_error)
