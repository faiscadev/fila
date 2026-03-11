# Story 14.2: Queue-Level Raft Groups & Assignment

Status: ready-for-dev

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

- [ ] Task 1: Multi-Raft manager for per-queue Raft groups (AC: 1, 2, 3, 6, 7)
  - [ ] 1.1 Create `MultiRaftManager` struct that manages multiple `Raft<TypeConfig>` instances (one per queue)
  - [ ] 1.2 Add `create_queue_group(queue_id, members)` — creates a new Raft instance for the queue, starts its gRPC handlers
  - [ ] 1.3 Add `remove_queue_group(queue_id)` — shuts down and cleans up a queue's Raft group
  - [ ] 1.4 Add `get_queue_raft(queue_id)` — returns the Raft instance for a queue (for submitting writes)
  - [ ] 1.5 Store queue→Raft group mappings in the meta Raft state machine (so all nodes learn about queue groups through Raft replication)

- [ ] Task 2: Extend meta Raft state machine for queue group management (AC: 1, 3, 4)
  - [ ] 2.1 Add `CreateQueueGroup { queue_id, members }` and `DeleteQueueGroup { queue_id }` variants to `ClusterRequest`
  - [ ] 2.2 When the meta state machine applies `CreateQueueGroup`, trigger local multi-raft manager to create the Raft instance
  - [ ] 2.3 When the meta state machine applies `DeleteQueueGroup`, trigger local multi-raft manager to shut down the Raft instance
  - [ ] 2.4 Track active queue groups in `StateMachineData` for snapshot/restore

- [ ] Task 3: Per-queue Raft storage isolation (AC: 5)
  - [ ] 3.1 Create a RocksDB column family per queue Raft group (e.g., `raft_log:{queue_id}`) or use key-prefixed isolation within the existing `raft_log` CF
  - [ ] 3.2 Create a `QueueRaftStore` that wraps `FilaRaftStore` with queue-scoped key prefixes
  - [ ] 3.3 Ensure separate vote, log, purge, and snapshot state per queue group

- [ ] Task 4: Queue-level gRPC multiplexing (AC: 5)
  - [ ] 4.1 Extend the cluster gRPC service to include a `queue_id` field in Raft RPCs (so a single gRPC endpoint serves all queue groups)
  - [ ] 4.2 Route incoming Raft RPCs to the correct queue's Raft instance based on `queue_id`
  - [ ] 4.3 Extend `FilaNetwork` to include `queue_id` in outgoing RPCs

- [ ] Task 5: Wire queue CRUD to multi-raft lifecycle (AC: 1, 3, 4, 8)
  - [ ] 5.1 When `Broker::create_queue` is called in cluster mode, submit `CreateQueueGroup` to the meta Raft leader
  - [ ] 5.2 When `Broker::delete_queue` is called in cluster mode, submit `DeleteQueueGroup` to the meta Raft leader
  - [ ] 5.3 In single-node mode (cluster disabled), bypass all Raft group management (AC 8)

- [ ] Task 6: Write path through queue Raft (AC: 5)
  - [ ] 6.1 Route enqueue/ack/nack operations to the queue's Raft instance instead of the meta Raft
  - [ ] 6.2 Queue Raft state machine applies operations to the local scheduler/storage

- [ ] Task 7: Integration tests (AC: 9)
  - [ ] 7.1 Test: create queue on 3-node cluster → Raft group created, leader elected
  - [ ] 7.2 Test: enqueue message through queue Raft → committed and applied
  - [ ] 7.3 Test: delete queue → Raft group cleaned up
  - [ ] 7.4 Test: multiple queues → leadership distributed across nodes

- [ ] Task 8: Verify single-node mode unchanged (AC: 8)
  - [ ] 8.1 Run existing test suite — zero regressions
  - [ ] 8.2 Verify no Raft groups created when `cluster.enabled = false`

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

### Completion Notes List

### File List
