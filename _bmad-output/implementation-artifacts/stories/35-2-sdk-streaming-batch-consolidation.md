# Story 35.2: SDK Streaming Batch Consolidation

Status: ready-for-dev

## Story

As a developer,
I want the SDK to send accumulated messages as a single StreamEnqueueRequest per batch (not one request per message),
So that HTTP/2 DATA frame overhead is amortized across the batch instead of paid per message.

## Acceptance Criteria

1. **Given** the SDK's `StreamManager::send_batch()` currently sends one `StreamEnqueueRequest` per message
   **When** the SDK accumulates N messages (via Auto or Linger mode)
   **Then** `send_batch()` sends a single `StreamEnqueueRequest` containing all N messages with one sequence number

2. **Given** a single `StreamEnqueueRequest` with N messages
   **When** the server processes it
   **Then** the response maps the single sequence number back to per-message results
   **And** each result is delivered to the correct caller's oneshot channel

3. **Given** the SDK's Auto accumulator
   **When** using default settings
   **Then** `max_batch_size` is 100 (unchanged)

4. **Given** the SDK's Linger accumulator
   **When** using default settings
   **Then** `linger_ms` defaults to 5ms and `batch_size` defaults to 100

5. **Given** existing SDK tests
   **When** the consolidation is implemented
   **Then** all existing tests pass without modification

6. **Given** a Linger-mode client sending 1000 messages over 2 seconds
   **When** consolidation is active
   **Then** fewer than 50 StreamEnqueueRequests are produced (proving consolidation)

## Tasks / Subtasks

- [ ] Task 1: Modify `send_batch()` to send a single StreamEnqueueRequest per batch (AC: 1)
- [ ] Task 2: Modify `resolve_stream_response()` to handle multiple results per response (AC: 2)
- [ ] Task 3: Update pending map to store Vec of senders per sequence number (AC: 2)
- [ ] Task 4: Add Linger mode defaults (linger_ms=5, batch_size=100) (AC: 4)
- [ ] Task 5: Add integration test for consolidation verification (AC: 6)
- [ ] Task 6: Verify all existing tests pass (AC: 5)
- [ ] Task 7: Update docs/configuration.md if defaults changed (AC: 4)

## Dev Notes

- Key file: `crates/fila-sdk/src/client.rs`
- `send_batch()` at line ~668: currently creates one StreamEnqueueRequest per message
- `resolve_stream_response()` at line ~739: currently takes first result only
- Proto `StreamEnqueueRequest.messages` is `repeated` — server already handles multi-message requests
- Server handler at `crates/fila-server/src/service.rs:531` already processes all messages correctly
- The pending map currently maps `seq -> oneshot::Sender` for single results. Need to change to `seq -> Vec<oneshot::Sender>` for multi-message batches.
- Linger default 5ms matches Kafka's `linger.ms=5` default

### References

- [Source: crates/fila-sdk/src/client.rs]
- [Source: crates/fila-server/src/service.rs]
- [Source: proto/fila/v1/service.proto]

## Dev Agent Record

### Agent Model Used

Claude Opus 4.6 (1M context)

### Debug Log References

### Completion Notes List

### File List
