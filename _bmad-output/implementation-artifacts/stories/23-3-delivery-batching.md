# Story 23.3: Delivery Batching (Consumer-Side)

Status: review

## Story

As a Fila consumer,
I want the server to batch multiple ready messages into a single gRPC stream frame,
So that delivery throughput is higher by amortizing HTTP/2 framing and protobuf encoding overhead.

## Acceptance Criteria

1. **Given** multiple messages are ready for delivery in a queue
   **When** the server delivers to a consumer
   **Then** it collects up to `delivery_batch_max_messages` immediately-available messages and sends them in a single `ConsumeResponse` using the `messages` repeated field

2. **And** when only one message is ready, the existing `message` field is used (backward compatible with older SDKs)

3. **And** each message in a batch gets an individual lease with its own visibility timeout

4. **And** the SDK consumer stream interface is unchanged -- callers receive individual `ConsumeMessage` values regardless of server-side batching

5. **And** ack/nack operates on individual message IDs within a batch (no batch-level ack)

6. **And** a new `delivery_batch_max_messages` config in `SchedulerConfig` (default 10) controls batch size

7. **And** all existing consume tests pass unchanged (backward compatible)

## Tasks / Subtasks

- [x] Task 1: Add `repeated Message messages` field to `ConsumeResponse` in service.proto
- [x] Task 2: Add `delivery_batch_max_messages` to `SchedulerConfig` (default 10)
- [x] Task 3: Implement server-side batching in the consume stream converter task
- [x] Task 4: Update SDK consumer to handle batched `messages` field
- [x] Task 5: Add e2e tests for batched delivery (3 tests)
- [x] Task 6: Run full test suite, clippy, fmt -- all pass

## Dev Notes

- The scheduler already sends individual `ReadyMessage` values through a per-consumer mpsc channel. Batching happens in the gRPC service layer: the converter task drains multiple messages from the channel when available.
- No scheduler changes needed -- batching is purely a presentation concern at the gRPC layer.
- The `delivery_batch_max_messages` config is read by the server, not the scheduler. It's placed in `SchedulerConfig` for consistency with other performance knobs.
