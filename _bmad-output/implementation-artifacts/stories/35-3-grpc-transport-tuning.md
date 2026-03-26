# Story 35.3: gRPC Transport Tuning

Status: ready-for-dev

## Story

As an operator,
I want optimized gRPC transport settings for high-throughput workloads,
So that the existing gRPC stack extracts maximum performance without protocol changes.

## Acceptance Criteria

1. **Given** the current `GrpcConfig` defaults (2MB stream window, 4MB connection window, TCP_NODELAY on)
   **When** the gRPC transport settings are tuned
   **Then** window sizes are increased to reduce flow control overhead

2. **Given** the scheduler's `write_coalesce_max_batch` (current: 100)
   **When** profiling shows batches hitting the cap
   **Then** the default is increased to allow larger coalescing batches

3. **Given** the scheduler's `delivery_batch_max_messages` (current: 10)
   **When** consumer throughput can benefit from larger delivery batches
   **Then** the default is increased

4. **Given** each setting change
   **When** validated by benchmarks
   **Then** settings that show no improvement or regression are reverted

5. **Given** the tuned settings
   **When** all existing tests pass
   **Then** no regressions are introduced

## Tasks / Subtasks

- [ ] Task 1: Add `http2_max_frame_size` to GrpcConfig (AC: 1)
- [ ] Task 2: Increase default window sizes (AC: 1)
- [ ] Task 3: Increase `write_coalesce_max_batch` default (AC: 2)
- [ ] Task 4: Increase `delivery_batch_max_messages` default (AC: 3)
- [ ] Task 5: Apply http2_max_frame_size to tonic server builder (AC: 1)
- [ ] Task 6: Update config tests for new defaults (AC: 5)
- [ ] Task 7: Update docs/configuration.md with new defaults (AC: 4)
- [ ] Task 8: Verify all tests pass (AC: 5)

## Dev Notes

- `GrpcConfig` at `crates/fila-core/src/broker/config.rs:226`
- Server builder at `crates/fila-server/src/main.rs:202`
- Current defaults: stream=2MB, connection=4MB, no http2_max_frame_size exposed
- Tonic default http2_max_frame_size is 16KB — increasing to 64KB for large batches
- Window sizes: 8MB stream, 16MB connection (4x increase) — reduces WINDOW_UPDATE overhead
- write_coalesce_max_batch: 100 → 256 (2.5x) — allows more messages per write batch
- delivery_batch_max_messages: 10 → 100 (10x) — aligns with enqueue batch size for symmetry

### References

- [Source: crates/fila-core/src/broker/config.rs]
- [Source: crates/fila-server/src/main.rs]

## Dev Agent Record

### Agent Model Used

Claude Opus 4.6 (1M context)

### Debug Log References

### Completion Notes List

### File List
