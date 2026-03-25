# Story 32.4: Hybrid Envelope — Store-as-Received Wire Bytes

Status: pending

## Story

As a developer,
I want the enqueue path to store the original protobuf wire bytes instead of cloning and re-serializing messages,
So that the largest per-message CPU costs (clone + serialize + mutation clone) are eliminated.

## Acceptance Criteria

1. **Given** the current enqueue path deep-clones the `Message` struct and re-serializes it via `prost::Message::encode_to_vec()`
   **When** the hybrid envelope pattern is implemented
   **Then** the original protobuf wire bytes (`Bytes`) received from the gRPC layer are stored directly in RocksDB
   **And** no `Message::clone()` or `prost::encode_to_vec()` occurs on the enqueue hot path
   **And** the mutation value is the original `Bytes` reference (atomic increment only, no copy)

2. **Given** the scheduler needs routing metadata (queue_id, fairness_key, weight) for DRR scheduling
   **When** a message is enqueued
   **Then** routing metadata is extracted from the wire bytes via partial protobuf decode (e.g., `rustwire` or field-level extraction)
   **And** full message deserialization does NOT occur on the enqueue hot path
   **And** extracted metadata uses `Spur` tokens from Story 32.3 for queue_id and fairness_key

3. **Given** the consume/delivery path currently deserializes messages from storage
   **When** a message is delivered to a consumer
   **Then** the stored wire bytes are sent directly to the consumer without re-serialization
   **And** the gRPC response contains the original wire bytes as the message payload

4. **Given** Lua `on_enqueue` hooks need access to message fields
   **When** a Lua hook is configured on a queue
   **Then** the hook receives extracted metadata (queue_id, fairness_key, headers, weight) and payload length
   **And** if the hook needs full payload access, the payload is lazy-deserialized from wire bytes on demand
   **And** queues WITHOUT Lua hooks pay zero deserialization cost

5. **Given** the store-as-received changes
   **When** all existing tests (unit, integration, e2e) are run
   **Then** all tests pass with identical observable behavior
   **And** messages stored by the new code can be read correctly
   **And** `fila-bench` enqueue throughput improves significantly over the 32.3 baseline

6. **Given** cluster mode with Raft replication
   **When** messages are replicated via Raft proposals
   **Then** the wire bytes are replicated as-is (no re-serialization for Raft)
   **And** followers store the same wire bytes as the leader

## Tasks / Subtasks

- [ ] Task 1: Add partial protobuf decode capability
  - [ ] 1.1 Evaluate `rustwire` vs `proto-scan` vs manual field extraction
  - [ ] 1.2 Implement metadata extraction: queue_id, fairness_key, weight, headers from wire bytes
  - [ ] 1.3 Benchmark extraction cost vs full decode

- [ ] Task 2: Refactor enqueue path to store wire bytes
  - [ ] 2.1 Thread `Bytes` (original wire bytes) from gRPC handler into scheduler
  - [ ] 2.2 Refactor `prepare_enqueue()` to work with extracted metadata + wire bytes instead of `Message`
  - [ ] 2.3 Store wire bytes as RocksDB value (eliminate `encode_to_vec()`)
  - [ ] 2.4 Eliminate `Message::clone()` — use `Bytes::clone()` (atomic refcount only)

- [ ] Task 3: Refactor delivery path to send wire bytes
  - [ ] 3.1 On delivery, read stored wire bytes from storage
  - [ ] 3.2 Send wire bytes directly in gRPC response (no decode + re-encode)
  - [ ] 3.3 Verify consumer SDKs correctly decode the wire bytes

- [ ] Task 4: Handle Lua hook compatibility
  - [ ] 4.1 For queues with Lua hooks: pass extracted metadata to hook
  - [ ] 4.2 Implement lazy payload deserialization for hooks that need it
  - [ ] 4.3 For queues without Lua hooks: zero deserialization cost

- [ ] Task 5: Benchmark and profile
  - [ ] 5.1 Run `fila-bench` enqueue throughput
  - [ ] 5.2 Run flamegraph — verify clone/serialize/mutation overhead eliminated
  - [ ] 5.3 Measure allocation count reduction per message

## Dev Notes

### The Hybrid Envelope Pattern

This is the single highest-impact optimization in Plateau 1. It eliminates 3 of the top per-message CPU costs:

| Operation | Before | After |
|-----------|--------|-------|
| Message clone (deep copy) | 500ns-2μs | ~10ns (Bytes atomic increment) |
| Protobuf serialization | 1-5μs | 0 (store wire bytes as-is) |
| Mutation value clone | 500ns-1μs | ~10ns (Bytes atomic increment) |

The key insight from Kafka: the broker never deserializes message payloads. It stores bytes and routes by metadata extracted from the wire format.

### Wire Bytes Flow

```
Client → protobuf bytes → tonic/prost
  → extract metadata (partial decode: queue_id, fairness_key, weight)
  → store wire bytes in RocksDB as-is
  → on delivery: read wire bytes → send to consumer (no re-encode)
```

### Partial Decode Options

- **`rustwire`**: dependency-free, extract field by tag number. Two-level extraction for nested `MessageMetadata`.
- **`proto-scan`**: event-stream parser with `#[derive]` macros. Heavier but more ergonomic.
- **Manual**: read varint tag, skip to field offset. Fragile but zero-dependency.

Recommend `rustwire` for simplicity. Evaluate performance vs full decode in Task 1.

### Key Files to Modify

| File | Change |
|------|--------|
| `crates/fila-server/src/service.rs` | Thread Bytes through to scheduler |
| `crates/fila-core/src/broker/scheduler/handlers.rs` | Refactor prepare_enqueue for wire bytes |
| `crates/fila-core/src/broker/scheduler/delivery.rs` | Send stored wire bytes on delivery |
| `crates/fila-core/src/storage/rocksdb.rs` | Store/retrieve wire bytes |
| `crates/fila-core/Cargo.toml` | Add rustwire or proto-scan dependency |

### References

- [Source: Kafka Efficient Design](https://docs.confluent.io/kafka/design/efficient-design.html) — Zero per-message work
- [Source: Apache Iggy Zero-Copy](https://iggy.apache.org/blogs/2025/05/08/zero-copy-deserialization/) — Store-as-received pattern
- [Source: Greptime Protobuf Optimization](https://greptime.com/blogs/2024-04-09-rust-protobuf-performance) — 80% decode time reduction
- [Research: technical-kafka-parity-profiling-strategy-2026-03-25.md, Pattern 1] — Hybrid envelope analysis
