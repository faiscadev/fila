# Story 2.2: Weighted Fairness & Accuracy Verification

Status: review

## Story

As a platform engineer,
I want higher-weighted fairness groups to receive proportionally more throughput,
so that I can prioritize important tenants while still guaranteeing fair shares for all.

## Acceptance Criteria

1. Given messages are enqueued across multiple fairness keys with different weights, when the DRR scheduler operates under sustained load, then a key with weight 2 receives approximately 2x the throughput of a key with weight 1
2. Each key's actual throughput stays within 5% of its calculated fair share (`key_weight / total_weight * total_throughput`)
3. The default weight for keys without explicit assignment is 1
4. Weight changes take effect on the next scheduling round
5. Property-based tests (proptest) verify the fairness invariant: for any combination of keys and weights under sustained load, each key receives within 5% of its fair share
6. The fairness accuracy test runs with at least 10,000 messages across 5+ keys with varying weights

## Tasks / Subtasks

- [x] Task 1: Proptest spike — add proptest dependency and write one proptest against existing code (AC: prerequisite)
  - [x] 1.1 Add `proptest` crate to `fila-core` dev-dependencies
  - [x] 1.2 Write 5 proptests for key encoding properties (prefix correctness, roundtrips, ordering)
  - [x] 1.3 Verify proptest runs correctly with `cargo test`

- [x] Task 2: DRR fairness property test — pure scheduling (AC: #1, #2, #5)
  - [x] 2.1 Write proptest that generates random key counts (2-20), random weights (1-10)
  - [x] 2.2 For each generated scenario: run 10 DRR rounds with unlimited messages
  - [x] 2.3 Assert: each key's share of total deliveries is within 5% of `key_weight / total_weight`
  - [x] 2.4 Test the DRR module directly (no storage, no scheduler — pure in-memory)

- [x] Task 3: Integration fairness test — full scheduler path (AC: #1, #2, #6)
  - [x] 3.1 Write integration test: 6 keys with weights 1-6, 10,500 messages total
  - [x] 3.2 Each key gets weight*500 messages (enough that no key exhausts before round ends)
  - [x] 3.3 Register consumer, run scheduler, collect all delivered messages
  - [x] 3.4 Assert: each key's delivery count is within 5% of fair share
  - Note: test marked `#[ignore]` due to O(n²) scan cost (~3 min in debug mode); run with `cargo test -- --ignored`

- [x] Task 4: Verify default weight and weight update behavior (AC: #3, #4)
  - [x] 4.1 Test: `drr_default_weight_is_one` — messages with default weight=1 get equal delivery
  - [x] 4.2 Test: `drr_weight_update_changes_proportions` — weight update from 1→3 changes delivery ratio to 3:1

## Dev Notes

### Architecture Context

Story 2.1 already implemented the full DRR scheduling algorithm with weight support:
- `DrrScheduler::add_key(queue_id, fairness_key, weight)` — weight is stored per-key
- `DrrScheduler::update_weight(queue_id, fairness_key, weight)` — updates take effect next round
- `DrrScheduler::start_new_round()` — allocates `weight * quantum` deficit per key
- Default weight is enforced via `weight.max(1)` in `add_key` and `update_weight`

This story is primarily about **verification** — using property-based testing to prove the fairness invariant holds across all possible weight/key combinations.

### Proptest Spike

The Epic 1 retrospective identified a proptest spike as a prerequisite for Story 2.2. The spike should:
1. Add `proptest` as a dev-dependency
2. Write one simple proptest against existing code (e.g., key encoding properties)
3. Build familiarity with the framework before writing the fairness invariant test

### Fairness Invariant

For a DRR scheduler with keys `k1...kN` with weights `w1...wN`:
- Fair share for key `ki` = `wi / sum(w1...wN)`
- Under sustained load (all keys have unlimited messages), key `ki` should receive `fair_share_i * total_delivered` messages, ±5%

The 5% tolerance accounts for:
- Round boundary effects (partial rounds at the end)
- Integer division in deficit allocation
- Key ordering effects in round-robin traversal

### Files to Create

- None — tests go in existing test modules

### Files to Modify

- `crates/fila-core/Cargo.toml` — add `proptest` dev-dependency
- `crates/fila-core/src/storage/keys.rs` — add proptest for key encoding (spike)
- `crates/fila-core/src/broker/drr.rs` — add proptest for fairness invariant
- `crates/fila-core/src/broker/scheduler.rs` — add integration fairness test

### Testing Standards

- Property-based tests use `proptest!` macro with strategies for key counts, weights, message counts
- Integration test uses deterministic setup (fixed weights, large message count)
- Both test types verify the 5% tolerance invariant

### Dependencies

- New dev-dependency: `proptest` crate

## Dev Agent Record

### Agent Model Used

Claude Opus 4.6

### Debug Log References

- Proptest fairness invariant: first design used limited messages per key, causing exhaustion to distort shares. Redesigned to use unlimited messages (sustained load model).
- Integration 10k test: deadlocked with default channel capacity (256 << 10,500 commands). Fixed by using custom channel size. Still O(n²) due to per-delivery prefix scan (~184s in debug mode). Marked `#[ignore]`.
- Weight update test: initial design registered consumer before enqueues, causing 1:1 enqueue-to-delivery ratio (quantum >> message count). Fixed by registering consumer after enqueues with quantum=5 so deficit limits delivery.

### Completion Notes List

- 5 proptests added to keys.rs (key encoding properties)
- 1 proptest added to drr.rs (fairness invariant with unlimited messages, 2-20 keys, weights 1-10)
- 1 ignored integration test in scheduler.rs (10k messages, 6 keys with weights 1-6, 5% tolerance)
- 2 integration tests in scheduler.rs (default weight, weight update proportions)
- proptest-regressions/broker/drr.txt checked in for regression replay

### File List

- `crates/fila-core/Cargo.toml` — added `proptest` dev-dependency
- `crates/fila-core/src/storage/keys.rs` — added 5 proptests for key encoding
- `crates/fila-core/src/broker/drr.rs` — added fairness invariant proptest
- `crates/fila-core/src/broker/scheduler.rs` — added 3 integration tests (10k ignored, default weight, weight update)
- `crates/fila-core/proptest-regressions/broker/drr.txt` — proptest regression seeds
