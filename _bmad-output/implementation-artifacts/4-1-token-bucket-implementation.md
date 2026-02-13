# Story 4.1: Token Bucket Implementation

Status: review

## Story

As a developer,
I want a token bucket rate limiter implementation,
so that per-key rate limits can be enforced in the scheduler.

## Acceptance Criteria

1. **Given** a token bucket configured with a rate (tokens per second) and burst size, **when** the bucket is checked for token availability, **then** tokens are refilled based on elapsed time since last refill
2. **Given** a token bucket with sufficient tokens, **when** `try_consume(n)` is called, **then** it returns true and decrements tokens by n
3. **Given** a token bucket with insufficient tokens, **when** `try_consume(n)` is called, **then** it returns false without modification
4. **Given** a token bucket, **when** tokens are refilled, **then** tokens never exceed the burst size (max capacity)
5. **Given** a token bucket decision, **when** measured, **then** execution completes in <1μs (NFR7)
6. **Given** a `ThrottleManager`, **when** managing token buckets, **then** it supports creating, removing, and updating bucket configurations keyed by throttle key string
7. **Given** a `ThrottleManager` with active buckets, **when** `check_keys(throttle_keys)` is called, **then** it returns true only if ALL keys have available tokens, consuming one token from each
8. **Given** a `ThrottleManager`, **when** `check_keys` is called with a key that has no configured bucket, **then** that key is treated as unthrottled (passes)

## Tasks / Subtasks

