# Story 14.1: Raft Integration & Single-Node Mode

Status: ready-for-dev

## Story

As a developer,
I want Raft consensus embedded in the Fila binary with zero overhead in single-node mode,
so that clustering is built into the same binary without affecting existing single-node deployments.

## Acceptance Criteria

1. **Given** Fila runs as a single binary **When** cluster mode is configured **Then** Fila embeds a Raft consensus implementation (openraft) compiled into the same binary — no external consensus service required.
2. **Given** cluster configuration is specified via `fila.toml` **Then** the following settings are supported: `cluster.enabled` (bool, default false), `cluster.node_id` (u64), `cluster.peers` (list of peer addresses), `cluster.bind_addr` (intra-cluster listen address), `cluster.bootstrap` (bool, for initial cluster formation).
3. **Given** `cluster.bootstrap = true` on a single node **Then** a single-node cluster can be bootstrapped (the node initializes membership containing only itself).
4. **Given** `cluster.bootstrap = false` and `cluster.peers` is set **Then** additional nodes join an existing cluster by contacting seed peers.
5. **Given** a cluster is formed **Then** leader election completes within the Raft election timeout (configurable, default 1 second).
6. **Given** a running cluster **Then** cluster membership changes (add/remove node) are committed via Raft log entries.
7. **Given** a Raft state machine **Then** committed entries apply to local state: message writes, DRR state, leases, pending index, config — one Raft log per queue replicating everything.
8. **Given** intra-cluster communication **Then** a dedicated gRPC service (separate from client-facing RPCs) handles Raft RPCs (vote, append_entries, snapshot).
9. **Given** `cluster.enabled = false` (the default) **Then** single-node mode continues to work exactly as before — zero Raft overhead, zero behavior change.
10. **Given** a cluster is running **Then** integration tests verify: 3-node cluster bootstrap, leader election, membership change (add 4th node, remove a node).

## Tasks / Subtasks

- [ ] Task 1: Add openraft dependency and define Raft type configuration (AC: 1)
  - [ ] 1.1: Add `openraft = "0.9"` to workspace dependencies in root `Cargo.toml` and to `fila-core/Cargo.toml`
  - [ ] 1.2: Create `crates/fila-core/src/cluster/` module with `mod.rs`, `types.rs`
  - [ ] 1.3: Define `FilaTypeConfig` implementing `openraft::RaftTypeConfig` with: `NodeId = u64`, `Node = FilaNode` (wraps address info), `Entry = openraft::Entry<FilaTypeConfig>`, `D = ClusterRequest` (serializable command enum), `R = ClusterResponse`, `SnapshotData = Cursor<Vec<u8>>`, `AsyncRuntime = TokioRuntime`
  - [ ] 1.4: Define `ClusterRequest` enum covering all state-mutating operations: `Enqueue { message }`, `Ack { queue_id, msg_id }`, `Nack { queue_id, msg_id, error }`, `CreateQueue { name, config }`, `DeleteQueue { queue_id }`, `SetConfig { key, value }`, `SetThrottleRate { key, rate, burst }`, `RemoveThrottleRate { key }`, `Redrive { dlq_queue_id, count }`
  - [ ] 1.5: Define `ClusterResponse` enum for return values from each command variant

- [ ] Task 2: Implement Raft state machine backed by StorageEngine (AC: 7)
  - [ ] 2.1: Create `crates/fila-core/src/cluster/state_machine.rs`
  - [ ] 2.2: Implement `RaftStateMachine` for `FilaStateMachine` wrapping `Arc<dyn StorageEngine>` + in-memory scheduler state
  - [ ] 2.3: `apply()` method: deserialize `ClusterRequest` from committed log entry, execute the corresponding operation on storage/scheduler state, return `ClusterResponse`
  - [ ] 2.4: `snapshot()` method: serialize current storage + scheduler state to snapshot bytes
  - [ ] 2.5: `install_snapshot()` method: restore storage + scheduler state from snapshot bytes
  - [ ] 2.6: `applied_state()` / `get_current_snapshot()`: return last applied log id and snapshot metadata

