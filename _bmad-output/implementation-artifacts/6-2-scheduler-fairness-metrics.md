# Story 6.2: Scheduler & Fairness Metrics

Status: review

## Story

As an operator,
I want per-fairness-key throughput metrics and DRR scheduling diagnostics,
so that I can answer "why was key X delayed?" and verify fair scheduling.

## Acceptance Criteria

1. **Given** the DRR scheduler is delivering messages, **when** a message is delivered to a consumer, **then** `fila.fairness.throughput` counter is incremented with labels `queue_id` and `fairness_key`
2. **Given** the DRR scheduler is delivering messages, **when** metrics are exported, **then** `fila.fairness.fair_share_ratio` gauge shows each key's actual throughput divided by its weight-based fair share target (labels: `queue_id`, `fairness_key`)
3. **Given** the DRR scheduler completes a round, **when** `start_new_round` is called, **then** `fila.scheduler.drr.rounds` counter is incremented (label: `queue_id`)
4. **Given** the DRR scheduler has active keys, **when** metrics are exported, **then** `fila.scheduler.drr.active_keys` gauge shows the current number of active fairness keys per queue
5. **Given** the DRR scheduler processes keys, **when** `next_key` returns a key, **then** `fila.scheduler.drr.keys_processed` counter is incremented (label: `queue_id`)
6. **Given** metrics are labeled with `queue_id` and `fairness_key`, **when** an operator queries, **then** they can determine why a specific key was delayed (insufficient weight, starved by other keys, etc.)
7. **Given** a test with multiple fairness keys of different weights, **when** messages are delivered, **then** per-key throughput metrics are emitted and directionally correct (higher-weight keys get proportionally more deliveries)

## Tasks / Subtasks

