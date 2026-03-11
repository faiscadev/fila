# Story 14.4: Replication, Failover & Recovery

Status: ready-for-dev

## Story

As an operator,
I want automatic failover when a node goes down,
so that message processing continues without manual intervention or data loss.

## Acceptance Criteria

1. **Given** a multi-node cluster with queue-level Raft groups, **when** data is written for a queue, **then** the Raft leader replicates everything via its Raft log: message data, acks, nacks — followers have replicated state at all times, and writes are committed only after a quorum acknowledges.

2. **Given** a node fails unexpectedly, **when** the Raft followers detect the failure via heartbeat timeout, **then** a new leader is elected from followers within 1-2 seconds, and automatic failover completes within 10 seconds.

3. **Given** a consumer stream is connected to the failed node, **when** the node goes down, **then** the consumer receives a disconnection, and can reconnect to a healthy node within 5 seconds. In-flight messages are governed by their visibility timeout (at-least-once delivery preserved).

4. **Given** a failed node recovers, **when** it rejoins the cluster, **then** it catches up from the Raft log (or receives a Raft snapshot if too far behind), and cluster state converges within 30 seconds.

5. **Given** a 3-node cluster, **when** integration tests run, **then** they verify: kill one node → failover completes → zero message loss → restart node → rejoin and convergence.

6. **Given** a queue's Raft leader changes (failover or rebalance), **when** the new leader starts, **then** it rebuilds its in-memory scheduler state (DRR active keys, pending index, leased message tracking) from the local storage so the scheduler can serve consumers immediately.

7. **Given** a Fila node is running in single-node mode, **when** operations are submitted, **then** behavior is unchanged from 14.3 (zero overhead, no Raft path).

## Tasks / Subtasks

- [ ] Task 1: Leader change detection and scheduler rebuild (AC: 2, 6)
  - [ ] 1.1 Add `LeaderChangeWatcher` that monitors `Raft::metrics()` for leader changes on queue-level Raft groups via `watch::Receiver`
  - [ ] 1.2 When this node becomes leader for a queue, trigger scheduler recovery for that queue: rebuild DRR keys, pending index, leased_msg_keys from RocksDB (reuse `Scheduler::recover()` logic scoped to one queue)
  - [ ] 1.3 When this node loses leadership for a queue, drain consumer streams for that queue (drop `ready_tx` channels) so consumers reconnect to the new leader
  - [ ] 1.4 Wire `LeaderChangeWatcher` startup in `ClusterManager` / main.rs

- [ ] Task 2: Consumer stream leader-awareness (AC: 3)
  - [ ] 2.1 In `HotPathService::consume`, if cluster mode is on, check `ClusterHandle::is_queue_leader()` and return `UNAVAILABLE` with leader hint if this node is not the leader
  - [ ] 2.2 Ensure the consume converter task (spawned in `service.rs`) detects when the `ready_tx` channel is dropped (leader loss) and sends an error to the stream before closing

- [ ] Task 3: Failover integration tests (AC: 2, 3, 5)
  - [ ] 3.1 Test: 3-node cluster, enqueue messages, kill leader node, verify new leader elected within 10 seconds, verify enqueue still works on remaining nodes
  - [ ] 3.2 Test: kill leader, verify consumer can reconnect to a surviving node and receive messages
  - [ ] 3.3 Test: verify zero message loss — enqueue N messages before kill, consume all N after failover