- [ ] Task 3: Implement Raft log storage backed by RocksDB (AC: 1)
  - [ ] 3.1: Create `crates/fila-core/src/cluster/log_store.rs`
  - [ ] 3.2: Add a dedicated `raft_log` column family to `RocksDbEngine` for Raft log entries and vote state
  - [ ] 3.3: Implement `RaftLogStorage` trait: `save_vote()`, `read_vote()`, `get_log_state()`, `try_get_log_entries()`, `append()`, `truncate()`, `purge()`
  - [ ] 3.4: Raft log entries stored as serialized bytes keyed by log index (u64 big-endian)

- [ ] Task 4: Implement Raft network transport via gRPC (AC: 8)
  - [ ] 4.1: Create `crates/fila-proto/proto/fila/v1/cluster.proto` with `FilaCluster` service: `Vote(VoteRequest) returns (VoteResponse)`, `AppendEntries(AppendEntriesRequest) returns (AppendEntriesResponse)`, `Snapshot(stream SnapshotChunk) returns (SnapshotResponse)`, `AddNode(AddNodeRequest) returns (AddNodeResponse)`, `RemoveNode(RemoveNodeRequest) returns (RemoveNodeResponse)`
  - [ ] 4.2: Create `crates/fila-core/src/cluster/network.rs` — implement `RaftNetworkFactory` and `RaftNetwork` traits using tonic gRPC clients
  - [ ] 4.3: Create `crates/fila-core/src/cluster/grpc_service.rs` — implement `FilaCluster` gRPC service that forwards RPCs to the local openraft `Raft` instance
  - [ ] 4.4: Intra-cluster gRPC service listens on `cluster.bind_addr` (separate port from client-facing service)

- [ ] Task 5: Extend configuration with cluster settings (AC: 2)
  - [ ] 5.1: Add `ClusterConfig` struct to `crates/fila-core/src/broker/config.rs`: `enabled: bool` (default false), `node_id: u64`, `peers: Vec<String>`, `bind_addr: String` (default "0.0.0.0:5556"), `bootstrap: bool` (default false), `election_timeout_ms: u64` (default 1000), `heartbeat_interval_ms: u64` (default 300), `snapshot_threshold: u64` (default 10000)
  - [ ] 5.2: Add `cluster: ClusterConfig` field to `BrokerConfig`
  - [ ] 5.3: Ensure `cluster.enabled = false` by default — serde default

- [ ] Task 6: Wire Raft into server startup with conditional initialization (AC: 3, 4, 9)
  - [ ] 6.1: In `fila-server/src/main.rs`, conditionally initialize Raft only when `cluster.enabled = true`
  - [ ] 6.2: When `cluster.enabled = false`: existing code path unchanged, no Raft objects created, zero overhead
  - [ ] 6.3: When `cluster.enabled = true` and `cluster.bootstrap = true`: create Raft instance, initialize membership with self, start cluster gRPC service on `bind_addr`
  - [ ] 6.4: When `cluster.enabled = true` and `cluster.bootstrap = false`: create Raft instance, contact seed peers to join cluster, start cluster gRPC service
  - [ ] 6.5: Graceful shutdown: if Raft active, shut down Raft before broker

- [ ] Task 7: Implement membership change operations (AC: 5, 6)
  - [ ] 7.1: `AddNode` RPC handler: call `raft.change_membership()` to add a new voter
  - [ ] 7.2: `RemoveNode` RPC handler: call `raft.change_membership()` to remove a voter
  - [ ] 7.3: Node join flow: new node contacts a seed peer via `AddNode` RPC, seed peer's Raft leader processes the membership change
  - [ ] 7.4: Handle leader forwarding — if contacted node is not leader, return leader address so caller can retry

