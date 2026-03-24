# Story 23.1: Client-Side Batching & BatchEnqueue RPC

Status: ready-for-dev

## Story

As a developer using the Fila SDK,
I want to configure client-side message batching with linger time and batch size limits,
So that high-throughput producers can amortize per-message overhead without changing application code.

## Acceptance Criteria

1. **Given** the current SDK sends one message per `enqueue()` call
   **When** batching is configured on the SDK client
   **Then** a new `BatchEnqueue` RPC is added to the proto definition that accepts `repeated EnqueueRequest` and returns `repeated EnqueueResponse`

2. **And** the server-side handler processes all messages in the batch, with each message independently validated — invalid messages get individual error responses without failing the batch

3. **And** per-queue ordering within a batch is preserved (messages to the same queue appear in batch order)

4. **And** the Rust SDK (`fila-sdk`) adds `BatchConfig` with: `linger_ms: Option<u64>` (time threshold, default None = disabled), `batch_size: Option<usize>` (max messages per batch, default 100)

5. **And** when batching is disabled (default), the SDK uses the existing single-message `Enqueue` RPC — zero behavior change

6. **And** all existing tests pass (single-message path unchanged)

7. **And** new integration tests verify: batch enqueue correctness (all messages stored), partial failure handling (one bad message doesn't fail the batch), ordering preserved

## Tasks / Subtasks

- [ ] Task 1: Add `BatchEnqueue` RPC, `BatchEnqueueRequest`, `BatchEnqueueResponse`, `BatchEnqueueResult` to service.proto
- [ ] Task 2: Rebuild protos (`cargo build -p fila-proto`)
- [ ] Task 3: Implement server-side `batch_enqueue` handler in `HotPathService`
- [ ] Task 4: Add `BatchConfig` struct and `batch_enqueue()` method to fila-sdk
- [ ] Task 5: Add `BatchEnqueueError` to SDK error types
- [ ] Task 6: Add integration tests for batch enqueue (correctness, partial failure, ordering)
- [ ] Task 7: Verify all existing tests pass

## Dev Notes

- Server handler processes each message independently through the normal scheduler path. Write coalescing (single WriteBatch) is Story 23.2.
- SDK exposes `batch_enqueue(messages: Vec<...>)` as a direct method. Background linger/auto-flush batching can be added as a follow-up.
- When batching is disabled (default), `enqueue()` uses the existing single-message RPC path.