- [x] Task 1: Extend Metrics struct with fairness and DRR instruments (AC: #1, #3, #4, #5)
  - [x] Subtask 1.1: Add `fairness_throughput: Counter<u64>` for `fila.fairness.throughput`
  - [x] Subtask 1.2: Add `drr_rounds: Counter<u64>` for `fila.scheduler.drr.rounds`
  - [x] Subtask 1.3: Add `drr_active_keys: Gauge<u64>` for `fila.scheduler.drr.active_keys`
  - [x] Subtask 1.4: Add `drr_keys_processed: Counter<u64>` for `fila.scheduler.drr.keys_processed`
  - [x] Subtask 1.5: Add `fairness_ratio: Gauge<f64>` for `fila.fairness.fair_share_ratio`
  - [x] Subtask 1.6: Add recording methods for each new metric
- [x] Task 2: Instrument scheduler delivery path (AC: #1, #3, #5)
  - [x] Subtask 2.1: Call `record_fairness_delivery(queue_id, fairness_key)` after successful delivery in `drr_deliver_queue`
  - [x] Subtask 2.2: Call `record_drr_round(queue_id)` in `drr_deliver_queue` when `start_new_round` is called
  - [x] Subtask 2.3: Call `record_drr_key_processed(queue_id)` when `next_key` returns a key
- [x] Task 3: Instrument gauge recording in `record_gauges` (AC: #2, #4)
  - [x] Subtask 3.1: Compute and record `drr_active_keys` per queue from `drr.key_stats()`
  - [x] Subtask 3.2: Compute and record `fairness_ratio` per key: track cumulative deliveries per key, compute ratio = actual_share / expected_share
- [x] Task 4: Extend test harness for multi-label assertions (AC: #7)
  - [x] Subtask 4.1: Add `assert_counter_with_labels` helper that matches on `queue_id` + `fairness_key`
  - [x] Subtask 4.2: Add `assert_gauge_f64` helper for f64 gauge assertions
- [x] Task 5: Integration tests (AC: #7)
  - [x] Subtask 5.1: Test fairness throughput counter increments per key
  - [x] Subtask 5.2: Test DRR rounds counter increments
  - [x] Subtask 5.3: Test active_keys gauge reflects current state
  - [x] Subtask 5.4: Test keys_processed counter increments
  - [x] Subtask 5.5: Test directional fairness — higher-weight key gets proportionally more throughput

## Dev Notes

### Architecture Compliance

- **Metric naming**: `fila.category.metric_type` with `snake_case` labels per architecture spec
- **Labels**: `queue_id` on all metrics, `fairness_key` on per-key metrics
- **Performance**: All recording is O(1) — no expensive operations on the hot path
- **Cardinality**: `fairness_key` is operator-controlled (Lua on_enqueue sets it), bounded by queue design

### Existing Infrastructure (from Story 6.1)

- **`Metrics` struct** in `broker/metrics.rs` with `new()`, `from_meter()`, and recording methods
- **`MetricTestHarness`** with `assert_counter()`, `assert_gauge()` for `queue_id`-labeled metrics
- **`record_gauges()`** in scheduler iterates `known_queues` to report 0 for idle queues
- **Delivery path** in `drr_deliver_queue()` already calls `self.metrics.record_lease(queue_id)` after successful delivery

### Key Code Locations

| Location | What Happens | Metric to Record |
|----------|--------------|------------------|
| `scheduler.rs` — `drr_deliver_queue()`, after `start_new_round()` | New DRR round begins | `fila.scheduler.drr.rounds` |
| `scheduler.rs` — `drr_deliver_queue()`, after `drr.next_key()` returns `Some` | A key is selected for processing | `fila.scheduler.drr.keys_processed` |
| `scheduler.rs` — `drr_deliver_queue()`, after `try_deliver_to_consumer()` succeeds | Message delivered | `fila.fairness.throughput` |
| `scheduler.rs` — `record_gauges()` | Periodic gauge update | `drr_active_keys`, `fairness_ratio` |

### Fair Share Ratio Calculation

For each `(queue_id, fairness_key)`:
```
total_weight = sum of all active keys' weights in this queue
fair_share = key_weight / total_weight
actual_share = key_deliveries / total_deliveries (across all keys in queue)
ratio = actual_share / fair_share
```
- `ratio = 1.0` means perfectly fair
- `ratio < 1.0` means underserved
- `ratio > 1.0` means overserved

Track cumulative deliveries per key in a `HashMap<(String, String), u64>` on the scheduler. Reset on each gauge reporting cycle to give a rolling window view.

### DRR State Access

- `self.drr.key_stats(queue_id)` returns `Vec<(key_name, deficit, weight)>` — use to count active keys and get weights
- `self.drr.has_active_keys(queue_id)` — check if queue has any active keys
- Active key count = `key_stats(queue_id).len()`

### Test Strategy

Integration tests use the `MetricTestHarness` from Story 6.1. For directional correctness:
- Create a Metrics instance from the harness
- Call recording methods simulating delivery patterns (e.g., key A with weight 3 gets ~3x deliveries vs key B with weight 1)
- Flush and assert counter values are directionally correct

### Key Files to Modify

| File | Change |
|------|--------|
| `crates/fila-core/src/broker/metrics.rs` | Add 5 new instruments, recording methods, extend test harness |
| `crates/fila-core/src/broker/scheduler.rs` | Add delivery tracking HashMap, instrument DRR delivery path, extend record_gauges |

### References

- [Source: _bmad-output/planning-artifacts/architecture.md#Observability] Metric naming conventions
- [Source: _bmad-output/planning-artifacts/epics.md:786-802] Story 6.2 AC definition
- [Source: crates/fila-core/src/broker/drr.rs] DRR scheduler state and methods
- [Source: crates/fila-core/src/broker/metrics.rs] Story 6.1 Metrics struct and test harness
- [Source: crates/fila-core/src/broker/scheduler.rs:1089-1212] DRR delivery path

## Dev Agent Record

### Agent Model Used

Claude Opus 4.6

### Completion Notes List

- All 5 tasks implemented: Metrics struct extensions, scheduler instrumentation, gauge computation, test harness, integration tests
- Code review caught MEDIUM issue: `start_new_round` was recording DRR rounds counter even when round was still active (no-op). Fixed by making `start_new_round` return `bool`.
- `record_gauges` changed from `&self` to `&mut self` to support `fairness_deliveries.clear()` per-window reset
- 6 new integration tests (15 total metric tests)
- All 259 workspace tests pass, clippy clean, fmt clean

### Change Log

- feat: add scheduler & fairness metrics (story 6.2)
- fix: start_new_round returns bool to prevent inflated rounds counter

### File List

- `crates/fila-core/src/broker/metrics.rs` — Added 5 new instruments, 5 recording methods, 2 test harness helpers, 2 extraction functions, 6 tests
- `crates/fila-core/src/broker/scheduler.rs` — Added `fairness_deliveries` HashMap, 3 metric calls in delivery path, extended `record_gauges` with active_keys + fairness_ratio
- `crates/fila-core/src/broker/drr.rs` — Changed `start_new_round` return type from `()` to `bool`
