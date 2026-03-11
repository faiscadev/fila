# Story 13.2: Phase 2 Viability Seams

Status: review

## Story

As a developer,
I want thin architectural seams that enable future hierarchical queue scaling,
so that phase 2 (splitting hot queues across multiple Raft groups) is a matter of implementing new logic behind existing interfaces, not rearchitecting the core.

## Acceptance Criteria

1. A routing indirection layer maps `(queue, fairness_key)` → `RaftGroup` — in phase 1 the implementation is trivial (every fairness key in a queue maps to the same group, 1:1), but the indirection exists in the code path.
2. DRR is scoped to a key-set parameter: the scheduler runs DRR over "the fairness keys I'm responsible for" — in phase 1 this happens to be all keys in the queue, but the scope is explicit, not hardcoded.
3. Each queue emits aggregate scheduling stats as OTel metrics: messages scheduled per fairness key, current deficit state, throughput — in phase 1 these are consumed only for observability.
4. The enqueue path threads `fairness_key` through the routing decision: routing is by `(queue, fairness_key)`, not just queue — in phase 1 the fairness key is ignored in routing (all go to the same group).
5. All 278 tests + 11 e2e tests pass with zero behavioral changes.
6. No speculative abstractions or premature engineering — these are thin seams, not full implementations.

## Tasks / Subtasks

- [x] Task 1: Create `QueueRouter` type with trivial 1:1 routing (AC: 1, 4)
  - [x] 1.1: Define `QueueRouter` in `crates/fila-core/src/broker/router.rs` with a `route(queue_id, fairness_key) -> GroupId` method that always returns the same group
  - [x] 1.2: Define `GroupId` as a newtype over `String` (or `u64`) — in phase 1, `GroupId` is just the `queue_id`
  - [x] 1.3: Wire `QueueRouter` into `Scheduler` — enqueue path calls `router.route(queue_id, fairness_key)` before storage write (result is currently unused beyond the seam existing)
  - [x] 1.4: Add unit test: `route()` returns same group for all fairness keys in same queue