- [ ] Task 4: Node rejoin and convergence (AC: 4)
  - [ ] 4.1 Test: kill a node, enqueue messages on surviving cluster, restart the killed node, verify it catches up via Raft log
  - [ ] 4.2 Verify `install_snapshot` path works when the rejoined node is too far behind (the MetaStoreEvent emission from 14.3's Cubic fix covers this)

- [ ] Task 5: Verify single-node mode unchanged (AC: 7)
  - [ ] 5.1 Run existing test suite — all tests pass, zero regressions
  - [ ] 5.2 Verify no new overhead when `cluster.enabled = false`

## Dev Notes

### Architecture

Story 14.4 adds the operational resilience layer on top of 14.1-14.3's Raft infrastructure. The key insight: openraft already replicates all committed log entries to followers — the data replication AC is already satisfied by the existing Raft write path. What's missing is the **leader change handling**: when leadership moves (failover or rebalance), the new leader must rebuild its in-memory scheduler state and take over consumer stream delivery.

### Key Design Decisions

**Leader change detection via metrics watch:** openraft's `Raft::metrics()` returns a `watch::Receiver<RaftMetrics>` that updates whenever Raft state changes (leader ID, term, membership). Polling `metrics.current_leader` on each change lets us detect when this node gains or loses leadership for a queue group — without needing custom events from the Raft state machine.

**Per-queue scheduler rebuild on leader promotion:** When this node becomes leader for a queue, it must rebuild the in-memory DRR keys, pending index, and leased_msg_keys for that queue. The existing `Scheduler::recover()` does this for all queues at startup. For leader change, we need a scoped version: `recover_queue(queue_id)` that rebuilds only the state for the newly-led queue. This avoids full-scheduler disruption.

**Consumer stream teardown on leader loss:** When this node loses leadership for a queue, all active consumer streams for that queue must be torn down so consumers reconnect to the new leader. The simplest mechanism: drop the `ready_tx` channel held by the scheduler for each consumer of that queue. The converter task in `service.rs` already handles this: when `ready_rx.recv()` returns `None`, the stream closes with an error.

**Node kill in tests:** `FullTestNode` from 14.3 has a `shutdown_tx` field. For "kill" semantics, drop the node's Raft instances and gRPC server without graceful shutdown. Then for "restart", create a new `FullTestNode` with the same `node_id` and RocksDB data directory — the Raft log is durable in RocksDB, so the node catches up from its last committed entry.

### Existing Infrastructure

- `Raft::metrics()` → `watch::Receiver<RaftMetrics>` — built into openraft, includes `current_leader: Option<NodeId>`
- `MultiRaftManager::groups` → `RwLock<HashMap<String, Arc<Raft<TypeConfig>>>>` — iterate to watch all queue groups
- `Scheduler::recover()` in `recovery.rs` — rebuilds all queues from storage. Need to extract per-queue logic.
- `FullTestNode` in `tests.rs` — test harness with 3-node cluster support. Reuse for failover tests.
- `install_snapshot()` in `store.rs` — already emits MetaStoreEvents for queue groups (14.3 Cubic fix), so snapshot-based catchup triggers queue creation.
- `ClusterHandle::is_queue_leader()` — already exists, used in Task 2.

### Key Files to Modify

| File | Change |
|------|--------|
| `crates/fila-core/src/cluster/mod.rs` | Add `LeaderChangeWatcher`, wire startup |
| `crates/fila-core/src/cluster/multi_raft.rs` | Expose iterator over groups for leader watching |
| `crates/fila-core/src/broker/scheduler/recovery.rs` | Extract per-queue recovery: `recover_queue(queue_id)` |
| `crates/fila-core/src/broker/scheduler/mod.rs` | Add `SchedulerCommand::RecoverQueue` or method for scoped recovery |
| `crates/fila-server/src/service.rs` | Add leader check in `consume()` handler |
| `crates/fila-core/src/cluster/tests.rs` | Failover integration tests |

### References

- [Source: _bmad-output/planning-artifacts/epics.md#Story 14.4]
- [Source: crates/fila-core/src/broker/scheduler/recovery.rs — existing recover() logic]
- [Source: crates/fila-core/src/cluster/mod.rs — ClusterHandle, process_meta_events]
- [Source: crates/fila-core/src/cluster/store.rs — install_snapshot MetaStoreEvent emission]
- [Source: crates/fila-core/src/cluster/tests.rs — FullTestNode harness]
- [Source: crates/fila-server/src/service.rs — consume stream handler]

## Dev Agent Record

### Agent Model Used

Claude Opus 4.6

### Debug Log References

### Completion Notes List

### Change Log

### File List
