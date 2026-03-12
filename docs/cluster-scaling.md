# Cluster Scaling Benchmark Methodology

This document describes how to measure Fila's horizontal scaling characteristics
using the `fila-bench` harness against a multi-node cluster.

## Prerequisites

- Fila server binary (3 copies, or 1 binary run 3 times with different configs)
- `fila-bench` binary (from `crates/fila-bench`)
- `fila` CLI binary (from `crates/fila-cli`)

## Setting Up a 3-Node Local Cluster

Create three configuration files, one per node.

### node1.toml

```toml
[server]
listen_addr = "127.0.0.1:5555"

[cluster]
enabled = true
node_id = 1
bind_addr = "127.0.0.1:5556"
bootstrap = true
peers = []
```

### node2.toml

```toml
[server]
listen_addr = "127.0.0.1:5565"

[cluster]
enabled = true
node_id = 2
bind_addr = "127.0.0.1:5566"
bootstrap = false
peers = ["127.0.0.1:5556"]
```

### node3.toml

```toml
[server]
listen_addr = "127.0.0.1:5575"

[cluster]
enabled = true
node_id = 3
bind_addr = "127.0.0.1:5576"
bootstrap = false
peers = ["127.0.0.1:5556"]
```

### Starting the Cluster

```bash
# Terminal 1
fila-server --config node1.toml

# Terminal 2
fila-server --config node2.toml

# Terminal 3
fila-server --config node3.toml
```

Verify the cluster is healthy:

```bash
fila --addr 127.0.0.1:5555 queue list
# Should show "Cluster nodes: 3" at the bottom
```

## Running Scaling Benchmarks

### Step 1: Baseline (Single Node)

Run `fila-bench` against a single standalone node to establish baseline throughput:

```bash
fila-bench --addr 127.0.0.1:5555 --category enqueue_throughput
fila-bench --addr 127.0.0.1:5555 --category consume_throughput
fila-bench --addr 127.0.0.1:5555 --category enqueue_consume_mixed
```

Record the messages/second for each category.

### Step 2: Cluster Throughput

Create queues distributed across the cluster, then run the same benchmarks:

```bash
# Create test queues (they'll be replicated across all 3 nodes)
fila --addr 127.0.0.1:5555 queue create bench-q1
fila --addr 127.0.0.1:5555 queue create bench-q2
fila --addr 127.0.0.1:5555 queue create bench-q3

# Check leadership distribution
fila --addr 127.0.0.1:5555 queue list
```

Run benchmarks targeting different nodes to exercise the full cluster:

```bash
# Enqueue through node 1 (may forward to queue leaders on other nodes)
fila-bench --addr 127.0.0.1:5555 --category enqueue_throughput

# Enqueue through node 2
fila-bench --addr 127.0.0.1:5565 --category enqueue_throughput

# Mixed workload across all nodes (run in parallel)
fila-bench --addr 127.0.0.1:5555 --category enqueue_consume_mixed &
fila-bench --addr 127.0.0.1:5565 --category enqueue_consume_mixed &
fila-bench --addr 127.0.0.1:5575 --category enqueue_consume_mixed &
wait
```

### Step 3: Measure Scaling Factor

Compare cluster throughput to single-node baseline:

```
scaling_factor = cluster_total_msgs_per_sec / single_node_msgs_per_sec
```

For a well-distributed workload across 3 queues on 3 nodes, the ideal
scaling factor is 3.0x. In practice, expect 2.0x-2.5x due to Raft
consensus overhead (log replication, leader forwarding).

## What to Measure

| Metric | How | Tool |
|--------|-----|------|
| Enqueue throughput (msg/s) | `fila-bench --category enqueue_throughput` | fila-bench |
| Consume throughput (msg/s) | `fila-bench --category consume_throughput` | fila-bench |
| P99 enqueue latency | `fila-bench --category enqueue_latency` | fila-bench |
| Raft consensus overhead | Compare single-node vs cluster latency | fila-bench |
| Leadership distribution | `fila queue list` LEADER column | fila CLI |
| Failover time | Kill leader, measure time to new leader election | manual |

## Interpreting Results

- **Linear scaling** means the scaling factor approaches N (number of nodes).
  Fila achieves this when queues are distributed across different leaders.
- **Sub-linear scaling** is expected for single-queue workloads since all
  writes go through one Raft leader regardless of cluster size.
- **Raft overhead** adds ~1-3ms per write (log replication to majority).
  This is the cost of durability and fault tolerance.
- **Forwarding overhead** adds latency when a client connects to a non-leader
  node. The request is transparently forwarded to the leader via gRPC.

## Notes

- The `fila-bench` harness runs single-node benchmarks by default. To benchmark
  a cluster, point it at any cluster node's client address.
- Queue creation in cluster mode goes through the meta Raft group. All nodes
  in the cluster will create the queue locally.
- Consumer streams are served by the queue's Raft leader. If the leader
  changes (failover), consumers automatically reconnect to the new leader.
