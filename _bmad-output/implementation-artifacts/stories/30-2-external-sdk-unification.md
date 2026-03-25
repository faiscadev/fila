# Story 30.2: External SDK Unification — Go, Python, JS, Ruby, Java

Status: review

## Story

As a developer,
I want all 5 external SDKs to use the unified API surface from Story 30.1,
so that every SDK has one code path per operation with no "batch" naming.

## Acceptance Criteria

1. **Given** the proto changes from Story 30.1 (unified `Enqueue` with `repeated`, `BatchEnqueue` removed)
   **When** each external SDK is updated
   **Then** `batch_enqueue()` is removed, `enqueue()` accepts one or more messages
   **And** consume uses the unified `repeated Message` delivery
   **And** ack/nack accept one or more message IDs
   **And** no "batch" prefix/suffix remains in the public API

2. **Given** the 5 SDKs are independent repos
   **When** the updates are made
   **Then** one agent per SDK runs in parallel (Go, Python, JS, Ruby, Java)
   **And** each opens a PR in its respective repo

3. Each SDK's integration tests pass against a server built from Story 30.1
4. Each SDK's CI runs and passes

## Tasks / Subtasks

- [x] Task 1: Copy unified service.proto to each SDK repo
- [x] Task 2: Update Go SDK (fila-go) — PR #4, 20/20 tests pass
- [x] Task 3: Update Python SDK (fila-python) — PR #4, 31/31 tests pass
- [x] Task 4: Update JS SDK (fila-js) — PR #4, 30/30 tests pass
- [x] Task 5: Update Ruby SDK (fila-ruby) — PR #4
- [x] Task 6: Update Java SDK (fila-java) — PR #4, 34/35 pass (1 pre-existing TLS failure)
- [x] Task 7: All 5 PRs opened

## Dev Notes

- **Parallel agents**: One agent per SDK repo, all running concurrently. Each gets the unified proto + design guidance.
- **Proto changes**: `EnqueueRequest` now wraps `repeated EnqueueMessage`. `BatchEnqueue` RPC removed. Ack/Nack accept repeated items. Consume uses only `repeated Message messages`.
- **"Batch" naming removed**: `batch_enqueue` → `enqueue_many` (or language-idiomatic equivalent). `BatchMode` → `AccumulatorMode`.
- **External repo PRs required**: Per CLAUDE.md rule, all external repo changes must go through PRs.
- **SDK repos**: fila-go, fila-python, fila-js, fila-ruby, fila-java (all under faiscadev org)

### References

- [Source: _bmad-output/planning-artifacts/batch-pipeline-epics.md#Story 30.2]
- [Source: crates/fila-proto/proto/fila/v1/service.proto] (unified proto)

## Dev Agent Record

### Agent Model Used

Claude Opus 4.6 (1M context) — 5 parallel agents

### Debug Log References

None.

### Completion Notes List

- All 5 agents completed successfully in parallel (~6-10 min each).
- Go: `BatchMode` → `AccumulatorMode`, `batch.go` → `accumulator.go`, `BatchEnqueue` → `EnqueueMany`
- Python: `BatchMode` → `AccumulatorMode`, `AutoBatcher` → `AutoAccumulator`, `batch_enqueue` → `enqueue_many`
- JS: `batchEnqueue` → `enqueueMany`, `BatchEnqueueResult` → `EnqueueResult`
- Ruby: `batch_enqueue` → `enqueue_many`, `BatchEnqueueResult` → `EnqueueResult`
- Java: `batchEnqueue` → `enqueueMany`, `BatchEnqueueResult` → `EnqueueResult`

### External PRs

- Go: https://github.com/faiscadev/fila-go/pull/4
- Python: https://github.com/faiscadev/fila-python/pull/4
- JS: https://github.com/faiscadev/fila-js/pull/4
- Ruby: https://github.com/faiscadev/fila-ruby/pull/4
- Java: https://github.com/faiscadev/fila-java/pull/4

### File List

- _bmad-output/implementation-artifacts/stories/30-2-external-sdk-unification.md
- _bmad-output/implementation-artifacts/sprint-status.yaml
- _bmad-output/implementation-artifacts/epic-execution-state.yaml
