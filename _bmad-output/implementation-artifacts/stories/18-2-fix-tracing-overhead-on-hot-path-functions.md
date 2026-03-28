# Story 18.2: Fix Tracing Overhead on Hot-Path Functions

Status: ready-for-dev

## Story

As an operator,
I want hot-path tracing to not Debug-format message payloads,
So that throughput improves without losing observability.

## Acceptance Criteria

1. **Given** hot-path functions in fila-server (enqueue, ack, nack, consume)
   **When** `#[instrument]` attributes are updated to use `skip_all` instead of `skip(self)`
   **Then** `cargo bench -p fila-bench --bench system` throughput is measurably higher than Story 18.1 baseline (numbers pasted in research doc and PR)

2. **Given** the tracing fix is applied
   **When** the server processes requests
   **Then** tracing still emits useful span fields (queue name, message ID, operation type) — observability is not degraded

3. **Given** all tracing changes
   **When** `cargo test` runs
   **Then** all existing tests pass (zero regressions)

## Tasks / Subtasks

- [ ] Task 1: Fix hot-path `#[instrument]` in service.rs (AC: 1, 2)
  - [ ] Change `#[instrument(skip(self), fields(...))]` to `#[instrument(skip_all, fields(...))]` on `enqueue()`
  - [ ] Change `#[instrument(skip(self), fields(...))]` to `#[instrument(skip_all, fields(...))]` on `consume()`
  - [ ] Change `#[instrument(skip(self), fields(...))]` to `#[instrument(skip_all, fields(...))]` on `ack()`
  - [ ] Change `#[instrument(skip(self), fields(...))]` to `#[instrument(skip_all, fields(...))]` on `nack()`
- [ ] Task 2: Fix admin service `#[instrument]` for consistency (AC: 2)
  - [ ] Change all 13 `#[instrument(skip(self))]` to `#[instrument(skip_all)]` in admin_service.rs (preserve existing fields)
- [ ] Task 3: Verify all tests pass (AC: 3)
  - [ ] `cargo test --workspace`
  - [ ] `cargo clippy --workspace -- -D warnings`
- [ ] Task 4: Run benchmarks and record improvement (AC: 1)
  - [ ] `cargo build --release --workspace`
  - [ ] `cargo bench -p fila-bench --bench system`
  - [ ] Update research doc with post-fix numbers and delta from baseline
  - [ ] Paste key numbers in PR description

## Dev Notes

### The Fix

Change `skip(self)` to `skip_all` on all `#[instrument]` macros. This prevents Debug-formatting of the `request` parameter (which includes protobuf message payloads) while keeping explicit `fields(...)` that are filled via `Span::current().record()`.

**Before:** `#[instrument(skip(self), fields(queue_id, msg_id))]`
**After:** `#[instrument(skip_all, fields(queue_id, msg_id))]`

### Files to Modify

- `crates/fila-server/src/service.rs` — 4 hot-path functions (lines 62, 181, 299, 391)
- `crates/fila-server/src/admin_service.rs` — 13 admin functions (for consistency)

### What NOT to Change

- Do NOT remove `#[instrument]` entirely — we want tracing spans, just without payload formatting
- Do NOT change the `fields(...)` lists — they define which fields get recorded in spans
- Do NOT touch anything in fila-core (no `#[instrument]` there)
- Do NOT modify the benchmark suite

### Baseline Numbers (from Story 18.1)

| Metric | Baseline |
|--------|----------|
| enqueue_throughput_1kb | 6,697 msg/s |
| e2e_latency_p50_light | 0.20 ms |
| e2e_latency_p99_light | 0.57 ms |

### References

- [Source: _bmad-output/planning-artifacts/research/tracing-hot-path-baseline.md] — baseline and analysis
- [Source: crates/fila-server/src/service.rs] — hot-path functions
- [Source: crates/fila-server/src/admin_service.rs] — admin functions

## Dev Agent Record

### Agent Model Used

### Debug Log References

### Completion Notes List

### File List
