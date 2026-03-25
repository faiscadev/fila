# Story 32.5: Arena Allocation for Batch Processing

Status: pending

## Story

As a developer,
I want per-batch scratch memory allocated from an arena instead of individual heap allocations,
So that allocation churn and cache thrashing on the scheduler thread are reduced.

## Acceptance Criteria

1. **Given** each message in a batch creates multiple small heap allocations (storage key, prepared enqueue struct, metadata)
   **When** `bumpalo` arena allocation is introduced for batch processing
   **Then** per-batch scratch data is allocated from a single arena
   **And** the arena is dropped after the batch is committed (one deallocation for the entire batch)

2. **Given** the arena allocator
   **When** per-message allocations in `flush_coalesced_enqueues()` are measured
   **Then** heap allocations per message decrease compared to the 32.4 baseline
   **And** the remaining allocations are for data that outlives the batch (e.g., pending entries, DRR state)

3. **Given** `bumpalo` does not run destructors
   **When** arena-allocated data is designed
   **Then** arena-allocated types use `&[u8]` slices instead of `Vec<u8>`, and `&str` instead of `String`
   **And** types with heap allocations are NOT placed in the arena (or are explicitly managed)

4. **Given** the arena changes
   **When** all existing tests pass
   **Then** behavior is identical to pre-arena code
   **And** `fila-bench` enqueue throughput improves over the 32.4 baseline

5. **Given** stories 32.2-32.5 are all applied
   **When** the cumulative improvement is measured
   **Then** the improvement is documented against the 32.1 baseline with per-story attribution

## Tasks / Subtasks

- [ ] Task 1: Add `bumpalo` dependency
  - [ ] 1.1 Add `bumpalo = "3"` to `fila-core/Cargo.toml`
  - [ ] 1.2 Create arena in `flush_coalesced_enqueues()` (per-batch lifetime)

- [ ] Task 2: Migrate batch processing to arena allocation
  - [ ] 2.1 Allocate storage keys from arena (`arena.alloc_slice_copy()`)
  - [ ] 2.2 Allocate prepared enqueue scratch data from arena
  - [ ] 2.3 Ensure arena-allocated data uses borrowed types (slices, not Vecs)

- [ ] Task 3: Profile allocation reduction
  - [ ] 3.1 Measure heap allocations per message before and after (using `divan` or custom counter)
  - [ ] 3.2 Run flamegraph — verify allocator overhead reduced

- [ ] Task 4: Benchmark cumulative Plateau 1 improvement
  - [ ] 4.1 Run `fila-bench` enqueue throughput
  - [ ] 4.2 Document cumulative improvement: 32.1 baseline → 32.2 → 32.3 → 32.4 → 32.5

## Dev Notes

### bumpalo Characteristics

- Allocation: single pointer increment (vs malloc's free-list traversal)
- Deallocation: all memory freed at once when arena dropped — no per-object deallocation
- `extend_from_slice_copy` is ~80x faster than `extend_from_slice` for Copy types
- Cache-friendly: objects allocated together are physically adjacent in memory

### Interaction with Earlier Stories

This story builds on the reduced allocation surface from stories 32.3 (interning eliminates string clones) and 32.4 (store-as-received eliminates protobuf serialization allocations). The remaining allocations targeted by arena are:
- Storage key bytes (binary encoding)
- Mutation structs
- Any remaining per-message scratch data

### Key Files to Modify

| File | Change |
|------|--------|
| `crates/fila-core/src/broker/scheduler/mod.rs` | Create arena per batch in flush_coalesced_enqueues |
| `crates/fila-core/src/broker/scheduler/handlers.rs` | Use arena for per-message scratch data |
| `crates/fila-core/src/storage/keys.rs` | Arena-backed key encoding |
| `crates/fila-core/Cargo.toml` | Add bumpalo dependency |

### References

- [Source: bumpalo GitHub](https://github.com/fitzgen/bumpalo) — Arena allocator API
- [Source: Guide to Arenas in Rust](https://blog.logrocket.com/guide-using-arenas-rust/) — Usage patterns
- [Research: technical-kafka-parity-profiling-strategy-2026-03-25.md, Pattern 5] — Arena allocation analysis
