# Story 22.2: DRR Scheduler O(1) Key Selection

Status: ready-for-dev

## Story

As a developer optimizing Fila's throughput at high fairness-key counts,
I want the DRR scheduler to select the next eligible key in O(1) instead of O(n),
so that scheduling overhead does not scale with fairness-key count.

## Acceptance Criteria

1. **Given** a DRR scheduler with N active fairness keys
   **When** `next_key()` is called
   **Then** the next eligible key is returned in O(1) time (deque pop, not linear scan)

2. **Given** the refactored DRR scheduler
   **When** all existing property-based and unit tests run
   **Then** all tests pass without modification (behavioral equivalence)

3. **Given** the refactored DRR scheduler
   **When** the public API is inspected
   **Then** no public method signatures have changed

4. **Given** a queue with keys that exhaust their deficit
   **When** `consume_deficit` completes a key's burst
   **Then** that key is not re-added to the eligible deque (only keys with remaining deficit stay eligible)

5. **Given** a new round starts via `start_new_round`
   **When** deficits are replenished
   **Then** the eligible deque is rebuilt from active keys with positive deficit

## Tasks / Subtasks

- [x] Task 1: Add `eligible_keys: VecDeque<String>` to `DrrQueueState`
- [x] Task 2: Remove `round_position: usize` field (replaced by eligible deque ordering)
- [x] Task 3: Refactor `next_key()` to pop from `eligible_keys` front
- [x] Task 4: Refactor `consume_deficit()` to manage eligible deque membership
- [x] Task 5: Refactor `start_new_round()` to rebuild eligible deque
- [x] Task 6: Refactor `drain_deficit()` to remove from eligible deque
- [x] Task 7: Refactor `remove_key()` to remove from eligible deque
- [x] Task 8: Verify all existing tests pass without modification

## Dev Notes

### Key Design Decisions

- `eligible_keys` replaces the `round_position` cursor. Instead of scanning from a position, we pop from front.
- When a key completes its burst (delivered `weight` messages), it is removed from the front of `eligible_keys` and re-appended at the back only if it still has positive deficit.
- `start_new_round` rebuilds `eligible_keys` from `active_keys` order, including only keys with positive deficit after replenishment.
- `drain_deficit` removes the key from `eligible_keys` entirely (O(n) removal but only called on throttle, not on hot path).

### References

- [Source: _bmad-output/planning-artifacts/research/performance-optimization-roadmap.md — Epic 22, Story 22.2]
- [Source: crates/fila-core/src/broker/drr.rs — current DRR implementation]

## Dev Agent Record

### Agent Model Used

Claude Opus 4.6 (1M context)

### Debug Log References

None.

### Completion Notes List

- Replaced O(n) linear scan in `next_key()` with O(1) `eligible_keys` deque pop
- Removed `round_position` field entirely — ordering maintained by deque
- All 11 existing tests (including property-based fairness invariant) pass unmodified

### File List

- `crates/fila-core/src/broker/drr.rs` — refactored DRR data structure and methods
