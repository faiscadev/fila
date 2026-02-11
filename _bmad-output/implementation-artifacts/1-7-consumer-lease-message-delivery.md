# Story 1.7: Consumer Lease & Message Delivery

Status: in-progress

## Story

As a consumer,
I want to receive ready messages from a queue via a streaming connection,
So that I can process messages as they become available without polling.

## Acceptance Criteria

1. Given messages have been enqueued to a queue, when a consumer calls the `Lease` RPC with a queue name, a server-streaming gRPC connection is established
2. The scheduler registers the consumer with a per-consumer `mpsc::Sender`
3. Pending messages are delivered to the consumer through the stream
4. Each delivered message includes: message ID, headers, payload, metadata (fairness_key, attempt count)
5. A lease is created in the `leases` CF with `{queue_id}:{msg_id}` -> `{consumer_id}:{expiry_ts_ns}`
6. A lease expiry entry is created in the `lease_expiry` CF with `{expiry_ts_ns}:{queue_id}:{msg_id}`
7. The visibility timeout is configurable per queue (default 30 seconds)
8. When the consumer disconnects, the scheduler unregisters the consumer
9. Multiple consumers can lease from the same queue simultaneously (each gets different messages)
10. An integration test enqueues 10 messages, opens a lease stream, and receives all 10 messages

## Tasks / Subtasks

- [x] Task 1: Update SchedulerCommand for tokio channels (AC: #2)
  - [x] 1.1 Change `RegisterConsumer.tx` from `crossbeam_channel::Sender` to `tokio::sync::mpsc::Sender`
- [x] Task 2: Implement consumer registry in scheduler (AC: #2, #3, #8, #9)
  - [x] 2.1 Add `consumers: HashMap<String, ConsumerEntry>` to Scheduler
  - [x] 2.2 Implement `handle_register_consumer`: insert entry, deliver pending messages
  - [x] 2.3 Implement `handle_unregister_consumer`: remove entry
- [x] Task 3: Implement message delivery logic (AC: #3, #5, #6, #7, #9)
  - [x] 3.1 Add `try_deliver_pending(queue_id)`: scan messages without leases, assign to consumers
  - [x] 3.2 Round-robin delivery across multiple consumers for same queue
  - [x] 3.3 Create lease + lease_expiry atomically via `write_batch`
  - [x] 3.4 Use queue's `visibility_timeout_ms` for lease expiry calculation
  - [x] 3.5 Call `try_deliver_pending` after each enqueue and after consumer registration
- [x] Task 4: Implement Lease gRPC RPC (AC: #1, #4, #8)
  - [x] 4.1 Two-channel bridge: scheduler sends `ReadyMessage`, converter task maps to `LeaseResponse`
  - [x] 4.2 `tokio::select!` detects both message arrival and client disconnection
  - [x] 4.3 Sends `UnregisterConsumer` on stream close
  - [x] 4.4 `ready_to_proto` converts `ReadyMessage` to protobuf `Message` with metadata
- [x] Task 5: Remove dead_code attributes from now-used key functions
- [x] Task 6: Tests (AC: #3, #5, #8, #9, #10)
  - [x] 6.1 Consumer receives enqueued messages
  - [x] 6.2 Consumer receives pending messages on registration
  - [x] 6.3 Lease creates entries in storage (leases CF)
  - [x] 6.4 Multiple consumers get different messages
  - [x] 6.5 Unregister stops delivery
  - [x] 6.6 Enqueue 10 messages, lease receives all 10

## Dev Notes

- Used `tokio::sync::mpsc` instead of `crossbeam` for consumer channels since tokio `Sender::try_send()` is safe to call from non-async context and simplifies the gRPC streaming bridge
- Converter task uses `tokio::select!` on both `ready_rx.recv()` and `stream_tx.closed()` to handle client disconnection even when no messages are pending
- `try_deliver_pending` does a full scan of messages CF for the queue — acceptable for MVP, will be optimized with DRR in story 2.1
