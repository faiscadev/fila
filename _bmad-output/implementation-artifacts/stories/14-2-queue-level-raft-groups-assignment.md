# Story 14.2: Queue-Level Raft Groups & Assignment

Status: review

## Story

As an operator,
I want each queue to be its own Raft group distributed across the cluster,
so that queues scale independently and failure of one queue's leader doesn't affect other queues.

## Acceptance Criteria

1. **Given** a Fila cluster is running with multiple nodes (from Story 14.1), **when** an operator creates a queue, **then** a new Raft group is created for that queue with all cluster nodes as replicas, and one node is elected Raft leader for that queue.

2. **Given** a multi-node cluster with multiple queues, **when** different queues are created, **then** leadership is distributed across nodes automatically (different queues may have different leaders).

3. **Given** a queue has a Raft group, **when** the operator deletes the queue, **then** the Raft group is shut down and cleaned up on all nodes.

4. **Given** an operator creates or deletes a queue on any node, **when** the receiving node is not the meta Raft leader, **then** the request is forwarded to the meta leader and the operator receives a normal response (transparent routing).

5. **Given** a queue's Raft group is running, **when** a write operation (enqueue, ack, nack) is submitted, **then** it is committed to the queue's own Raft log before the response is returned.

6. **Given** a new node joins the cluster, **when** it is added to the meta Raft group, **then** it also joins as a follower for all existing queue Raft groups.

7. **Given** a node is removed from the cluster, **when** it was a leader for some queue groups, **then** those queue groups elect new leaders from remaining nodes within the election timeout.

8. **Given** a Fila node is running in single-node mode (cluster disabled), **when** queues are created, **then** no Raft groups are created and operations go directly to the local scheduler (zero overhead preserved from Story 14.1).

9. **Given** a 3-node cluster, **when** integration tests run, **then** they verify: queue creation creates a Raft group, writes are committed through the queue's Raft, leadership is distributed, and queue deletion cleans up the Raft group.

## Tasks / Subtasks

- [x] Task 1: Multi-Raft manager for per-queue Raft groups (AC: 1, 2, 3, 6, 7)
  - [x] 1.1 Create `MultiRaftManager` struct that manages multiple `Raft<TypeConfig>` instances (one per queue)
  - [x] 1.2 Add `create_queue_group(queue_id, members)` — creates a new Raft instance for the queue, starts its gRPC handlers
  - [x] 1.3 Add `remove_queue_group(queue_id)` — shuts down and cleans up a queue's Raft group
  - [x] 1.4 Add `get_queue_raft(queue_id)` — returns the Raft instance for a queue (for submitting writes)
  - [x] 1.5 Store queue→Raft group mappings in the meta Raft state machine (so all nodes learn about queue groups through Raft replication)

- [x] Task 2: Extend meta Raft state machine for queue group management (AC: 1, 3, 4)
  - [x] 2.1 Add `CreateQueueGroup { queue_id, members }` and `DeleteQueueGroup { queue_id }` variants to `ClusterRequest`
  - [x] 2.2 When the meta state machine applies `CreateQueueGroup`, update queue_groups map
  - [x] 2.3 When the meta state machine applies `DeleteQueueGroup`, remove from queue_groups map
  - [x] 2.4 Track active queue groups in `StateMachineData` for snapshot/restore

- [x] Task 3: Per-queue Raft storage isolation (AC: 5)
  - [x] 3.1 Key-prefixed isolation within the existing `raft_log` CF (prefix: `q:{queue_id}:`)
  - [x] 3.2 Refactored `FilaRaftStore` with `KeyBuilder` for queue-scoped key prefixes
  - [x] 3.3 Separate vote, log, purge, and snapshot state per queue group via key prefixes

- [x] Task 4: Queue-level gRPC multiplexing (AC: 5)
  - [x] 4.1 Added `group_id` field to `RaftRequest` proto for multiplexing
  - [x] 4.2 `ClusterGrpcService::resolve_raft()` routes RPCs to correct Raft instance by group_id
  - [x] 4.3 `FilaNetwork` includes `group_id` in outgoing RPCs via `make_request()`

- [ ] Task 5: Wire queue CRUD to multi-raft lifecycle (AC: 1, 3, 4, 8) — deferred to Story 14.3
  - [ ] 5.1 When `Broker::create_queue` is called in cluster mode, submit `CreateQueueGroup` to the meta Raft leader
  - [ ] 5.2 When `Broker::delete_queue` is called in cluster mode, submit `DeleteQueueGroup` to the meta Raft leader
  - [x] 5.3 In single-node mode (cluster disabled), bypass all Raft group management (AC 8) — already handled by conditional `ClusterManager` initialization

- [ ] Task 6: Write path through queue Raft (AC: 5) — deferred to Story 14.3 (Request Routing)
  - [ ] 6.1 Route enqueue/ack/nack operations to the queue's Raft instance instead of the meta Raft
  - [ ] 6.2 Queue Raft state machine applies operations to the local scheduler/storage

- [x] Task 7: Integration tests (AC: 9)
  - [x] 7.1 Test: create queue on 3-node cluster → Raft group created, leader elected
  - [x] 7.2 Test: enqueue message through queue Raft → committed and applied
  - [x] 7.3 Test: delete queue → Raft group cleaned up
  - [x] 7.4 Test: multiple queues → leadership distributed across nodes

- [x] Task 8: Verify single-node mode unchanged (AC: 8)
  - [x] 8.1 Run existing test suite — 309 tests pass, zero regressions
  - [x] 8.2 Verify no Raft groups created when `cluster.enabled = false` — ClusterManager is not started

## Dev Notes

### Architecture

