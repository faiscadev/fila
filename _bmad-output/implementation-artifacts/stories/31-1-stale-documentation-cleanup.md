# Story 31.1: Stale Documentation Cleanup — Post-Unification Docs Update

Status: ready-for-dev

## Story

As a developer or evaluator,
I want documentation to accurately reflect the current unified API surface,
So that I can trust the docs when integrating with Fila or evaluating its capabilities.

## Acceptance Criteria

1. **Given** `docs/api-reference.md` describes a single-message `Enqueue` RPC with `queue`, `headers`, `payload` fields
   **When** the docs are updated
   **Then** the Enqueue RPC documentation shows `repeated EnqueueMessage messages` request format
   **And** the response shows `repeated EnqueueResult results` with `oneof { message_id, EnqueueError }` structure
   **And** the `EnqueueErrorCode` enum is documented with all variants (`QUEUE_NOT_FOUND`, `STORAGE`, `LUA`, `PERMISSION_DENIED`)
   **And** `AckRequest` and `NackRequest` are documented with `repeated` items and typed error codes (`AckErrorCode`, `NackErrorCode`)
   **And** `ConsumeResponse` is documented with `repeated Message messages` only (no singular `message` field)
   **And** `StreamEnqueue` RPC is documented (bidirectional streaming with sequence tracking)

2. **Given** `docs/profiling.md` references a `batch-enqueue` workload and `BatchEnqueue` RPC path
   **When** the docs are updated
   **Then** all references to the removed `BatchEnqueue` RPC are replaced with the unified `Enqueue` RPC (with multiple messages)
   **And** workload names and make targets reflect current profiling capabilities

3. **Given** `docs/benchmarks.md` has a "Batch benchmarks" section referencing `BatchEnqueue` RPC and `BatchMode::Auto`
   **When** the docs are updated
   **Then** the "Batch benchmarks" section is rewritten to reference multi-message Enqueue throughput (not a separate RPC)
   **And** all references to `BatchMode::Auto` are replaced with `AccumulatorMode` or removed
   **And** empty placeholder benchmark tables are either populated with current data or removed

4. **Given** the docs are updated
   **When** reviewed against `crates/fila-proto/proto/fila/v1/service.proto` and `messages.proto`
   **Then** every RPC, message type, and field name in the docs matches the current proto definitions
   **And** no references to removed types (`BatchEnqueueRequest`, `BatchEnqueueResponse`, `BatchEnqueueResult`, `BatchMode`) remain in any docs file

## Tasks / Subtasks

- [ ] Task 1: Update `docs/api-reference.md`
  - [ ] 1.1 Rewrite Enqueue RPC section with unified `repeated EnqueueMessage` request/response
  - [ ] 1.2 Add `StreamEnqueue` RPC documentation
  - [ ] 1.3 Update Ack/Nack RPC sections with repeated items and typed error codes
  - [ ] 1.4 Update Consume RPC section with `repeated Message messages` only
  - [ ] 1.5 Add `EnqueueErrorCode`, `AckErrorCode`, `NackErrorCode` enum documentation

- [ ] Task 2: Update `docs/profiling.md`
  - [ ] 2.1 Replace `batch-enqueue` workload references with current names
  - [ ] 2.2 Remove references to removed `BatchEnqueue` RPC

- [ ] Task 3: Update `docs/benchmarks.md`
  - [ ] 3.1 Rewrite "Batch benchmarks" section for unified Enqueue
  - [ ] 3.2 Replace `BatchMode::Auto` references with `AccumulatorMode`
  - [ ] 3.3 Clean up empty placeholder tables

- [ ] Task 4: Cross-check all docs against current proto definitions
  - [ ] 4.1 Grep all docs/ for removed type names
  - [ ] 4.2 Verify RPC signatures match service.proto

## Dev Notes

### Context

Epic 30 unified the API surface: `BatchEnqueue` RPC was removed, `Enqueue` now accepts `repeated EnqueueMessage`, ack/nack accept repeated items, consume delivers via `repeated Message` only. `BatchMode` was renamed to `AccumulatorMode`. These changes left 3 doc files stale.

### Key Files to Modify

| File | Change |
|------|--------|
| `docs/api-reference.md` | Rewrite Enqueue/Ack/Nack/Consume sections, add StreamEnqueue |
| `docs/profiling.md` | Replace BatchEnqueue references (lines ~59, 64, 78) |
| `docs/benchmarks.md` | Rewrite batch section (lines ~125-217), remove BatchMode::Auto (line ~319) |

### References

- [Source: crates/fila-proto/proto/fila/v1/service.proto] — Current RPC definitions
- [Source: crates/fila-proto/proto/fila/v1/messages.proto] — Current message type definitions
- [Story: 30-1-api-surface-unification.md] — The story that created the stale state
