# Story 33.2: In-Memory Lease Tracking — Eliminate Per-Delivery Storage Writes

Status: pending

## Story

As a developer,
I want lease state tracked in memory with periodic checkpoints to storage,
So that the 2 per-delivery RocksDB writes (lease + expiry) are eliminated from the hot path.

## Acceptance Criteria

1. **Given** every message delivery currently writes 2 RocksDB mutations (CF_LEASES + CF_LEASE_EXPIRY)
   **When** in-memory lease tracking is implemented
   **Then** delivering a message marks it as leased in an in-memory `HashMap<MsgId, LeaseInfo>` — zero disk writes on the hot path
   **And** lease expiry is tracked via a timing wheel (`tokio_util::time::DelayQueue`) instead of storage-based expiry scanning

2. **Given** the in-memory lease map
   **When** a message is acked
   **Then** the entry is removed from the in-memory lease map
   **And** a "delete message" operation is queued in a pending batch (NOT written immediately)

3. **Given** the in-memory lease map
   **When** a message is nacked
   **Then** the entry is removed from the in-memory lease map
   **And** the message is re-enqueued to DRR (message still in storage)

4. **Given** the in-memory lease state
   **When** a periodic checkpoint fires (configurable interval, e.g., every 100ms or 1000 operations)
   **Then** all pending deletes are batch-written to RocksDB
   **And** the current lease set snapshot is persisted for crash recovery
   **And** this is the crash-recovery boundary

5. **Given** a lease timeout expires
   **When** the `DelayQueue` timer fires
   **Then** the message is re-enqueued to DRR from in-memory state
   **And** no RocksDB read is required (lease info is in memory)

6. **Given** a crash and recovery
   **When** the server restarts
   **Then** the last checkpoint's lease set is read from storage
   **And** messages in the lease set whose timer has expired are redelivered
   **And** messages delivered after the last checkpoint are also redelivered (at-least-once semantics preserved)

7. **Given** the at-least-once semantics trade-off
   **When** the crash window (= checkpoint interval) is documented
   **Then** the configuration docs explain that messages delivered between the last checkpoint and crash will be redelivered
   **And** this is consistent with Fila's existing at-least-once guarantee

8. **Given** the in-memory lease tracking
   **When** consume throughput is measured
   **Then** per-message consume cost drops significantly (lease writes eliminated)
   **And** throughput improves over the 33.1 baseline

## Tasks / Subtasks

- [ ] Task 1: Implement in-memory lease map
  - [ ] 1.1 Add `HashMap<MsgId, LeaseInfo>` to scheduler state
  - [ ] 1.2 On delivery: insert lease entry in map (no RocksDB write)
  - [ ] 1.3 On ack: remove from map, queue pending delete
  - [ ] 1.4 On nack: remove from map, re-enqueue to DRR

- [ ] Task 2: Implement timing wheel for lease expiry
  - [ ] 2.1 Add `DelayQueue` for lease timeout tracking
  - [ ] 2.2 On delivery: insert timeout entry
  - [ ] 2.3 On ack/nack: cancel timeout
  - [ ] 2.4 On timeout fire: re-enqueue to DRR

- [ ] Task 3: Implement periodic checkpoint
  - [ ] 3.1 Define checkpoint interval (configurable, default 100ms)
  - [ ] 3.2 On checkpoint: batch-write pending deletes to RocksDB
  - [ ] 3.3 On checkpoint: persist current lease set snapshot

- [ ] Task 4: Implement crash recovery
  - [ ] 4.1 On startup: read last checkpoint's lease set
  - [ ] 4.2 Expired leases → redeliver
  - [ ] 4.3 Post-checkpoint deliveries → redeliver (at-least-once)

- [ ] Task 5: Remove storage-based lease operations from hot path
  - [ ] 5.1 Remove per-delivery CF_LEASES write
  - [ ] 5.2 Remove per-delivery CF_LEASE_EXPIRY write
  - [ ] 5.3 Remove storage-based expiry scanning (replaced by DelayQueue)

- [ ] Task 6: Benchmark and profile
  - [ ] 6.1 Run consume throughput benchmarks
  - [ ] 6.2 Measure per-delivery cost reduction
  - [ ] 6.3 Test crash recovery correctness

## Dev Notes

### tokio_util::time::DelayQueue

Backed by a hierarchical timing wheel (same design as Kafka's Purgatory):
- 6 levels × 64 slots (Level 0: 1ms slots, Level 5: ~12 day slots)
- All operations O(1): insert, cancel, fire
- Slab-based storage: memory reused for expired entries
- `!Sync` — must be polled from a single task (natural fit for Fila's single-writer scheduler)

No external crate needed — `tokio-util` is already a dependency.

### Crash Window Trade-off

| Checkpoint Interval | Crash Window | Checkpoint I/O Load |
|--------------------|--------------|--------------------|
| 10ms | 10ms redelivery | High (100 writes/s) |
| 100ms (default) | 100ms redelivery | Moderate (10 writes/s) |
| 1000ms | 1s redelivery | Low (1 write/s) |

At 100ms default with 400K msg/s throughput: up to 40K messages may be redelivered on crash. This is consistent with Kafka's behavior (consumer offset commits are also periodic, default 5s).

### Key Files to Modify

| File | Change |
|------|--------|
| `crates/fila-core/src/broker/scheduler/mod.rs` | Add lease map, DelayQueue, checkpoint timer |
| `crates/fila-core/src/broker/scheduler/delivery.rs` | In-memory lease instead of storage write |
| `crates/fila-core/src/broker/scheduler/handlers.rs` | Ack/nack from in-memory state |
| `crates/fila-core/src/broker/scheduler/recovery.rs` | Rebuild from checkpoint on startup |
| `crates/fila-core/src/config.rs` | Checkpoint interval config |

### References

- [Source: Relaxed Logs and Strict Leases (Paul Khuong)](https://pvk.ca/Blog/2024/03/23/relaxed-logs-and-leases/) — In-memory lease architecture
- [Source: Kafka Purgatory and Hierarchical Timing Wheels](https://www.confluent.io/blog/apache-kafka-purgatory-hierarchical-timing-wheels/) — Timing wheel design
- [Source: Tokio Timer Wheel](https://tokio.rs/blog/2018-03-timers) — DelayQueue implementation
- [Research: technical-kafka-parity-profiling-strategy-2026-03-25.md, Pattern 3] — In-memory lease tracking analysis
