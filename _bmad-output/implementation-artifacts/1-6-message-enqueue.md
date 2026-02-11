# Story 1.6: Message Enqueue

Status: in-progress

## Story

As a producer,
I want to enqueue messages with headers and payload to a named queue,
So that my messages are durably stored and available for consumers.

## Acceptance Criteria

1. Given a queue has been created, when a producer calls the `Enqueue` RPC with a queue name, headers map, and payload bytes, the scheduler assigns a UUIDv7 message ID and a default fairness key (value `"default"`)
2. The message is persisted to the `messages` CF with the composite key format (`{queue_id}:{fairness_key}:{enqueued_at}:{msg_id}`)
3. The `EnqueueResponse` returns the assigned message ID
4. Calling `Enqueue` on a non-existent queue returns `NOT_FOUND` status
5. Headers can contain arbitrary string key-value pairs
6. Payload is stored as opaque bytes (broker never inspects content)
7. An integration test enqueues 100 messages and verifies all are persisted with unique, time-ordered IDs

## Tasks / Subtasks

- [x] Task 1: Implement enqueue handler in scheduler (AC: #1, #2)
  - [x] 1.1 Add `handle_enqueue` method to Scheduler that validates queue existence
  - [x] 1.2 Construct composite message key using `storage::keys::message_key()`
  - [x] 1.3 Persist message via `storage.put_message()`
  - [x] 1.4 Make `keys` module `pub(crate)` so scheduler can access key functions
- [x] Task 2: Implement gRPC Enqueue RPC (AC: #1, #3, #5, #6)
  - [x] 2.1 Create `crates/fila-server/src/service.rs` implementing `FilaService`
  - [x] 2.2 Build `Message` from `EnqueueRequest` with UUIDv7 ID, default fairness key, nanosecond timestamp
  - [x] 2.3 Send `SchedulerCommand::Enqueue` via broker, await reply, map errors to gRPC status
  - [x] 2.4 Stub Lease/Ack/Nack RPCs as UNIMPLEMENTED
- [x] Task 3: Register hot-path service in server binary (AC: #1)
  - [x] 3.1 Add `mod service;` and `FilaServiceServer` to `main.rs` gRPC server builder
  - [x] 3.2 Add missing dependencies (uuid, tokio-stream) to fila-server
- [x] Task 4: Tests (AC: #4, #7)
  - [x] 4.1 Test enqueue to non-existent queue returns `QueueNotFound`
  - [x] 4.2 Test enqueue persists message and can be read back from storage
  - [x] 4.3 Test enqueue 100 messages with unique, time-ordered UUIDv7 IDs
  - [x] 4.4 Update existing enqueue tests to create queue first (queue validation now enforced)

## Dev Notes

- `service.rs` uses `HotPathService` naming to distinguish from the admin service
- `fila_error_to_status()` is duplicated between `admin_service.rs` and `service.rs` — could be extracted to a shared module in a future refactor
- Channel capacity in test setup increased from 100 to 256 to accommodate the 100-message bulk test