- [ ] Task 8: Integration tests for cluster lifecycle (AC: 10)
  - [ ] 8.1: Create `crates/fila-core/src/cluster/tests.rs` (or `tests/` directory)
  - [ ] 8.2: Test: 3-node cluster bootstrap — start 3 nodes, verify leader elected within timeout
  - [ ] 8.3: Test: leader election — kill leader, verify new leader elected within election timeout
  - [ ] 8.4: Test: add 4th node — start a 4th node, join cluster, verify membership updated
  - [ ] 8.5: Test: remove a node — remove a node from membership, verify cluster continues operating
  - [ ] 8.6: All existing tests continue to pass (single-node mode unaffected)

- [ ] Task 9: Verify single-node mode is completely unaffected (AC: 9)
  - [ ] 9.1: Run full test suite with default config (cluster.enabled = false)
  - [ ] 9.2: Run e2e tests — all 11 pass
  - [ ] 9.3: Run clippy — zero warnings

## Dev Notes

### Architecture Decision: openraft

Use `openraft` (v0.9.x stable) — the most mature Rust Raft implementation, used by Databend and SurrealDB. Key traits to implement:

| openraft Trait | Fila Implementation | Purpose |
|---|---|---|
| `RaftTypeConfig` | `FilaTypeConfig` | Type parameterization |
| `RaftStateMachine` | `FilaStateMachine` | Apply committed log entries to storage |
| `RaftLogStorage` | `FilaLogStore` (RocksDB-backed) | Persist Raft log + vote state |
| `RaftNetwork` | `FilaNetwork` (gRPC) | Inter-node communication |
| `RaftNetworkFactory` | `FilaNetworkFactory` | Create network connections to peers |

### Critical Design: ClusterRequest as Raft Log Entry

Every state-mutating operation becomes a `ClusterRequest` serialized into the Raft log. This is the CockroachDB-style model from the research doc — one Raft log per queue replicating everything (storage + scheduler state + leases + DRR).

**In this story (14.1):** There is ONE Raft group for the entire cluster (a "meta" group managing cluster membership). Story 14.2 introduces per-queue Raft groups.

**Key constraint:** `ClusterRequest` and `ClusterResponse` must be `serde::Serialize + serde::Deserialize`. All domain types (`Message`, `QueueConfig`) already derive Serialize/Deserialize.

### Single-Node Mode: Zero Overhead

When `cluster.enabled = false`:
- No Raft objects are created
- No cluster gRPC service is started
- No additional threads or tokio tasks
- Broker, Scheduler, StorageEngine work exactly as today
- The only cost is the compiled-in openraft code (binary size), which is acceptable

### Conditional Compilation Pattern

```rust
// In main.rs startup:
let cluster = if config.cluster.enabled {
    Some(ClusterManager::new(config.cluster, storage.clone(), broker.clone()).await?)
} else {
    None
};
```

The `ClusterManager` encapsulates all Raft lifecycle. When `None`, zero overhead.

### RocksDB Column Family for Raft Log

Add a `raft_log` column family to `RocksDbEngine` for Raft-specific data:
- Key: `vote` → serialized vote state
- Key: `log:{index}` (u64 BE) → serialized log entry
- Key: `snapshot_meta` → last snapshot metadata

This keeps Raft log storage separate from application data. The existing `apply_mutations()` method on `StorageEngine` is what the state machine uses to apply committed entries.

### Cluster gRPC Service Design

The cluster service runs on a **separate port** (default 5556) from the client-facing service (5555). This is standard practice (CockroachDB uses 26257 for SQL, 26258 for internal).

Proto messages for Raft RPCs should wrap openraft's internal types. The `VoteRequest`, `AppendEntriesRequest`, etc. are serialized versions of openraft's corresponding types.

### Node Join Flow

