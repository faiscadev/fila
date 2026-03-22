# Story 17.1: Cluster Error Clarity & Ack/Nack Performance

Status: ready-for-dev

## Story

As an operator,
I want clear error messages that distinguish transient cluster state from real failures, and efficient ack/nack processing,
so that clients can make correct retry decisions and cluster performance scales with queue depth.

## Acceptance Criteria

1. **Given** a node is joining the cluster and hasn't caught up on Raft log entries
   **When** a client sends a request for a queue whose Raft group isn't locally available yet
   **Then** the server returns gRPC `UNAVAILABLE` (not `NOT_FOUND`) with a `NodeNotReady` error variant
   **And** clients/load balancers can distinguish "queue doesn't exist" from "node is still catching up"
   **And** `ClusterWriteError` has a new `NodeNotReady` variant mapped to gRPC `UNAVAILABLE`

2. **Given** a message is acked in clustered mode
   **When** the Raft state machine applies the ack entry
   **Then** the storage key is included in `ClusterRequest::Ack` (passed through Raft from the leader)
   **And** followers perform a direct key lookup instead of scanning all messages in the queue
   **And** ack is O(1) regardless of queue depth

3. **Given** a message is nacked in clustered mode
   **When** the Raft state machine applies the nack entry
   **Then** the storage key is included in `ClusterRequest::Nack` (passed through Raft from the leader)
   **And** followers perform a direct key lookup instead of scanning all messages in the queue
   **And** nack is O(1) regardless of queue depth

4. **Given** the ack/nack performance fix
   **When** unit tests exercise the Raft apply path
   **Then** tests verify O(1) key-based lookups are used (no `list_messages` calls for ack/nack)
   **And** a benchmark or test with 1000+ messages in a queue demonstrates ack/nack completes without full scan

5. **Given** the `NodeNotReady` error variant
   **When** e2e tests exercise a joining node
   **Then** the test verifies the client receives `UNAVAILABLE` (not `NOT_FOUND`) during node startup

## Tasks / Subtasks

