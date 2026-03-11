# Story 14.3: Request Routing & Transparent Delivery

Status: ready-for-dev

## Story

As a consumer or producer,
I want to connect to any Fila node and have my requests served correctly,
so that I don't need to know which node is the leader for which queue.

## Acceptance Criteria

1. **Given** a client sends an Enqueue request to any node in the cluster, **when** the receiving node is not the queue's Raft leader, **then** the request is forwarded to the leader, committed through the queue's Raft log, applied to the scheduler, and the client receives a normal success response (transparent routing).

2. **Given** a client sends an Enqueue request to the queue's Raft leader directly, **when** the message is committed through the queue's Raft log, **then** the message is applied to the local scheduler/storage and the response is returned with zero extra hops.

3. **Given** a client sends an Ack or Nack request to any node, **when** the receiving node is not the queue's Raft leader, **then** the request is forwarded to the leader and processed normally.

4. **Given** an operator creates or deletes a queue on any node, **when** the receiving node is not the meta Raft leader, **then** the request is forwarded to the meta leader, a `CreateQueueGroup`/`DeleteQueueGroup` is committed, and all nodes start/stop the queue's Raft group.

5. **Given** a queue's Raft group commits an Enqueue, Ack, or Nack entry, **when** the state machine applies it, **then** the operation is executed on the local scheduler/storage (not just acknowledged — real side effects).

6. **Given** a Fila node is running in single-node mode (cluster disabled), **when** operations are submitted, **then** they go directly to the local scheduler with zero overhead (no Raft path, no routing indirection).

7. **Given** a 3-node cluster, **when** integration tests run, **then** they verify: enqueue on node A is consumable from node B, ack on node C completes the lifecycle, queue creation/deletion propagates to all nodes.

## Tasks / Subtasks

