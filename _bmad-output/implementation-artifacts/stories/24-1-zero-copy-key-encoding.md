# Story 24.1: Zero-Copy Protobuf Passthrough & Key Encoding

Status: in-progress

## Story

As a Fila operator running high-throughput workloads,
I want the broker to avoid redundant allocations on the enqueue and delivery hot paths,
So that per-message overhead is reduced and throughput increases.

## Acceptance Criteria

1. **Given** protobuf `Message.payload` and `EnqueueRequest.payload` are currently `Vec<u8>`
   **When** prost is configured to use `bytes::Bytes` for payload fields
   **Then** the generated proto types use `bytes::Bytes` instead of `Vec<u8>`, enabling zero-copy reference-counted clones

2. **And** the domain `Message.payload` in `fila-core` is changed to `bytes::Bytes` so the zero-copy benefit flows through the entire hot path (enqueue → storage → delivery)

3. **And** the SDK public API (`EnqueueMessage.payload`, `ConsumeMessage.payload`) remains `Vec<u8>` for ergonomics, with conversions at the SDK boundary

4. **And** all storage key construction functions in `keys.rs` use `Vec::with_capacity()` with the exact calculated output size instead of heuristic estimates

5. **And** all existing tests pass with zero regressions

## Tasks / Subtasks

1. Configure prost `bytes` feature in `fila-proto/build.rs` for payload fields
2. Add `bytes` dependency to `fila-proto/Cargo.toml`
3. Change domain `Message.payload` to `bytes::Bytes`
4. Change `ReadyMessage.payload` to `bytes::Bytes`
5. Update proto ↔ domain conversion code
6. Update SDK boundary conversions
7. Optimize key construction with exact pre-allocation
8. Fix all compilation errors and run tests