- [ ] Task 1: Add `NodeNotReady` variant to `ClusterWriteError` (AC: 1)
  - [ ] 1.1: Add `NodeNotReady` variant to `ClusterWriteError` in `crates/fila-core/src/cluster/mod.rs`
  - [ ] 1.2: Map `NodeNotReady` to gRPC `UNAVAILABLE` in `cluster_write_err_to_status` (both `service.rs` and `admin_service.rs`)
  - [ ] 1.3: Distinguish `QueueGroupNotFound` (queue genuinely doesn't exist) from `NodeNotReady` (node still catching up) at the call site in `write_to_queue`
  - [ ] 1.4: Update `Display` impl for `ClusterWriteError`

- [ ] Task 2: Add `storage_key` field to `ClusterRequest::Ack` and `ClusterRequest::Nack` (AC: 2, 3)
  - [ ] 2.1: Add `storage_key: Vec<u8>` field to `ClusterRequest::Ack` and `ClusterRequest::Nack`
  - [ ] 2.2: Populate `storage_key` at the leader's call site (where the message key is already known from the lease/delivery path)
  - [ ] 2.3: Update protobuf `ClusterRequest` serialization if using protobuf for Raft log entries (check `store.rs` serde format)

- [ ] Task 3: Rewrite Raft apply for Ack to use O(1) lookup (AC: 2)
  - [ ] 3.1: In `store.rs` ack handler (line ~648), use `storage_key` directly instead of `list_messages` + linear scan
  - [ ] 3.2: Fallback: if `storage_key` is empty (backward compat with in-flight Raft entries), keep the scan path
  - [ ] 3.3: Delete message + lease + lease_expiry using the key directly

- [ ] Task 4: Rewrite Raft apply for Nack to use O(1) lookup (AC: 3)
  - [ ] 4.1: In `store.rs` nack handler (line ~682), use `storage_key` directly instead of `list_messages` + linear scan
  - [ ] 4.2: Same fallback as Task 3 for backward compat
  - [ ] 4.3: Read message by key, update attempt_count, clear leased_at, write back

- [ ] Task 5: Unit tests for O(1) ack/nack path (AC: 4)
  - [ ] 5.1: Test ack with storage_key resolves to direct lookup (no scan)
  - [ ] 5.2: Test nack with storage_key resolves to direct lookup
  - [ ] 5.3: Test backward compat: empty storage_key falls back to scan
  - [ ] 5.4: Test with 100+ messages to verify no performance regression

- [ ] Task 6: NodeNotReady e2e test (AC: 5)
  - [ ] 6.1: Test or extend existing cluster e2e test to verify UNAVAILABLE status during node catchup
  - [ ] 6.2: Verify NOT_FOUND is still returned for genuinely missing queues

- [ ] Task 7: Update sprint-status.yaml (AC: all)
  - [ ] 7.1: Mark story 17-1 as in-progress, epic-17 as in-progress

## Dev Notes

### Architecture Context

**Cluster module** (`crates/fila-core/src/cluster/`):
- `mod.rs` — `ClusterHandle`, `ClusterWriteError` enum, `write_to_queue()` method
- `types.rs` — `ClusterRequest` enum (Raft log entry data), `ClusterResponse` enum
- `store.rs` — Raft state machine `apply_to_state_machine()` method — where ack/nack scan happens
- `multi_raft.rs` — `MultiRaftManager`, one Raft group per queue

**gRPC mapping** — `cluster_write_err_to_status()` is duplicated in:
- `crates/fila-server/src/service.rs` (lines 19-27)
- `crates/fila-server/src/admin_service.rs` (lines 19-27)

Both must be updated when adding `NodeNotReady`.

### Key Design Decision: NodeNotReady vs QueueGroupNotFound

Currently `write_to_queue()` calls `multi_raft.get_raft(queue_id)` and returns `QueueGroupNotFound` when the Raft group isn't available (line 108 of `mod.rs`). This conflates two cases:
1. Queue genuinely doesn't exist (should be `NOT_FOUND`)
2. Queue exists but this node hasn't caught up yet (should be `UNAVAILABLE`)

To distinguish these, the `MultiRaftManager` or `ClusterHandle` needs a way to know whether a queue is expected to exist (it's in the meta Raft's known groups) vs. the local node doesn't have it yet. Check `multi_raft.rs` for whether the meta Raft state tracks known queue groups that can inform this decision.

### Key Design Decision: storage_key in ClusterRequest

The O(n) scan exists because `ClusterRequest::Ack` only carries `(queue_id, msg_id)`. The message key format is `{queue_id}:{fairness_key}:{enqueue_ts_ns}:{msg_id}` — you can't construct the key from just `queue_id` + `msg_id` without `fairness_key` and `enqueue_ts_ns`.

**Solution**: The leader already has the full message key when processing an ack (from the lease/delivery path). Pass it through the Raft log as `storage_key: Vec<u8>`. All replicas then do a direct `get` instead of scan.

`ClusterRequest` is serialized via `serde` (Serialize/Deserialize derives) and stored in the Raft log. Adding `Vec<u8>` is serde-compatible. Check if protobuf serialization was added in Epic 14.6 — if so, the proto definition also needs updating.

### Raft Log Entry Serialization Format

Epic 14.6 added protobuf serialization for cluster entries. Check:
- `crates/fila-proto/proto/cluster.proto` — proto definitions for `ClusterRequest`
- `crates/fila-core/src/cluster/store.rs` — how entries are serialized/deserialized

If protobuf is used, add `bytes storage_key` field to the Ack and Nack proto messages. If serde (bincode/JSON), the `Vec<u8>` with `#[serde(default)]` provides backward compat.

### Storage Engine API

The `StorageEngine` trait (`crates/fila-core/src/storage/engine.rs`) should have a `get_message(key)` or similar direct-access method. If not, you'll need to add one. Currently `list_messages` does prefix-scan iteration. A direct `get` on the messages column family is O(1).

Check if `RocksDbEngine` exposes a `get` on the messages CF, or if you need to add it.

### Backward Compatibility

Raft logs may contain old-format entries (without `storage_key`) from before this change. The apply method must handle both:
- New entries: use `storage_key` directly → O(1)
- Old entries: `storage_key` is empty/missing → fall back to scan → O(n) but only for legacy entries

Use `#[serde(default)]` on the new field for serde, or `optional bytes` in proto.

### Existing Patterns to Follow

- Per-command error types: `EnqueueError`, `AckError`, `NackError` in `crates/fila-core/src/error.rs` — follow this pattern for cluster errors
- Error mapping: `IntoStatus` trait in `crates/fila-server/src/error.rs` — `cluster_write_err_to_status` follows the same pattern
- Storage keys: `crates/fila-core/src/storage/keys.rs` — all key encoding functions
- Lease key is `{queue_id}:{msg_id}` — O(1) constructible, already used in ack/nack cleanup

### Testing Standards

- Unit tests for the Raft apply path in `crates/fila-core/src/cluster/` (likely `store.rs` tests or a new test module)
- E2e tests in `crates/fila-e2e/tests/cluster.rs` using `TestCluster` helper
- Verify both the new O(1) path and the backward-compat scan path
- Verify gRPC status codes via SDK or raw gRPC calls

### References

- [Source: crates/fila-core/src/cluster/mod.rs] — ClusterHandle, ClusterWriteError, write_to_queue
- [Source: crates/fila-core/src/cluster/types.rs] — ClusterRequest, ClusterResponse enums
- [Source: crates/fila-core/src/cluster/store.rs:648-724] — Ack/Nack O(n) scan in apply_to_state_machine
- [Source: crates/fila-core/src/storage/keys.rs] — message_key, lease_key, message_prefix functions
- [Source: crates/fila-server/src/service.rs:19-27] — cluster_write_err_to_status mapping
- [Source: crates/fila-server/src/admin_service.rs:19-27] — duplicate cluster_write_err_to_status
- [Source: crates/fila-e2e/tests/cluster.rs] — existing cluster e2e tests
- [Source: crates/fila-e2e/tests/helpers/cluster.rs] — TestCluster helper
- [GitHub: #63] — Distinguish 'node not ready' from 'queue not found'
- [GitHub: #64] — Ack/nack linear scan fix in Raft apply path

## Dev Agent Record

### Agent Model Used

Claude Opus 4.6 (1M context)

### Debug Log References

### Completion Notes List

### File List
