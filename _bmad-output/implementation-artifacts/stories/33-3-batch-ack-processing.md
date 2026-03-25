# Story 33.3: Batch Ack/Nack Processing — Amortize Storage Deletes

Status: pending

## Story

As a developer,
I want ack-triggered message deletes accumulated in memory and batch-written to storage periodically,
So that the per-ack storage write cost is amortized across many acks.

## Acceptance Criteria

1. **Given** the in-memory lease tracking from Story 33.2 queues pending deletes
   **When** acks accumulate
   **Then** message deletions are collected in a batch buffer (not written individually)
   **And** the batch is flushed to RocksDB as a single WriteBatch on the checkpoint interval (from 33.2)

2. **Given** batch ack processing
   **When** N acks occur between checkpoints
   **Then** exactly 1 RocksDB WriteBatch is executed (containing N delete operations)
   **And** per-ack amortized storage cost is O(1/N) of a single write

3. **Given** the batch delete buffer
   **When** a configurable threshold is reached (e.g., 10,000 pending deletes)
   **Then** an early flush is triggered to bound memory usage
   **And** the threshold is configurable alongside the checkpoint interval

4. **Given** Kafka's offset commit pattern
   **When** the batch ack implementation is compared
   **Then** the pattern is documented: Kafka commits consumer offsets periodically (default 5s), Fila checkpoints ack deletes periodically (default 100ms)
   **And** the semantic equivalence is noted (both are at-least-once with periodic durability)

5. **Given** the batch ack processing
   **When** end-to-end consume throughput is measured (enqueue → consume → ack → delete)
   **Then** throughput improves over the 33.2 baseline
   **And** the full consume lifecycle cost is quantified

## Tasks / Subtasks

- [ ] Task 1: Implement batch delete buffer
  - [ ] 1.1 Collect pending deletes in `Vec<DeleteOp>` on ack
  - [ ] 1.2 Flush as RocksDB WriteBatch on checkpoint (reuse 33.2 checkpoint)
  - [ ] 1.3 Add early flush threshold config

- [ ] Task 2: Integrate with 33.2 checkpoint
  - [ ] 2.1 Include pending deletes in the checkpoint WriteBatch
  - [ ] 2.2 Clear the delete buffer after successful flush
  - [ ] 2.3 Handle flush failure (retain buffer, retry on next checkpoint)

- [ ] Task 3: Handle crash recovery
  - [ ] 3.1 Un-flushed deletes mean messages persist in storage after crash
  - [ ] 3.2 These messages will be detected as un-acked during recovery and redelivered
  - [ ] 3.3 At-least-once semantics preserved — document this

- [ ] Task 4: Benchmark end-to-end consume lifecycle
  - [ ] 4.1 Measure: enqueue → consume → ack → storage delete
  - [ ] 4.2 Compare per-ack cost: individual writes vs batched
  - [ ] 4.3 Test under varying ack rates

## Dev Notes

### Pattern Extension

This story extends Pattern 3 (in-memory lease tracking) from Story 33.2. While 33.2 eliminates the lease write on delivery, this story eliminates the individual delete write on ack. Together they make the entire consume lifecycle (deliver → ack → delete) write-free on the hot path, with periodic batched writes for durability.

### Batch Size Economics

| Acks/Checkpoint | WriteBatch Size | Per-Ack Amortized Cost |
|----------------|-----------------|----------------------|
| 10 | 10 deletes | ~1-3μs/ack |
| 100 | 100 deletes | ~0.1-0.3μs/ack |
| 1,000 | 1,000 deletes | ~0.01-0.03μs/ack |

At target throughput (100K+ msg/s) with 100ms checkpoint: ~10K acks per batch. Per-ack storage cost becomes negligible.

### Key Files to Modify

| File | Change |
|------|--------|
| `crates/fila-core/src/broker/scheduler/handlers.rs` | Queue delete on ack instead of immediate write |
| `crates/fila-core/src/broker/scheduler/mod.rs` | Delete buffer, flush on checkpoint |
| `crates/fila-core/src/config.rs` | Early flush threshold config |

### References

- [Source: NATS JetStream AckAll](https://docs.nats.io/nats-concepts/jetstream/consumers) — Batch ack pattern
- [Research: technical-kafka-parity-profiling-strategy-2026-03-25.md, Pattern 3 extension] — Batch ack analysis
