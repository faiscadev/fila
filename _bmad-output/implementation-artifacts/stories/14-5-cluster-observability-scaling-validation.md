# Story 14.5: Cluster Observability & Scaling Validation

Status: done

## Story

As an operator,
I want to view aggregated stats across all cluster nodes and verify linear scaling,
so that I can monitor the cluster as a single system and trust that adding nodes increases capacity.

## Acceptance Criteria

1. **Given** a multi-node cluster, **when** an operator calls GetStats for a queue, **then** the response includes additional cluster fields: `leader_node_id` (which node is the Raft leader for this queue) and `replication_status` (number of replicas in the queue's Raft group).

2. **Given** a multi-node cluster, **when** an operator calls ListQueues, **then** each queue entry includes `leader_node_id` and the response header includes `cluster_node_count` (total nodes in cluster).

3. **Given** OTel metrics are enabled, **when** metrics are recorded in cluster mode, **then** all existing metrics include a `node_id` label so operators can distinguish per-node contributions in Grafana/Prometheus.

4. **Given** a multi-node cluster, **when** an operator runs `fila queue inspect <name>`, **then** the CLI displays the queue's Raft leader node ID and replication count alongside existing stats.

5. **Given** a 3-node cluster with a queue, **when** integration tests call GetStats and ListQueues, **then** the tests verify that leader_node_id is a valid cluster member and replication count matches the group size.

6. **Given** a Fila node is running in single-node mode, **when** GetStats/ListQueues are called, **then** `leader_node_id` is 0 (not clustered) and `replication_status` is 0, and behavior is otherwise unchanged.

7. **Given** the Epic 12 benchmark suite, **when** a scaling test methodology document is written, **then** it describes how to reproduce a multi-node scaling benchmark using the existing `fila-bench` harness (without requiring actual multi-node benchmark execution in CI).

## Tasks / Subtasks

- [x] Task 1: Add cluster fields to proto messages (AC: 1, 2, 6)
  - [x] 1.1 Add `leader_node_id` (uint64) and `replication_count` (uint32) to `GetStatsResponse` in admin.proto
  - [x] 1.2 Add `leader_node_id` (uint64) to `QueueInfo` and `cluster_node_count` (uint32) to `ListQueuesResponse` in admin.proto
  - [x] 1.3 Update `QueueStats` and `QueueSummary` structs in fila-core to carry these fields
  - [x] 1.4 Update admin_handlers.rs to populate from ClusterHandle (or default to 0 in single-node)

- [x] Task 2: Add node_id label to OTel metrics (AC: 3)
  - [x] 2.1 Accept optional `node_id` in `Metrics::new()` / `from_meter()` (or store as a field)
  - [x] 2.2 Include `node_id` KeyValue in all metric record calls when set (cluster mode)
  - [x] 2.3 Wire node_id from BrokerConfig/ClusterConfig into Metrics construction
  - [x] 2.4 Test: verify node_id label appears on metrics when configured

- [x] Task 3: Wire cluster info into GetStats and ListQueues handlers (AC: 1, 2)
  - [x] 3.1 Pass ClusterHandle (or None) to the Scheduler so it can query leader/replication info
  - [x] 3.2 In handle_get_stats: query ClusterHandle::is_queue_leader and multi_raft.get_raft for group size
  - [x] 3.3 In handle_list_queues: include leader_node_id per queue and cluster_node_count
  - [x] 3.4 Update admin_service.rs to map new fields to proto response

- [x] Task 4: Update CLI display (AC: 4)
  - [x] 4.1 Update `fila queue inspect` to show "Raft leader: node X" and "Replicas: N" when values are non-zero
  - [x] 4.2 Update `fila queue list` to show leader_node_id column when cluster_node_count > 0

- [x] Task 5: Integration tests (AC: 5, 6)
  - [x] 5.1 Test: 3-node cluster GetStats returns valid leader_node_id and correct replication_count
  - [x] 5.2 Test: 3-node cluster ListQueues returns leader_node_id per queue and cluster_node_count=3
  - [x] 5.3 Test: single-node mode returns leader_node_id=0 and replication_count=0 (existing tests)

- [x] Task 6: Scaling benchmark methodology (AC: 7)
  - [x] 6.1 Write docs/cluster-scaling.md documenting how to run a multi-node scaling benchmark
  - [x] 6.2 Include example fila.toml configs for a 3-node local cluster
  - [x] 6.3 Reference the Epic 12 fila-bench harness and explain how to measure throughput scaling

## Dev Notes

### Architecture

Story 14.5 adds observability on top of the clustering infrastructure from 14.1–14.4. The key changes are:
1. **Proto extensions** — adding cluster-aware fields to existing admin RPCs (not new RPCs)
2. **OTel node_id label** — attaching `node_id` to all metrics for per-node dashboarding
3. **Scheduler ↔ Cluster info flow** — the scheduler needs read-only access to cluster state for stats queries

### Key Design Decisions

**Scheduler accesses ClusterHandle for stats:** The scheduler is single-threaded and runs on its own thread. It can't directly call async ClusterHandle methods. Two options:
- Option A: Pre-compute cluster info on the broker side (in the async admin_service handler) and pass it with the stats response. The scheduler returns raw QueueStats, and the admin_service enriches it with cluster info before returning to the client.
- Option B: Store cluster state snapshot in the scheduler.

**Option A is simpler** — the admin_service already has access to ClusterHandle. GetStats/ListQueues are admin operations, not hot-path. Adding a few async calls in the handler is fine.

**node_id label injection:** Rather than modifying every `record_*` method to accept node_id, store the node_id as a field on Metrics and include it as a default label in every recording call. When node_id is 0 (single-node), omit the label to avoid noise.

**Scaling benchmark as documentation only:** Actually running a multi-node benchmark in CI is impractical (need 3 server processes, port coordination, etc.). Instead, document the methodology so operators can reproduce it. The existing fila-bench harness already handles single-node benchmarks — the doc explains how to point it at a cluster.

### Existing Infrastructure

- `GetStats` handler: `admin_handlers.rs:handle_get_stats()` → `QueueStats` → `admin_service.rs` → proto
- `ListQueues` handler: `admin_handlers.rs:handle_list_queues()` → `Vec<QueueSummary>` → `admin_service.rs` → proto
- `Metrics` struct: `metrics.rs` — 17 instruments with `queue_id` label. Need to add optional `node_id`.
- `ClusterHandle`: has `is_queue_leader()`, `multi_raft.get_raft()`, `multi_raft.list_groups()`, `meta_members()`
- CLI: `fila-cli/src/main.rs` — `queue inspect` and `queue list` subcommands
- Proto: `admin.proto` — `GetStatsResponse`, `QueueInfo`, `ListQueuesResponse`
- Benchmark: `fila-bench` crate, `docs/benchmarks.md`

### Key Files to Modify

| File | Change |
|------|--------|
| `crates/fila-proto/proto/fila/v1/admin.proto` | Add cluster fields to GetStatsResponse, QueueInfo, ListQueuesResponse |
| `crates/fila-core/src/broker/stats.rs` | Add leader_node_id, replication_count to QueueStats |
| `crates/fila-core/src/broker/command.rs` | Add leader_node_id to QueueSummary |
| `crates/fila-core/src/broker/metrics.rs` | Add optional node_id label to all recordings |
| `crates/fila-server/src/admin_service.rs` | Enrich GetStats/ListQueues with cluster info from ClusterHandle |
| `crates/fila-cli/src/main.rs` | Display cluster fields in queue inspect/list |
| `crates/fila-core/src/cluster/tests.rs` | Integration tests for cluster stats |
| `docs/cluster-scaling.md` | Scaling benchmark methodology |

### References

- [Source: _bmad-output/planning-artifacts/epics.md#Story 14.5]
- [Source: crates/fila-core/src/broker/stats.rs — QueueStats struct]
- [Source: crates/fila-core/src/broker/metrics.rs — Metrics struct and OTel instruments]
- [Source: crates/fila-server/src/admin_service.rs — GetStats/ListQueues handlers]
- [Source: crates/fila-cli/src/main.rs — queue inspect/list CLI commands]
- [Source: crates/fila-proto/proto/fila/v1/admin.proto — proto definitions]

## Dev Agent Record

### Agent Model Used

Claude Opus 4.6

### Debug Log References

### Completion Notes List

### Change Log

- Added cluster fields (leader_node_id, replication_count, cluster_node_count) to admin.proto
- Added leader_node_id/replication_count to QueueStats and QueueSummary structs
- Updated admin_service.rs to enrich stats with cluster info from Raft metrics
- Added node_id label to OTel Metrics struct (optional, cluster-mode only)
- Updated Scheduler::new to accept cluster_node_id parameter
- Updated CLI queue inspect/list to show cluster fields when non-zero
- Added 2 cluster stats integration tests, 2 single-node tests, 2 metric label tests
- Created docs/cluster-scaling.md with 3-node benchmark methodology

### File List

- crates/fila-proto/proto/fila/v1/admin.proto
- proto/fila/v1/admin.proto
- crates/fila-core/src/broker/stats.rs
- crates/fila-core/src/broker/command.rs
- crates/fila-core/src/broker/metrics.rs
- crates/fila-core/src/broker/mod.rs
- crates/fila-core/src/broker/scheduler/mod.rs
- crates/fila-core/src/broker/scheduler/admin_handlers.rs
- crates/fila-core/src/broker/scheduler/tests/common.rs
- crates/fila-core/src/broker/scheduler/tests/fairness.rs
- crates/fila-core/src/broker/scheduler/tests/ack_nack.rs
- crates/fila-core/src/cluster/tests.rs
- crates/fila-server/src/admin_service.rs
- crates/fila-cli/src/main.rs
- docs/cluster-scaling.md