- [x] Task 2: Make DRR key-set scope explicit (AC: 2)
  - [x] 2.1: Review `DrrScheduler` — the key-set is already implicit (only operates on `add_key`'d keys). Document this property with a doc comment clarifying the phase 2 intent
  - [x] 2.2: If `drr_deliver_queue` pulls keys from any source other than what was explicitly added, refactor to use only the explicit key-set. (Expected: already clean — verify and document)

- [x] Task 3: Emit aggregate scheduling stats as OTel metrics (AC: 3)
  - [x] 3.1: Review existing metrics — `fila.fairness.throughput` (per queue_id + fairness_key) and `fila.fairness.fair_share_ratio` already exist. `key_stats()` method on DrrScheduler exists.
  - [x] 3.2: Add `fila.scheduler.drr.deficit` gauge with `(queue_id, fairness_key)` dimensions — exposes current deficit state per key
  - [x] 3.3: Add `fila.scheduler.drr.weight` gauge with `(queue_id, fairness_key)` dimensions — exposes current weight per key
  - [x] 3.4: Wire new metrics into `record_gauges()` in `metrics_recording.rs`
  - [x] 3.5: Add metric tests verifying the new gauges report correct values

- [x] Task 4: Thread fairness_key through enqueue routing decision (AC: 4)
  - [x] 4.1: In `handle_enqueue`, call `self.router.route(&message.queue_id, &message.fairness_key)` — the result is the `GroupId` (phase 1: always same group, no behavior change)
  - [x] 4.2: Log the routing decision at debug level for observability

- [x] Task 5: Verify all tests pass with zero behavioral changes (AC: 5, 6)
  - [x] 5.1: `cargo test --workspace` — all tests pass
  - [x] 5.2: `cargo clippy --workspace` — zero new warnings
  - [x] 5.3: e2e test suite — all 11 tests pass

## Dev Notes

### Current State Analysis

The codebase is already well-positioned for these seams:

**DRR is already implicitly scoped (Constraint 2 is nearly free):**
`DrrScheduler` operates exclusively on keys registered via `add_key()`. It has no mechanism to "discover" fairness keys from storage or queue config. Task 2 is primarily a documentation/verification task.

**Scheduling stats already partially exist (Constraint 3 is incremental):**
- `fila.fairness.throughput` with `(queue_id, fairness_key)` — messages delivered per key ✓
- `fila.fairness.fair_share_ratio` with `(queue_id, fairness_key)` — delivery proportion ✓
- `DrrScheduler::key_stats(queue_id)` returns `Vec<(key, deficit, weight)>` ✓
- Missing: deficit and weight as OTel gauges (a coordinator would need these)

**No routing concept exists (Constraints 1 and 4 need work):**
- The enqueue path writes directly to `self.storage` with no intermediate routing step
- There is no `QueueRouter`, `GroupId`, or placement table concept anywhere
- A TODO comment exists in `handle_create_queue` for the cluster transition

### QueueRouter Design

Keep it minimal — a thin seam, not a full routing layer:

```rust
/// Phase 1: trivial 1:1 routing. Every fairness key maps to the queue itself.
/// Phase 2: consistent hashing across Raft groups.
pub struct QueueRouter;

/// Identifies a Raft group. In phase 1, this is always the queue_id.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct GroupId(pub String);

impl QueueRouter {
    pub fn route(&self, queue_id: &str, _fairness_key: &str) -> GroupId {
        GroupId(queue_id.to_string())
    }
}
```

The seam exists in the code path. Phase 2 replaces the implementation with a placement table lookup and consistent hashing.

### What NOT to Do

- Do NOT add a placement table, consistent hashing, or any distribution logic — phase 1 is 1:1.
- Do NOT add configuration for routing — this is a code seam, not a user-facing feature.
- Do NOT change DRR's internal data structures — it's already clean.
- Do NOT add async anywhere — scheduler runs synchronously.
- Do NOT over-engineer the `GroupId` type — a simple newtype is enough.
- Do NOT add metrics for things a coordinator doesn't need. Only expose deficit and weight, which are the coordinator's inputs for weight adjustment.

### Key Files to Modify

| File | Change |
|------|--------|
| `crates/fila-core/src/broker/router.rs` | **NEW** — `QueueRouter` and `GroupId` |
| `crates/fila-core/src/broker/mod.rs` | Add `mod router;` |
| `crates/fila-core/src/broker/scheduler/mod.rs` | Add `router: QueueRouter` field |
| `crates/fila-core/src/broker/scheduler/handlers.rs` | Call `router.route()` in enqueue path |
| `crates/fila-core/src/broker/drr.rs` | Doc comment clarifying key-set scoping for phase 2 |
| `crates/fila-core/src/broker/metrics.rs` | Add deficit and weight gauge instruments |
| `crates/fila-core/src/broker/scheduler/metrics_recording.rs` | Wire new gauges into `record_gauges()` |

### References

- [Source: _bmad/docs/research/decoupled-scheduler-sharded-storage.md#phase-2-viability-constraints] — Four constraints and rationale
- [Source: crates/fila-core/src/broker/drr.rs] — DRR engine (already parameterized by key-set)
- [Source: crates/fila-core/src/broker/scheduler/handlers.rs] — Enqueue path (no routing today)
- [Source: crates/fila-core/src/broker/metrics.rs] — 14 existing metric instruments
- [Source: crates/fila-core/src/broker/scheduler/metrics_recording.rs] — `record_gauges()` function
- [Source: _bmad-output/planning-artifacts/epics.md#story-13.2] — Epic ACs

## Dev Agent Record

### Agent Model Used

Claude Opus 4.6

### Debug Log References

### Completion Notes List

- QueueRouter kept intentionally simple (struct, not trait) — phase 2 replaces implementation, not interface
- DRR key-set scoping was already clean; task 2 was documentation-only
- Existing `key_stats()` call in `record_gauges()` reused for deficit/weight gauges — no extra iteration
- Routing result (`GroupId`) is computed and logged but not consumed in phase 1 — the code seam exists for phase 2

### File List

- `crates/fila-core/src/broker/router.rs` — NEW: QueueRouter and GroupId types with trivial 1:1 routing
- `crates/fila-core/src/broker/mod.rs` — Added `pub mod router;`
- `crates/fila-core/src/broker/scheduler/mod.rs` — Added `router: QueueRouter` field and initialization
- `crates/fila-core/src/broker/scheduler/handlers.rs` — Routing call and debug log in enqueue path
- `crates/fila-core/src/broker/drr.rs` — Phase 2 design property doc comment
- `crates/fila-core/src/broker/metrics.rs` — `drr_deficit` (i64) and `drr_weight` (u64) gauges, recording methods, test harness helpers, 2 tests
- `crates/fila-core/src/broker/scheduler/metrics_recording.rs` — Wired deficit/weight gauges into `record_gauges()` loop