- [x] Task 1: Implement `TokenBucket` struct with `new`, `try_consume`, and `refill` (AC: #1, #2, #3, #4)
  - [x] Subtask 1.1: `TokenBucket::new(rate_per_second, burst_size)` initializes with full tokens
  - [x] Subtask 1.2: `TokenBucket::refill(&mut self, now: Instant)` adds `elapsed * rate` tokens, capped at burst
  - [x] Subtask 1.3: `TokenBucket::try_consume(&mut self, n: f64) -> bool` checks and decrements
- [x] Task 2: Implement `ThrottleManager` struct (AC: #6, #7, #8)
  - [x] Subtask 2.1: `ThrottleManager::new()` with empty bucket map
  - [x] Subtask 2.2: `set_rate(key, rate_per_second, burst_size)` creates or updates a bucket
  - [x] Subtask 2.3: `remove_rate(key)` removes a bucket
  - [x] Subtask 2.4: `refill_all(&mut self, now: Instant)` refills all buckets in one pass
  - [x] Subtask 2.5: `check_keys(keys: &[String]) -> bool` checks all keys, consumes from all only if all pass
- [x] Task 3: Unit tests for TokenBucket (AC: #1, #2, #3, #4)
  - [x] Subtask 3.1: Refill timing accuracy test
  - [x] Subtask 3.2: Burst cap test
  - [x] Subtask 3.3: Consume/reject behavior test
  - [x] Subtask 3.4: Rate accuracy over 1-second window test
- [x] Task 4: Unit tests for ThrottleManager (AC: #6, #7, #8)
  - [x] Subtask 4.1: Create/remove/update bucket test
  - [x] Subtask 4.2: Multi-key check (all pass, one fails) test
  - [x] Subtask 4.3: Unknown key treated as unthrottled test
- [x] Task 5: Benchmark token bucket decisions with criterion (AC: #5)

## Dev Notes

### Architecture Placement

Create a new module `crates/fila-core/src/broker/throttle.rs` for the token bucket and throttle manager. This follows the existing pattern of one module per domain concept under `broker/` (like `drr.rs`, `command.rs`, `config.rs`). The architecture doc explicitly names `token_bucket` as a future module, but `throttle.rs` is more concise and matches the naming of `drr.rs`.

Register the module in `crates/fila-core/src/broker/mod.rs`.

### Token Bucket Algorithm

Use the standard token bucket with continuous refill:

```
tokens = min(burst, tokens + elapsed_secs * rate_per_second)
```

- `tokens: f64` — fractional tokens for sub-second precision
- `rate_per_second: f64` — refill rate
- `burst: f64` — max capacity (also initial token count)
- `last_refill: Instant` — last refill timestamp

Use `std::time::Instant` for monotonic timing. The scheduler is single-threaded so no synchronization needed.

### ThrottleManager Design

```rust
pub struct ThrottleManager {
    buckets: HashMap<String, TokenBucket>,
}
```

Key methods:
- `set_rate(key, rate_per_second, burst_size)` — creates or reconfigures a bucket. When updating an existing bucket, preserve current token count (clamped to new burst).
- `remove_rate(key)` — removes the bucket entirely (key becomes unthrottled)
- `refill_all(now)` — batch refill all buckets (called once per scheduler loop iteration)
- `check_keys(keys: &[String]) -> bool` — atomicity: check all keys first, then consume from all only if every key has tokens. If ANY key is exhausted, consume from NONE (prevent partial token drain). Keys not in the manager are treated as unthrottled (pass-through).

### Critical: Atomic Multi-Key Check

For hierarchical throttling (FR17), a message may have multiple throttle keys like `["provider:aws", "region:us-east-1"]`. The check must be atomic:
1. First pass: verify all configured keys have ≥1 token (unconfigured keys pass)
2. If all pass: second pass: consume 1 token from each configured key
3. If any fail: return false, consume nothing

This prevents the scenario where consuming from key A succeeds but key B fails, leaving key A incorrectly drained.

### Performance Target

NFR7 requires <1μs per token bucket decision. This is easily achievable with:
- `HashMap<String, TokenBucket>` lookup: ~50-100ns
- Refill: simple f64 arithmetic: ~10ns
- Total for multi-key check with 2-3 keys: ~200-300ns

Add a criterion benchmark to verify.

### Integration with Scheduler (Story 4.2, NOT this story)

The `ThrottleManager` will be owned by the `Scheduler` struct and called during DRR delivery. This story only creates the standalone data structure and its tests. Story 4.2 wires it into the scheduler loop.

### Existing Code Patterns

- **DRR module** (`broker/drr.rs`): Follow the same pattern — standalone struct with `HashMap`-based state, inline unit tests in `#[cfg(test)]` block, proptest for invariants
- **Error handling**: No errors needed for this module — token bucket operations are infallible (no IO, no storage). Methods return `bool` (allowed/denied)
- **Benchmarks**: Follow `benches/drr.rs` pattern using criterion with `black_box()`

### Dependencies

No new crate dependencies needed. Uses only:
- `std::time::Instant` for timing
- `std::collections::HashMap` for bucket storage
- `criterion` (already in dev-dependencies) for benchmarks

### File Structure

```
crates/fila-core/src/broker/throttle.rs    # TokenBucket + ThrottleManager + tests
crates/fila-core/benches/throttle.rs       # Criterion benchmark
```

Also update:
- `crates/fila-core/src/broker/mod.rs` — add `pub mod throttle;`

### References

- [Source: _bmad-output/planning-artifacts/architecture.md — Concurrency Architecture] Token buckets in scheduler core diagram
- [Source: _bmad-output/planning-artifacts/architecture.md — Scheduler Core Loop] `refill_token_buckets(now)` and `key.any_throttle_key_exhausted()` pseudocode
- [Source: _bmad-output/planning-artifacts/architecture.md — Naming Patterns] Module: `snake_case`, Type: `PascalCase`
- [Source: _bmad-output/planning-artifacts/architecture.md — In-Memory State] "Token bucket state: tokens remaining, last refill timestamp (resets on restart)"
- [Source: _bmad-output/planning-artifacts/epics.md — Story 4.1] Full acceptance criteria

## Dev Agent Record

### Agent Model Used

Claude Opus 4.6

### Debug Log References

### Completion Notes List

- Implemented `TokenBucket` struct with continuous refill algorithm using `f64` tokens and `std::time::Instant`
- Implemented `ThrottleManager` with `HashMap<String, TokenBucket>` for multi-key management
- Atomic multi-key check: two-pass algorithm (verify all, then consume all) prevents partial token drain
- `update()` method preserves current tokens clamped to new burst when reconfiguring a bucket
- 19 unit tests covering all acceptance criteria: refill timing, burst cap, consume/reject, multi-key atomicity, unknown keys
- Criterion benchmark confirms <1μs performance: `try_consume` ~4ns, `check_3_keys` ~64ns, `refill_10_buckets` ~66ns
- No new dependencies required
- All 165 existing tests + 9 integration tests pass with 0 regressions

### File List

- `crates/fila-core/src/broker/throttle.rs` (new) — TokenBucket, ThrottleManager, 19 unit tests
- `crates/fila-core/src/broker/mod.rs` (modified) — added `pub mod throttle;`
- `crates/fila-core/benches/throttle.rs` (new) — criterion benchmarks
- `crates/fila-core/Cargo.toml` (modified) — added `[[bench]]` section for throttle