```
Node 4 starts with cluster.peers = ["node1:5556", "node2:5556"]
  ↓
Node 4 contacts node1:5556 via AddNode RPC
  ↓
If node1 is leader: processes membership change via Raft
If node1 is NOT leader: returns leader address
  ↓
Node 4 retries with actual leader
  ↓
Leader commits membership change
  ↓
Node 4 is now a member, starts receiving Raft log entries
```

### Error Types

Follow the per-command error pattern from CLAUDE.md:

```rust
pub enum ClusterError {
    NotEnabled,
    NotLeader { leader_addr: Option<String> },
    RaftError(String),  // openraft errors are complex; wrap as string for now
    NetworkError(String),
}
```

### Key Files

| File | Action |
|------|--------|
| `Cargo.toml` (root) | Add `openraft` to workspace deps |
| `crates/fila-core/Cargo.toml` | Add `openraft` dependency |
| `crates/fila-core/src/cluster/mod.rs` | **NEW** — module root, `ClusterManager` |
| `crates/fila-core/src/cluster/types.rs` | **NEW** — `FilaTypeConfig`, `ClusterRequest`, `ClusterResponse`, `FilaNode` |
| `crates/fila-core/src/cluster/state_machine.rs` | **NEW** — `RaftStateMachine` impl |
| `crates/fila-core/src/cluster/log_store.rs` | **NEW** — `RaftLogStorage` impl (RocksDB) |
| `crates/fila-core/src/cluster/network.rs` | **NEW** — `RaftNetwork` + `RaftNetworkFactory` impl (gRPC) |
| `crates/fila-core/src/cluster/grpc_service.rs` | **NEW** — `FilaCluster` gRPC service handler |
| `crates/fila-core/src/cluster/tests.rs` | **NEW** — Integration tests |
| `crates/fila-core/src/lib.rs` | Add `pub mod cluster;` |
| `crates/fila-core/src/broker/config.rs` | Add `ClusterConfig` struct |
| `crates/fila-core/src/storage/rocksdb.rs` | Add `raft_log` column family |
| `crates/fila-proto/proto/fila/v1/cluster.proto` | **NEW** — Cluster gRPC service definition |
| `crates/fila-proto/build.rs` | Add cluster.proto to build |
| `crates/fila-server/src/main.rs` | Conditional Raft init, cluster gRPC service |

### What NOT to Do

- Do NOT create per-queue Raft groups yet — that's Story 14.2. This story has ONE meta Raft group for cluster membership and basic consensus.
- Do NOT implement request routing or forwarding — that's Story 14.3.
- Do NOT implement failover or recovery logic — that's Story 14.4.
- Do NOT change any existing scheduler logic — the scheduler continues to run as-is.
- Do NOT add queue-level leadership tracking — that's Story 14.2.
- Do NOT modify the existing client-facing gRPC service — it stays unchanged.
- Do NOT use `openraft` features that are alpha/unstable — stick with 0.9.x stable API.

### References

- [Source: _bmad/docs/research/decoupled-scheduler-sharded-storage.md] — Full clustering architecture, CockroachDB-style model, HA design
- [Source: _bmad-output/planning-artifacts/epics.md#story-14.1] — Epic ACs
- [Source: crates/fila-core/src/storage/traits.rs] — StorageEngine trait with `apply_mutations()` ready for Raft
- [Source: crates/fila-core/src/broker/config.rs] — BrokerConfig to extend with ClusterConfig
- [Source: crates/fila-core/src/broker/scheduler/handlers.rs:109,184] — TODO(cluster) comments for distributed atomicity
- [Source: crates/fila-core/src/broker/router.rs] — QueueRouter seam from Story 13.2
- [Source: _bmad-output/implementation-artifacts/stories/13-2-phase-2-viability-seams.md] — Previous story context and patterns
- [Source: openraft docs — docs.rs/openraft/latest/openraft/docs/getting_started/] — openraft 0.9 API guide

## Dev Agent Record

### Agent Model Used

### Debug Log References

### Completion Notes List

### File List
