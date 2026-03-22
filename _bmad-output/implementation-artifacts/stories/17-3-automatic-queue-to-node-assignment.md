# Story 17.3: Automatic Queue-to-Node Assignment

Status: ready-for-dev

## Story

As an operator,
I want the cluster to automatically distribute queue leadership across nodes,
so that I don't end up with all queues on one node after scaling or restarts.

## Acceptance Criteria

1. **Given** a new queue is created in a multi-node cluster
   **When** the system assigns which nodes participate in the queue's Raft group
   **Then** the assignment distributes leadership across available nodes (not all queues on the same leader)

2. **Given** a cluster with N nodes
   **When** multiple queues are created sequentially
   **Then** a load-aware or round-robin strategy selects the preferred leader based on current queue distribution

3. **Given** a cluster larger than the replication factor
   **When** a new queue is created
   **Then** the system selects which N nodes out of M participate in the queue's Raft group

4. **Given** the admin API
   **When** a client queries queue stats or cluster metadata
   **Then** the current queue-to-node mapping is exposed (which node leads each queue)

5. **Given** 6 queues created on a 3-node cluster
   **When** leadership is inspected
   **Then** no node has more than 3 queue leaderships (roughly balanced)

## Tasks / Subtasks

- [ ] Task 1: Implement preferred-leader selection in queue creation (AC: 1, 2)
  - [ ] Add `preferred_leader` field to `CreateQueueGroup` cluster request
  - [ ] Implement round-robin preferred leader selection in `admin_service.rs` `create_queue` handler
  - [ ] Track per-node queue leadership count in meta Raft state (`StateMachineData`)
  - [ ] Select preferred leader as the node with fewest current queue leaderships (tie-break: lowest node ID)
  - [ ] Modify `multi_raft.rs` `create_group()` to bootstrap on the preferred leader instead of always the lowest node ID

- [ ] Task 2: Implement node subset selection for large clusters (AC: 3)
  - [ ] When cluster size > replication factor, select a subset of nodes for the queue's Raft group
  - [ ] Default replication factor: 3 (configurable via `BrokerConfig`)
  - [ ] Subset selection: pick N least-loaded nodes from M available
  - [ ] Pass the selected subset (not all members) to `CreateQueueGroup`

- [ ] Task 3: Expose queue-to-node mapping in admin API (AC: 4)
  - [ ] Add leadership information to `QueueStats` response (leader node ID per queue)
  - [ ] Ensure `cluster_stats` / metadata endpoint shows queue-to-node mapping
  - [ ] This may already be partially available via existing `leader_node_id` in stats — verify and extend if needed

- [ ] Task 4: Unit and integration tests (AC: 1, 2, 3)
  - [ ] Test preferred-leader selection logic: given N nodes with varying queue counts, verify least-loaded is chosen
  - [ ] Test subset selection: given 5 nodes and replication factor 3, verify 3 nodes are selected
  - [ ] Test round-robin behavior: creating multiple queues distributes across nodes

- [ ] Task 5: E2E tests for balanced leadership (AC: 5)
  - [ ] Add e2e test: create 6 queues on 3-node cluster, verify no node has >3 leaderships
  - [ ] Add e2e test: create queues, kill a node, create more queues — new queues avoid dead node
  - [ ] Use existing `TestCluster` helper from `crates/fila-e2e/tests/helpers/cluster.rs`

## Dev Notes

### Current Behavior (the problem)

In `admin_service.rs:165`, `create_queue` calls `cluster.meta_members()` and passes **all** cluster members to every queue's Raft group. In `multi_raft.rs:88`, the node with the **lowest ID** always bootstraps (becomes initial leader). Result: node 1 leads every queue.

### Preferred Leader Mechanism

The key change is in `multi_raft.rs` `create_group()`. Currently:
```rust
let min_member = members.iter().map(|(id, _)| *id).fold(u64::MAX, u64::min);
if self.node_id == min_member { /* bootstrap */ }
```

Change this to use the `preferred_leader` field from `CreateQueueGroup`. The preferred leader bootstraps; other nodes start as learners/followers.

### State Tracking

Track queue-to-leader assignment in `StateMachineData` (in `cluster/store.rs`). The existing `queue_groups: HashMap<String, Vec<u64>>` maps queue→members. Extend or add a parallel map for preferred leaders, or just use the first element of the member list as the preferred leader convention.

### Replication Factor

Add `replication_factor` to `BrokerConfig` (default: 3). When cluster size ≤ replication factor, all nodes participate. When cluster size > replication factor, select a subset.

### Key Files to Modify

- `crates/fila-core/src/cluster/types.rs` — Add `preferred_leader: NodeId` to `CreateQueueGroup`
- `crates/fila-core/src/cluster/multi_raft.rs` — Use preferred_leader for bootstrap decision
- `crates/fila-core/src/cluster/store.rs` — Track per-node leadership counts in `StateMachineData`
- `crates/fila-server/src/admin_service.rs` — Implement assignment strategy in `create_queue`
- `crates/fila-core/src/cluster/proto_convert.rs` — Serialize new preferred_leader field
- `crates/fila-proto/proto/fila/v1/cluster.proto` — Add preferred_leader to ClusterCreateQueueGroup message
- `crates/fila-e2e/tests/cluster.rs` — Add balanced-leadership e2e tests

### Patterns from Previous Stories

- **Error types** (17.1): Any new error variants must follow per-command error type pattern
- **Proto changes** (17.1, 14.6): Add new proto fields as optional to maintain backward compatibility
- **E2E test patterns** (16.5.1): Use `TestCluster::start(3)`, `find_leader_index()`, timing constants from existing cluster tests
- **Config additions** (various): Add to `BrokerConfig` with sensible defaults, document in `docs/configuration.md`

### What NOT to do

- Do NOT implement dynamic rebalancing (moving leadership of existing queues when nodes join/leave) — that's future work
- Do NOT change the Raft election mechanism itself — only control which node bootstraps initially
- Do NOT break single-node mode — when there's only 1 node, it must still work identically

### Project Structure Notes

- Cluster code lives in `crates/fila-core/src/cluster/`
- Proto definitions in `crates/fila-proto/proto/fila/v1/`
- E2E tests in `crates/fila-e2e/tests/`
- Server-side RPC handlers in `crates/fila-server/src/`

### References

- [Source: _bmad/docs/research/decoupled-scheduler-sharded-storage.md#Section 9] — queue-to-leader assignment vision
- [Source: GitHub issue #66] — full problem statement
- [Source: _bmad-output/planning-artifacts/epics.md#Epic 17] — acceptance criteria
- [Source: crates/fila-core/src/cluster/multi_raft.rs#L62-100] — current bootstrap logic
- [Source: crates/fila-server/src/admin_service.rs#L104-200] — current create_queue handler

## Dev Agent Record

### Agent Model Used

### Debug Log References

### Completion Notes List

### File List
