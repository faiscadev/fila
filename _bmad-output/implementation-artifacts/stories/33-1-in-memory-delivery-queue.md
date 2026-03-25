# Story 33.1: In-Memory Delivery Queue — Eliminate Consume-Path Storage Reads

Status: pending

## Story

As a developer,
I want messages carried in memory from enqueue to delivery so that the consume path avoids per-message storage reads,
So that consume throughput improves by 2-10x and matches enqueue throughput.

## Acceptance Criteria

1. **Given** the current consume path reads each message from RocksDB (`storage.get_message()`, 10-100μs per message)
   **When** the in-memory delivery queue is implemented
   **Then** `PendingEntry` carries the full message wire bytes (from Story 32.4's store-as-received pattern)
   **And** delivery reads from the in-memory pending entry, NOT from storage
   **And** zero RocksDB reads occur on the normal consume path (consumers keeping up)

2. **Given** the in-memory delivery queue uses memory
   **When** a per-queue memory budget is configured (e.g., 256MB default)
   **Then** when the budget is exceeded, oldest pending entries are evicted (wire bytes dropped, key retained)
   **And** evicted entries fall back to storage reads on delivery
   **And** the memory budget is configurable via `QueueConfig` or `BrokerConfig`

3. **Given** consumers are keeping up (no lag)
   **When** messages flow through enqueue → delivery → ack
   **Then** messages are served entirely from memory (enqueue writes to storage + memory, delivery reads from memory)
   **And** storage reads only occur for consumer lag scenarios (evicted entries)

4. **Given** a crash and recovery
   **When** the server restarts
   **Then** pending queues are rebuilt from storage by scanning un-acked messages (existing behavior)
   **And** the in-memory wire bytes are repopulated from storage during recovery
   **And** no messages are lost

5. **Given** the in-memory delivery queue
   **When** consume throughput is measured
   **Then** throughput improves significantly over the Plateau 1 baseline (32.6)
   **And** the per-message consume cost drops from ~100-200μs to ~5-20μs (storage read eliminated)

## Tasks / Subtasks

- [ ] Task 1: Extend PendingEntry with wire bytes
  - [ ] 1.1 Add `wire_bytes: Option<Bytes>` to `PendingEntry`
  - [ ] 1.2 At enqueue time, populate wire_bytes from the store-as-received `Bytes`
  - [ ] 1.3 Populate extracted metadata in PendingEntry for delivery without deserialization

- [ ] Task 2: Modify delivery path to read from memory
  - [ ] 2.1 In `try_deliver_to_consumer()`, check `entry.wire_bytes` first
  - [ ] 2.2 If present: use wire bytes directly (skip storage read + protobuf decode)
  - [ ] 2.3 If absent (evicted): fall back to `storage.get_message()` (existing path)

- [ ] Task 3: Implement memory budget and eviction
  - [ ] 3.1 Track per-queue memory usage (sum of wire_bytes lengths)
  - [ ] 3.2 When budget exceeded, set `wire_bytes = None` on oldest entries
  - [ ] 3.3 Add `delivery_queue_memory_budget` to config

- [ ] Task 4: Handle crash recovery
  - [ ] 4.1 On recovery, scan un-acked messages from storage (existing)
  - [ ] 4.2 Populate wire_bytes from stored values during rebuild

- [ ] Task 5: Benchmark and profile
  - [ ] 5.1 Run consume throughput (1, 10, 100 consumers)
  - [ ] 5.2 Compare against Plateau 1 baseline
  - [ ] 5.3 Test consumer lag scenario (verify fallback to storage reads works)

## Dev Notes

### The RabbitMQ Insight

RabbitMQ quorum queues keep messages in memory AND write to WAL. "For queues where consumers are keeping up, messages often do not get written to segment files at all." Consumers read from memory; storage is for crash recovery. This is the same pattern — memory for the hot path, storage for durability.

### Memory Math

At 400K msg/s with 1KB messages = 400MB/s. If consumers keep up within 1 second: ~400MB memory. With a 256MB budget per queue, this means ~250ms of buffering. Sufficient for "keeping up" consumers; anything beyond spills to storage reads.

### Interaction with Story 32.4

Story 32.4 (store-as-received) provides the `Bytes` wire representation that this story carries in memory. The two stories are complementary: 32.4 eliminates serialization on the enqueue side, 33.1 eliminates deserialization on the consume side. Together they achieve the "bytes in, bytes out" pattern that Kafka uses.

### Key Files to Modify

| File | Change |
|------|--------|
| `crates/fila-core/src/broker/scheduler/mod.rs` | PendingEntry wire_bytes, memory tracking |
| `crates/fila-core/src/broker/scheduler/handlers.rs` | Populate wire_bytes at enqueue |
| `crates/fila-core/src/broker/scheduler/delivery.rs` | Read from memory, fallback to storage |
| `crates/fila-core/src/config.rs` | Memory budget config |

### References

- [Source: RabbitMQ Quorum Queues](https://www.rabbitmq.com/blog/2020/04/21/quorum-queues-and-why-disks-matter) — In-memory delivery
- [Source: Pulsar PIP-430](https://github.com/apache/pulsar/blob/master/pip/pip-430.md) — Managed ledger cache
- [Research: technical-kafka-parity-profiling-strategy-2026-03-25.md, Pattern 2] — In-memory delivery queue analysis