- [ ] Task 1: Wire queue CRUD to multi-raft lifecycle (AC: 4, 6) — deferred from 14.2 Task 5
  - [ ] 1.1 Pass `ClusterManager` reference to `AdminService` and `HotPathService`
  - [ ] 1.2 When `AdminService::create_queue` is called in cluster mode, submit `CreateQueueGroup` to the meta Raft leader; on commit, each node's state machine triggers `MultiRaftManager::create_group()`
  - [ ] 1.3 When `AdminService::delete_queue` is called in cluster mode, submit `DeleteQueueGroup` to the meta Raft leader; on commit, each node triggers `MultiRaftManager::remove_group()`
  - [ ] 1.4 If the receiving node is not the meta leader, forward the request to the leader (use `ForwardToLeader` error's leader hint)

- [ ] Task 2: Queue Raft state machine applies operations to scheduler (AC: 5, 6) — deferred from 14.2 Task 6
  - [ ] 2.1 Give `FilaRaftStore` access to the broker's scheduler command channel (for queue-level stores)
  - [ ] 2.2 When `apply_to_state_machine` handles `Enqueue`, send `SchedulerCommand::Enqueue` to the broker
  - [ ] 2.3 When `apply_to_state_machine` handles `Ack`/`Nack`, send corresponding `SchedulerCommand`
  - [ ] 2.4 Meta Raft state machine continues to just track state (no scheduler routing)

- [ ] Task 3: Request routing in service handlers (AC: 1, 2, 3, 6)
  - [ ] 3.1 In `HotPathService::enqueue`, check cluster mode: if enabled, submit `ClusterRequest::Enqueue` to the queue's Raft leader via `client_write()`; if not leader, openraft's `ForwardToLeader` provides the leader address
  - [ ] 3.2 In `HotPathService::ack` and `nack`, same pattern: route to queue Raft leader
  - [ ] 3.3 Implement leader forwarding: when `client_write()` returns `ForwardToLeader`, create a gRPC client to the leader and retry the request there
  - [ ] 3.4 In single-node mode, bypass Raft entirely — send directly to scheduler (existing path)

- [ ] Task 4: Integration tests (AC: 7)
  - [ ] 4.1 Test: enqueue on node A, consume on node B — message delivered across nodes
  - [ ] 4.2 Test: ack on node C — full lifecycle across 3 nodes
  - [ ] 4.3 Test: create queue on non-leader node — propagates to all nodes
  - [ ] 4.4 Test: delete queue on non-leader node — cleaned up on all nodes

- [ ] Task 5: Verify single-node mode unchanged (AC: 6)
  - [ ] 5.1 Run existing test suite — verify zero regressions
  - [ ] 5.2 Verify no Raft involvement when `cluster.enabled = false`

## Dev Notes

### Architecture

Story 14.3 bridges the gap between the multi-Raft infrastructure (14.2) and the broker's service handlers. The key insight: in cluster mode, all write operations (enqueue, ack, nack, create/delete queue) must flow through Raft consensus before being applied to the local scheduler. Read operations (consume/stream) connect to the queue's Raft leader, which runs the DRR scheduler locally.

### Request Flow (Cluster Mode)

```
Client → any node → resolve queue leader → if local: client_write() → Raft commit → apply_to_state_machine → scheduler
                                          → if remote: forward to leader node → same flow on leader
```

### Key Design Decisions

**Leader forwarding via openraft:** When `client_write()` fails with `ForwardToLeader { leader_node }`, the error includes the leader's address. The service handler creates a gRPC client to that address and retries the request. This is transparent to the client.

**State machine → scheduler bridge:** `FilaRaftStore` for queue groups needs a reference to the broker's command sender (`mpsc::Sender<SchedulerCommand>`). When `apply_to_state_machine` processes an Enqueue/Ack/Nack, it sends the corresponding command to the scheduler. This means the `FilaRaftStore` for queue groups is structurally different from the meta store — it has a scheduler_tx field.

**Consume streams:** For 14.3, the consume stream connects to the node that has the queue's Raft leader. If the client connects to a follower, the service should return an error indicating which node is the leader (or proxy — but proxying streams adds complexity). Initial implementation: return leader hint, let client reconnect.

**Queue CRUD coordination:** When `CreateQueueGroup` is committed in the meta Raft log, each node's state machine updates `queue_groups`. A separate watcher/callback on the meta state machine triggers `MultiRaftManager::create_group()` on each node. The simplest approach: the node that receives the queue creation request handles the meta Raft write, then after commit, triggers local group creation. Other nodes will create their groups when they apply the same log entry.

### Existing Seams

- `QueueRouter` (broker/router.rs) — returns `GroupId(queue_id)` in phase 1. Maps directly to the queue's Raft group.
- `ClusterManager` holds `multi_raft: Arc<MultiRaftManager>` — accessible via `multi_raft()`.
- `MultiRaftManager::get_raft(queue_id)` — returns the local Raft instance for a queue.
- `ClusterRequest::CreateQueueGroup`/`DeleteQueueGroup` — already in types.rs.
- `StateMachineData::queue_groups` — already tracks active groups in meta Raft.

### Key Files to Modify

| File | Change |
|------|--------|
| `crates/fila-server/src/main.rs` | Pass `ClusterManager` to service constructors |
| `crates/fila-server/src/service.rs` | Add Raft routing logic to enqueue/ack/nack handlers |
| `crates/fila-server/src/admin_service.rs` | Wire create_queue/delete_queue to meta Raft |
| `crates/fila-core/src/cluster/store.rs` | Add scheduler_tx to queue-level stores, apply operations to scheduler |
| `crates/fila-core/src/cluster/multi_raft.rs` | Accept scheduler_tx in create_group, pass to FilaRaftStore |
| `crates/fila-core/src/cluster/mod.rs` | Expose scheduler integration points |
| `crates/fila-core/src/cluster/tests.rs` | New routing integration tests |

### References

- [Source: _bmad/docs/research/decoupled-scheduler-sharded-storage.md#Section 5.1, 6.2, 9]
- [Source: _bmad-output/planning-artifacts/epics.md#Story 14.3]
- [Source: _bmad-output/implementation-artifacts/stories/14-2-queue-level-raft-groups-assignment.md — Tasks 5, 6]
- [Source: crates/fila-core/src/broker/router.rs — QueueRouter seam from Epic 13]
- [Source: crates/fila-core/src/cluster/mod.rs — ClusterManager from Story 14.1]
- [Source: crates/fila-core/src/cluster/multi_raft.rs — MultiRaftManager from Story 14.2]

## Dev Agent Record

### Agent Model Used

Claude Opus 4.6

### Debug Log References

### Completion Notes List

### File List