Story 14.2 evolves from 14.1's single meta Raft group to a multi-Raft architecture where each queue has its own Raft group. The meta Raft group remains for cluster-wide coordination (membership, queue group management), while individual queue Raft groups handle queue-specific operations (enqueue, ack, nack).

This follows the CockroachDB range pattern: one "meta" range for cluster coordination, many "data" ranges for actual workload.

### Key Design Decisions

**Multi-Raft over single Raft:** Per-queue groups allow independent leadership — queue A's leader failure doesn't affect queue B. This is the core scaling mechanism.

**Meta Raft for coordination:** Queue group creation/deletion flows through the meta Raft so all nodes learn about queue groups atomically. The meta state machine stores the placement table (queue → members mapping).

**Storage isolation via key prefixes:** Rather than creating a RocksDB column family per queue (unbounded), use the existing `raft_log` CF with key prefixes: `{queue_id}:vote`, `{queue_id}:log:{index}`, etc. This keeps the CF count fixed while providing logical isolation.

**Single gRPC endpoint for all groups:** Rather than opening a separate port per queue Raft group, multiplex all Raft RPCs through the existing cluster gRPC service using a `queue_id` discriminator field. This matches Pulsar's approach of multiplexing per-topic replication over a shared connection.

**openraft supports multi-Raft:** Each `Raft::new()` call creates an independent Raft instance. Multiple instances can coexist in the same process with different `node_id` + group_id combinations. openraft's `node_id` is `u64` — we can encode `(queue_group_hash, physical_node_id)` or use a separate Raft instance per queue with the same node_id (each instance is independent).

### Existing Seams from Epic 13

- `QueueRouter` (broker/router.rs) — returns `GroupId(queue_id)` in phase 1. This story doesn't change the router yet (that's Story 14.3), but the `GroupId` maps directly to the queue's Raft group.
- DRR key-set scoping — scheduler already accepts a key-set parameter. Not modified in this story.
- Storage trait abstraction — `StorageEngine` trait from Story 13.1 cleanly separates storage concerns.

### Proto Changes

Extend `cluster.proto`:
- Add `queue_id` field to `RaftRequest` (for multiplexing per-queue Raft RPCs)
- Keep `AddNode`/`RemoveNode` on the meta group

### Key Files to Modify

| File | Change |
|------|--------|
| `crates/fila-core/src/cluster/mod.rs` | Add `MultiRaftManager`, wire into `ClusterManager` |
| `crates/fila-core/src/cluster/types.rs` | Add `CreateQueueGroup`/`DeleteQueueGroup` to `ClusterRequest` |
| `crates/fila-core/src/cluster/store.rs` | Add `QueueRaftStore` with key-prefix isolation |
| `crates/fila-core/src/cluster/grpc_service.rs` | Multiplex Raft RPCs by `queue_id` |
| `crates/fila-core/src/cluster/network.rs` | Include `queue_id` in outgoing RPCs |
| `crates/fila-proto/proto/fila/v1/cluster.proto` | Add `queue_id` to `RaftRequest` |
| `crates/fila-core/src/cluster/tests.rs` | Add multi-raft integration tests |

### References

- [Source: _bmad/docs/research/decoupled-scheduler-sharded-storage.md#Section 9 - Phase 2 Viability Constraints]
- [Source: _bmad-output/planning-artifacts/epics.md#Story 14.2]
- [Source: crates/fila-core/src/broker/router.rs — QueueRouter seam from Epic 13]
- [Source: crates/fila-core/src/cluster/mod.rs — ClusterManager from Story 14.1]

## Dev Agent Record

### Agent Model Used

Claude Opus 4.6

### Debug Log References

None — clean implementation.

### Completion Notes List

- Tasks 5 (broker CRUD wiring) and 6 (write path routing) are deferred to Story 14.3 (Request Routing & Transparent Delivery) as they require the full request routing layer. Story 14.2 delivers the multi-Raft infrastructure; Story 14.3 wires it into the broker's hot path.
- Key design decision: key-prefixed isolation (`q:{queue_id}:` prefix) within single `raft_log` CF rather than per-queue column families. Keeps CF count bounded.
- `FilaNetworkFactory` is now per-group scoped (meta vs queue) and carries `group_id` in outgoing RPCs.
- Bootstrap logic: smallest node ID in the members list bootstraps the queue group to avoid conflicts.
- 309 total tests (up from 305), 4 new multi-Raft integration tests.
- Code review fix: `remove_group` now cleans up key-prefixed RocksDB data to prevent stale data on queue re-creation.
- Code review fix: Proto `RaftRequest.group_id` field number corrected from 3 to 2.

### File List

| File | Change |
|------|--------|
| `crates/fila-core/src/cluster/multi_raft.rs` | **NEW** — MultiRaftManager for per-queue Raft instances |
| `crates/fila-core/src/cluster/mod.rs` | Add multi_raft module, wire MultiRaftManager into ClusterManager |
| `crates/fila-core/src/cluster/types.rs` | Add CreateQueueGroup/DeleteQueueGroup to ClusterRequest/Response |
| `crates/fila-core/src/cluster/store.rs` | Refactor with KeyBuilder for prefixed key isolation, add queue_groups to StateMachineData |
| `crates/fila-core/src/cluster/network.rs` | Scope FilaNetworkFactory per-group, add group_id to RPCs |
| `crates/fila-core/src/cluster/grpc_service.rs` | Add resolve_raft() for group_id multiplexing |
| `crates/fila-core/src/cluster/tests.rs` | 4 new multi-Raft tests, refactored helpers |
| `crates/fila-proto/proto/fila/v1/cluster.proto` | Add group_id field to RaftRequest |
