# Story 30.4: Unified Enqueue Handler — Throughput Fix

Status: review

## Story

As a developer,
I want the unified enqueue handler to submit batches to the scheduler as a single Enqueue command,
so that the sequential per-message round-trip bottleneck (328 msg/s = 3ms x N) is eliminated.

## Acceptance Criteria

1. Non-cluster enqueue handler submits all messages as a single `SchedulerCommand::Enqueue`
2. ACL checks performed per-message; passing messages batched into single scheduler command
3. Cluster mode falls back to per-message processing (documented)
4. StreamEnqueue continues to process per-stream-write (already batched by stream coalescing)
5. All existing tests pass

## Tasks / Subtasks

- [x] Task 1: Extract `prepare_enqueue_message` (ACL + Message construction, no scheduler send)
- [x] Task 2: Rewrite `enqueue()` handler for single-command batch submission
- [x] Task 3: Merge ACL failures and scheduler results by slot position
- [x] Task 4: Cluster mode documented as per-message fallback

## Dev Agent Record

### Agent Model Used

Claude Opus 4.6 (1M context)

### Completion Notes List

- `prepare_enqueue_message` extracted for ACL+construction without scheduler round-trip
- Non-cluster: single `SchedulerCommand::Enqueue` for all valid messages in the batch
- Cluster: per-message `ClusterRequest::Enqueue` (Raft consensus per-message, batch optimization future work)
- StreamEnqueue unchanged — already coalesces via drain loop
- Result slot merging preserves input order with mixed ACL failures and scheduler results

### File List

- crates/fila-server/src/service.rs
