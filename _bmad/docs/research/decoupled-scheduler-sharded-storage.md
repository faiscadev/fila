# Technical Research: Decoupled Scheduler + Sharded Storage Architecture

**Date:** 2026-03-10
**Topic:** Horizontal scaling architecture for Fila with preserved global fairness

---

## Executive Summary

The proposed three-layer architecture (sharded storage, centralized scheduler, stateless delivery) is **feasible and well-precedented**. The closest production analogues are **Pulsar+BookKeeper** (storage/compute separation) and **Meta's FOQS** (centralized priority scheduling over sharded storage). The centralized scheduler is not the bottleneck — a single Raft-replicated metadata scheduler can sustain **30,000–50,000 scheduling decisions/sec**, which far exceeds what Fila would need. The real engineering challenges are: (1) lease state durability across scheduler failover, and (2) the latency cost of multi-hop delivery (mitigable with pipelining). FIFO ordering is an opt-in feature (per-queue) that only guarantees order within a fairness key, which simplifies the distributed model — shard by fairness key gives trivial FIFO with no cross-shard coordination.

**Recommended first step:** Shard by queue (virtual queue overlay). Each queue gets one scheduler instance. This preserves perfect per-queue fairness with zero algorithmic changes while enabling horizontal scaling across queues. For extremely high-throughput single queues, a hierarchical two-level approach can be added later.

---

## 1. Feasibility: Production Precedents

### Systems with centralized scheduling brain + distributed storage

| System | Scheduling Brain | Storage Layer | Scale |
|--------|-----------------|---------------|-------|
| **Google Borg** | Single Borgmaster (Paxos 5x) | Local machine disks | 10K machines/cell, 10K+ tasks/min |
| **Apache Pulsar** | Per-bundle broker ownership | BookKeeper (distributed WAL) | 1M+ msgs/sec |
| **Meta FOQS** | Centralized priority scheduler | Sharded MySQL | Billions of jobs/month |
| **Temporal/Cadence** | History Service (sharded) | Cassandra/MySQL | 12B workflow executions/month (Uber) |
| **Celery Beat** | Single process | Redis/RabbitMQ | Simple periodic scheduling |

**Key finding:** Google's Borg paper (published *after* the Omega multi-scheduler paper) stated: "Borg's monolithic scheduler hasn't been a problem in terms of scalability." Omega's shared-state design was intellectually influential but never deployed in production. **A centralized scheduler is not the bottleneck** when it only handles metadata.

**Most relevant precedent — Meta's FOQS:**
- Sharded storage across MySQL instances
- Centralized priority scheduler with a **Prefetch Buffer** that does k-way merge across shard indexes
- Demand-aware replenishment: tracks per-topic dequeue rate and prefetches proportionally
- Items marked "delivered" in the database during prefetch to prevent double-delivery
- This is essentially Fila's proposed architecture with a priority queue instead of DRR

---

## 2. Scheduler Throughput Ceiling

### Raft throughput for metadata-only operations

| System | Write Throughput | Read Throughput | Failover Time | Key Constraint |
|--------|-----------------|-----------------|---------------|----------------|
| **etcd (3-node)** | 30,000–40,000 ops/sec | Higher (serializable reads skip consensus) | 1.1–1.7s default | Disk fsync latency |
| **TiKV PD** | Bounded by embedded etcd | Heartbeat processing | ~5s (unplanned) | Region heartbeat volume |
| **CockroachDB (per range)** | ~2,500 QPS before auto-split | Leaseholder-served | 6–9s (lease duration) | Single-range hot key |
| **Raw Raft (no app overhead)** | ~100,000+ ops/sec | N/A | Election timeout | Network RTT + disk |

**Analysis:** A single Raft group doing scheduling decisions (small metadata: fairness_key → count, deficit, msg_location) can sustain **30,000–50,000 writes/sec** on NVMe SSDs. Fila's scheduling decisions are lighter than etcd key-value operations (no storage engine overhead needed if state is in-memory with Raft-replicated WAL).

**Can it handle 1M+ decisions/sec?** Not with a single Raft group. But 1M decisions/sec would mean delivering 1M messages/sec, which is already Pulsar-scale throughput. At that scale, the shard-by-queue approach means the 30K–50K ceiling applies *per queue*, and multiple queues run on separate scheduler instances. For a single queue needing >50K msgs/sec, batch scheduling (plan 100 messages per Raft write) brings the effective ceiling to **3M–5M messages/sec per queue**.

---

## 3. Latency Impact

### Per-hop latency in datacenter networks

| Scenario | p50 RTT | p99 RTT |
|----------|---------|---------|
| Localhost (no network) | 143 µs | 387 µs |
| Same AZ | 360 µs | 750 µs |
| Cross AZ, same region | 420–600 µs | 826 µs |
| Cross AZ (AWS worst case) | 2,420 µs | — |

### Fila's 4-hop path: consumer → delivery → scheduler → storage → consumer

- **Serial 4 hops (same AZ):** ~1,440 µs (1.4 ms) at p50, ~3,000 µs (3 ms) at p99
- **Cross AZ:** ~1,800–2,400 µs (1.8–2.4 ms) at p50

### Comparison with existing systems (durable mode)

| System | Architecture | p50 E2E | p99 E2E | Notes |
|--------|-------------|---------|---------|-------|
| **Kafka (acks=all)** | Single-node read | 2–5 ms | 5–15 ms | 0 extra hops for reads |
| **Pulsar (2-replica)** | Broker + BookKeeper (1 hop) | ~0.3 ms | 20–35 ms | High p99 tail from catch-up reads |
| **RabbitMQ** | Single node | 5–20 ms | — | Degrades above 30 MB/s |
| **NATS JetStream** | Single node | 1–5 ms | 15–40 ms | Raft-replicated |

### Mitigation: pipelining eliminates the scheduler hop

The 4-hop latency exists only for the **first message**. With pipelining:

1. Scheduler continuously pushes delivery plans (message IDs + storage locations) to delivery nodes
2. While delivering batch N, delivery node prefetches storage data for batch N+1
3. Steady-state delivery latency = **1 hop** (storage fetch only, ~360 µs)

**Recommended approach: push delivery plans with lease semantics**
- Plans have a TTL — if not consumed within N seconds, messages return to the schedulable pool
- Delivery node sends consumption rate metrics back to scheduler for demand-aware plan sizing
- On delivery node failure, plans expire naturally — no explicit invalidation needed

### Batch size vs. fairness trade-off

Batching introduces a "staleness window" where new messages from underserved keys don't get priority:

| Batch Size | Delivery Time/msg | Staleness Window |
|-----------|-------------------|------------------|
| 10 | 0.1 ms | 1 ms |
| 100 | 0.1 ms | 10 ms |
| 1000 | 0.1 ms | 100 ms |

**Recommendation:** Start with batch size 50–100, time-bounded to 5–10 ms. Adapt downward when fairness variance is high, upward when load is uniform. Consider plan amendments for fairness-critical workloads (scheduler pushes incremental updates to active plans).

**Target latency budget:** p50 of 2–5 ms, p99 of 10–30 ms end-to-end. Competitive with Kafka (acks=all).

---

## 4. Storage Shard Rebalancing

### How existing systems handle data migration

| System | Unit of Migration | Can Scale Down? | Data Movement on Scale-Out | Downtime |
|--------|-------------------|-----------------|---------------------------|----------|
| **Kafka** | Partition (full log) | No | Full partition copy | None |
| **Redis Cluster** | Hash slot (key-by-key) | Yes | Key-by-key per slot | None (ASK redirects) |
| **CockroachDB** | Range (~512 MiB) | Yes (merge) | Raft snapshot + log | None |
| **Vitess** | Shard (table partition) | Yes | VReplication stream | Seconds (write cutover) |
| **BookKeeper** | Ledger (immutable) | N/A | **None** (new ledgers use new nodes) | None |

### Key insight: BookKeeper avoids rebalancing entirely

BookKeeper's ledgers are immutable and finite. New ledgers automatically use new bookies. Adding capacity requires **zero data movement**. This is the strongest argument for Pulsar-style architecture in Fila's storage layer.

**If Fila adopts immutable storage segments (like BookKeeper ledgers):**
- Adding storage nodes = immediate capacity increase with zero rebalancing
- Old segments stay on old nodes until they expire or are compacted
- Message bodies are immutable after enqueue (only metadata like attempt_count changes)

**If Fila uses mutable storage (like RocksDB across nodes):**
- CockroachDB's learner-replica approach is safest: add non-voting learner → send snapshot → replay log → promote to voter → remove old replica
- All robust systems share the principle: **source remains authoritative until explicit cutover**
- Idempotent replay is essential (offset-based, entry-ID-based tracking)

### Recommended protocol for Fila

1. Adopt BookKeeper-style immutable segments for message bodies
2. Metadata (attempt_count, lease state) lives on the scheduler, not storage
3. Adding storage nodes = new segments use new nodes, no migration needed
4. Removing storage nodes = drain (stop writing new segments) + wait for existing segments to be consumed + garbage collect

---

## 5. Failure Modes

### 5.1 Scheduler leader failure mid-delivery-plan

**The problem:** Consumer has been told "here is message X" but scheduler dies before tracking the lease.

**How Raft handles it:** Uncommitted entries are lost. The new leader has the committed state only. The client (delivery node) gets a timeout and must retry.

**Mitigation for Fila — two options:**

| Approach | Mechanism | Trade-off |
|----------|-----------|-----------|
| **Lease in replicated storage** | Delivery creates a lease record in storage before telling consumer. Survives scheduler failure. | Extra write on the critical path |
| **Consumer-side idempotency + lease timeout** | Scheduler dies → lease unknown to successor → message re-delivered after timeout. Consumer deduplicates. | Simpler, but at-least-once not exactly-once |

**Recommendation:** Store lease records in the Raft-replicated scheduler state (not storage nodes). The lease is committed via Raft before the message is sent to the consumer. On scheduler failover, the new leader reconstructs lease state from the Raft log.

### 5.2 Storage node failure with unreplicated messages

**Critical learning from NATS JetStream (Jepsen, December 2025):**
- NATS's default `sync_interval` is 2 minutes — no fsync before ack
- Single-bit error in one of five nodes caused loss of **49.7% of acknowledged writes**
- Process crashes alone caused total stream loss in versions 2.10.20–2.10.22

**Prevention:** Ack-after-replicate is non-negotiable. BookKeeper's quorum-write model (ack after AQ bookies persist) is the gold standard. Never acknowledge a message to the producer before it is replicated to a quorum.

### 5.3 Event stream lag (storage → scheduler)

**The problem:** Scheduler's view of pending messages is stale. Might miss new messages or try to schedule already-consumed ones.

**Mitigation:**
- **Lease-based protection:** Storage enforces leases. Even with stale scheduler view, double-delivery is prevented by storage rejecting duplicate lease attempts
- **Checkpointing:** Scheduler persists its stream position via Raft. On restart, replays from checkpoint
- **Bounded lag monitoring:** Alert when lag exceeds threshold

### 5.4 Split brain (two schedulers both think they're leader)

**Prevention — fencing tokens are non-negotiable:**
- Each leader election increments a monotonic epoch number
- Every delivery plan and lease operation carries the epoch
- Storage nodes persist the highest epoch seen and **reject operations from lower epochs**
- Even if an old leader resumes after a GC pause, its stale epoch is rejected

This is described by Martin Kleppmann as the critical safety mechanism for distributed locking: "If the lock service makes no effort to prevent two clients from holding the lock at the same time, the storage layer must be able to fence off stale clients."

### 5.5 Network partition between storage and scheduler

**Recommended approach:**
- Storage always accepts writes (up to disk capacity) during partition
- Scheduler enters degraded mode — only schedules from reachable storage nodes
- On partition heal, scheduler replays from last checkpoint
- Fairness may be temporarily violated during catchup (burst of messages from the partitioned shard), but correctness is preserved if leases and epochs are enforced

---

## 6. Alternative Architectures

### Comparison matrix

| Architecture | Perfect Fairness? | Complexity | Availability | Best For |
|---|---|---|---|---|
| **Centralized scheduler (proposed)** | Yes | Medium | Leader election latency | The general case |
| **Virtual queue overlay (shard by queue)** | Yes (per-queue) | Low-Medium | Per-shard failover | Near-term scaling, practical first step |
| **Hierarchical two-level** | Bounded error | Medium | High (coordinator is soft dep) | Very large scale |
| **Deterministic (Calvin-style)** | Yes | High | Limited by log quorum | Batch workloads |
| **Gossip/CRDT** | Approximate only | Medium-High | Very high | Soft fairness requirements |

### Virtual Queue Overlay (shard by queue) — RECOMMENDED FIRST STEP

Shard by queue, not by fairness key. Each queue gets one scheduler instance.

- **Preserves perfect per-queue fairness** with zero algorithmic changes
- Each shard is essentially the existing Fila scheduler running for a subset of queues
- Adding queues = adding scheduler shards (linear scaling)
- Consistent hashing ring maps queue → scheduler instance
- Failure of one shard affects only that shard's queues

**Limitation:** Single-queue throughput is bounded by one scheduler instance. For extremely hot queues, escalate to the hierarchical approach.

### Hierarchical Two-Level — FOR EXTREME SCALE

- **Level 1 (shard-local):** Each storage shard has a co-located fair scheduler running DRR for its local fairness keys
- **Level 2 (global coordinator):** Tracks aggregate deficit across shards, adjusts per-shard weights periodically (e.g., every 100ms)
- Cross-shard fairness has bounded error proportional to adjustment interval
- Coordinator failure = shards continue with last known weights (degrades fairness, not availability)

### Deterministic (Calvin-style) — THEORETICALLY ELEGANT, PRACTICALLY TRICKY

All scheduler replicas read the same replicated log and compute identical schedules independently. Perfect fairness, but:
- Consume/lease operations are stateful — replicas must agree on which consumer gets which message
- Every consume request would need to go through the log, adding latency
- Best suited for batch workloads, not interactive consume streams

### Gossip/CRDT — NOT SUITABLE FOR FILA

Multiple schedulers share fairness state via CRDTs. Converges eventually, but:
- Temporary fairness violations proportional to gossip propagation time
- Fila's DRR precision matters (Epic 4 solved O(n²) delivery scan for fairness correctness)
- "Approximately fair" is not Fila's value proposition

---

## 7. Impact on Fila's Existing Features

### Feature mapping to distributed model

| Feature | Difficulty | Key Insight |
|---------|-----------|-------------|
| **DRR fairness** | Easy | Pure metadata algorithm. Never touches message bodies. Maps directly to centralized scheduler. |
| **Throttling** | Easy | Pure in-memory scheduler state. No storage interaction needed. |
| **Lua hooks** | Medium | Neither `on_enqueue` nor `on_failure` needs message bodies (only headers, payload_size, attempt_count). `fila.get()` config store must be scheduler-local. |
| **DLQ** | Medium | Cross-shard atomic move. Solvable by co-locating DLQ with parent queue on same storage shard. |
| **Visibility timeouts / leases** | Medium | Lease tracking naturally on scheduler. Expiry reclamation needs storage round-trip (infrequent, acceptable). |
| **Consume streaming** | Medium | Extra scheduler-to-storage hop per delivery. Mitigable with prefetch/pipelining. |
| **Redrive** | Medium | Same cross-shard pattern as DLQ. Admin-only operation, latency acceptable. |
| **FIFO ordering** | Easy–Medium | **FIFO is opt-in per queue** and only guarantees order within a fairness key. See detailed analysis below. |

### FIFO ordering model

FIFO is an opt-in feature with an understood performance trade-off. It guarantees that messages **within the same fairness key** are delivered in enqueue order. It does NOT guarantee global ordering across fairness keys (that would contradict fair scheduling).

This simplifies the distributed model significantly:

| Queue Mode | Storage Sharding | FIFO Guarantee | Recovery | Performance |
|------------|-----------------|----------------|----------|-------------|
| **Default (no FIFO)** | By message ID (load-balanced across shards) | None | Simple — each shard recovers independently, no ordering constraints | Best throughput, even load distribution |
| **FIFO (opt-in)** | By fairness key (all messages for a key on one shard) | Within fairness key | Simple — each key's messages are on one shard, local ordering is sufficient | Hot-shard risk for high-volume keys |

**Why this works:** When FIFO is enabled, routing all messages for a fairness key to the same storage shard means FIFO is maintained by that shard's local ordering (enqueue timestamp in the storage key). No cross-shard merge-sort is needed, even during recovery. The scheduler's pending index still maintains per-key order, but recovery only needs to scan one shard per fairness key.

**The trade-off users accept:** FIFO queues may have uneven storage distribution if some fairness keys are much hotter than others. A single high-volume fairness key concentrates all its messages on one shard. This is a capacity planning concern, not a correctness concern.

### Cross-cutting observations

1. **The scheduler's pending index is the linchpin.** Currently holds `(msg_key, msg_id, throttle_keys)` per pending message. In distributed model, add `shard_id`. Must fit in scheduler memory. For very large queues, this could be a memory constraint.

2. **Recovery complexity is manageable.** For non-FIFO queues: each shard recovers independently, no ordering constraints. For FIFO queues: each fairness key is on one shard, so recovery scans one shard per key with local ordering — no cross-shard merge-sort needed.

3. **Write path is simpler than read path.** Enqueue flows naturally through scheduler (run Lua, assign metadata, pick shard, write). Delivery requires a read from a specific shard after a scheduling decision.

4. **Most "medium" features share the same root issue:** cross-shard operations that are currently atomic `write_batch` calls. Solution pattern: co-locate related data, accept eventual consistency where appropriate, or use two-phase writes with idempotency.

---

## 8. Relevant Academic Papers

| Paper | Venue | Key Contribution |
|-------|-------|-----------------|
| **Pisces** | OSDI 2012 | Multi-tenant storage fairness: distributed DWRR with per-worker sub-allocated tokens. 0.99 min-max ratio, <3% overhead. Most directly relevant to Fila. |
| **2DFQ** | SIGCOMM 2016 | Two-dimensional fair queuing for concurrent thread pools. Reduces burstiness by 1–2 orders of magnitude. |
| **DWRR** | PPoPP 2009 | Distributed Weighted Round-Robin: per-CPU round slicing + cross-CPU round balancing. Constant error bounds. Implemented in Linux kernel. |
| **Calvin** | SIGMOD 2012 | Deterministic transaction scheduling via replicated log. Basis for the deterministic scheduling alternative. |
| **DRF** | NSDI 2011 | Dominant Resource Fairness: extends max-min fairness to multiple resource types. Relevant if Fila ever considers multi-resource scheduling. |

---

## 9. Recommended Architecture

### Deployment model: CockroachDB-style single binary

One `fila` binary. Every node runs the same code. The cluster self-organizes.

```
┌──────────────────────────────────────────────────────────────┐
│                     Client SDK / Proxy                        │
│          (discovers cluster, routes by queue → leader)        │
└──────┬──────────────────┬──────────────────┬─────────────────┘
       │                  │                  │
 ┌─────▼─────┐      ┌─────▼─────┐      ┌─────▼─────┐
 │  fila (1)  │      │  fila (2)  │      │  fila (3)  │
 │            │      │            │      │            │
 │ Leader for │◄────►│ Leader for │◄────►│ Leader for │
 │ queues A,B │ Raft │ queues C,D │ Raft │ queues E,F │
 │ Follower   │      │ Follower   │      │ Follower   │
 │ for C,D,E,F│      │ for A,B,E,F│      │ for A,B,C,D│
 │            │      │            │      │            │
 │ Storage    │      │ Storage    │      │ Storage    │
 │ Scheduler  │      │ Scheduler  │      │ Scheduler  │
 │ Gateway    │      │ Gateway    │      │ Gateway    │
 └────────────┘      └────────────┘      └────────────┘
```

**How it works:**
- You run N copies of `fila` (minimum 3 for HA). They discover each other and form a cluster
- Each queue is a Raft group. All N nodes participate as replicas (or a subset for large clusters)
- The Raft leader for a queue handles scheduling, storage writes, and consumer delivery for that queue
- Followers replicate everything via Raft: message data, DRR state, leases, pending index, config
- Queue-to-leader assignment is automatic, balanced across nodes by the cluster
- Any node can serve any client — if a client connects to a follower for a queue, the node proxies or redirects to the leader

**Scaling:**
- Add a node → it joins the cluster, becomes a Raft follower for existing queues, the cluster rebalances leadership to it. The new node catches up via **Raft snapshots** (not a custom migration protocol)
- Remove a node → its Raft leadership transfers to other nodes in 1–2 seconds. Followers already have full state. **Zero data migration needed**
- More nodes = more queues can run in parallel = higher aggregate throughput

**Day 1 experience:**
- Single node: `fila` runs exactly like today. No cluster, no Raft, no overhead. Same binary.
- Three nodes: `fila --join node1,node2,node3`. Full HA, automatic failover, zero config beyond the join addresses.

This is the same model as CockroachDB (every node runs ranges, Raft handles replication/failover) and new Kafka KRaft (controller + broker in one binary). It eliminates the operational complexity of separate storage/scheduler/delivery processes.

### Phase 1: Shard by queue

Each queue is a Raft group. One leader per queue across the cluster.

- DRR, Lua hooks, throttling, leases — all unchanged, just running per-queue on the leader
- Fencing tokens (Raft term) on every operation
- Producers and consumers connect to any node; the cluster routes to the queue's leader

### Phase 2 (if needed): Hierarchical for hot queues

For queues exceeding single-node throughput (a future concern, not a day-1 deliverable):
- Split the queue's fairness keys across multiple Raft groups (each on a different leader)
- Add a lightweight coordinator that adjusts per-group DRR weights
- Bounded fairness error proportional to coordinator adjustment interval

### Phase 2 viability constraints

Phase 2 is not a day-1 deliverable, but the phase 1 design **must not close the door** to it. The following design constraints ensure phase 2 remains viable without premature engineering:

#### 1. Queue → Raft group mapping must go through an indirection layer

**Do:** Introduce a routing/placement table that maps `(queue, fairness_key)` → Raft group. In phase 1, the implementation is trivial: every fairness key in a queue maps to the same group (1:1). The indirection exists in the code path, it just always returns the same answer.

**Don't:** Hardcode queue identity = Raft group identity throughout the codebase. If every call site assumes `queue_id == raft_group_id`, splitting a queue across multiple groups in phase 2 requires finding and changing every assumption.

**What phase 2 changes:** The routing table becomes 1:N — fairness keys are partitioned across multiple Raft groups (e.g., by consistent hashing of the fairness key). The SDK asks "where does fairness key X in queue Q go?" instead of "where does queue Q live?"

#### 2. DRR must be scoped to a key-set, not welded to a queue

**Do:** The scheduler runs DRR over "the fairness keys I'm responsible for." In phase 1, that happens to be all keys in the queue. The DRR engine takes a set of keys as input — it doesn't reach up and grab them from the queue definition.

**Don't:** Have DRR intrinsically tied to "all fairness keys that exist in this queue" with no way to narrow the scope.

**What phase 2 changes:** Each Raft group runs DRR over its assigned subset of fairness keys. The DRR algorithm is identical — only the input set changes.

#### 3. Define the coordinator interface early (even if phase 1 ignores it)

**Do:** Each Raft group periodically emits aggregate scheduling stats: messages scheduled per fairness key, current deficit state, throughput. In phase 1, nothing consumes these stats (or they're just exposed as metrics for observability — useful regardless).

**Don't:** Make internal scheduling state completely opaque with no way to observe or influence it from outside the Raft group.

**What phase 2 changes:** A coordinator process consumes the stats from all Raft groups for a queue and emits weight adjustments. Each group applies the weight adjustments to its local DRR. This is a new consumer of an existing data stream, not a retrofit.

#### 4. Message routing must be by fairness key, not just by queue

**Do:** The enqueue path routes as `(queue, fairness_key) → Raft group`. In phase 1, the fairness key is ignored (all go to the same group). But the fairness key is present in the routing decision.

**Don't:** Route enqueue requests by queue alone with no awareness of the fairness key. Phase 2 needs to split by fairness key — if the key isn't available at routing time, it requires a protocol change.

**What phase 2 changes:** The router actually uses the fairness key to pick the target Raft group (e.g., `consistent_hash(fairness_key) % num_groups`).

#### Summary: what this costs in phase 1

These constraints add minimal overhead to phase 1:

| Constraint | Phase 1 Cost | Phase 2 Benefit |
|-----------|-------------|----------------|
| Routing indirection | One lookup table that always returns the same group | Split a queue without rewriting routing |
| DRR scoped to key-set | Pass key-set to DRR (happens to be "all keys") | Narrow scope per group without changing DRR |
| Emit scheduling stats | Expose as metrics (useful for observability anyway) | Coordinator consumes existing data stream |
| Route by (queue, fairness_key) | Ignore fairness_key in routing (it's in the request already) | Use it for partitioning without protocol change |

None of these are speculative abstractions or premature engineering. They're thin seams in the code that make phase 2 a matter of implementing new logic behind existing interfaces, rather than rearchitecting the core.

### Storage engine abstraction

RocksDB is a good choice today — battle-tested, embedded, excellent write performance for queue workloads. But it's an implementation detail, not an architectural commitment. The clustering work should ensure the storage engine is cleanly abstracted so it can be swapped in the future (for a purpose-built engine, a different LSM-tree, or something else entirely).

#### Role of the storage engine in the Raft model

In a Raft-replicated system, the storage engine's role changes fundamentally:

| Concern | Single-node Fila (today) | Clustered Fila (Raft) |
|---------|-------------------------|----------------------|
| **Durability** | RocksDB (WAL + fsync) | Raft log (replicated across nodes) |
| **Source of truth** | RocksDB on disk | Raft log (committed entries) |
| **Storage engine role** | Everything | State machine: applies committed Raft entries to local queryable state |
| **Crash recovery** | Replay RocksDB WAL | Replay Raft log (or receive snapshot from leader) |

RocksDB becomes a **local state machine backend** — it applies committed entries and serves reads. It no longer owns durability or recovery. This makes swapping it out easier: any engine that can apply key-value mutations and serve range scans works.

#### Abstraction requirements

The storage trait should describe **what Fila needs**, not what RocksDB offers:

| Fila needs | RocksDB concept (leak to avoid) |
|-----------|-------------------------------|
| Put message | `DB::put()` with column family |
| Get message by key | `DB::get()` with column family |
| Scan messages in range (for recovery, DLQ scan, redrive) | `Iterator` with column family prefix |
| Atomic batch of mutations (put + delete in one operation) | `WriteBatch` |
| Separate namespaces for messages, leases, config, etc. | Column families |

The trait should use Fila-domain terms (message store, lease store, config store) rather than RocksDB terms (column families, iterators, write batches). If the current `Storage` trait already leaks RocksDB internals (column family handles, raw iterators), cleaning this up is a natural part of the clustering work.

#### Future options this keeps open

- **Purpose-built queue engine** — LSM-trees are optimized for general key-value workloads. A queue workload (append-heavy, sequential reads, frequent deletes after ack) could benefit from a simpler log-structured design.
- **In-memory engine for tests** — a lightweight in-memory implementation of the storage trait makes unit testing the scheduler/Raft layer faster and more deterministic.
- **Different engines per deployment** — embedded RocksDB for single-node/small clusters, something else for large-scale deployments.

### High Availability

Since every node runs the same code and Raft replicates everything, HA is built into the model:

#### How Raft handles every failure scenario

Each queue is a Raft group (3 or 5 replicas across cluster nodes). Followers have **full replicated state** at all times: message data, DRR deficits, leases, pending index, scheduler metadata. There is no separate "storage" vs "scheduler" replication — it's one Raft log per queue that replicates everything.

| Raft Group Size | Tolerates N Failures | Availability Model |
|----------------|---------------------|-------------------|
| 3 nodes | 1 failure | Standard (same as Kafka, Pulsar, RabbitMQ quorum queues) |
| 5 nodes | 2 failures | High (for critical queues) |

**Node leaves (crash or decommission):**
- Followers already have full state — no data to migrate
- New leader elected in 1–2 seconds from an existing follower
- Consumers reconnect to the new leader (or the node they're connected to proxies to it)
- Producers are unaffected — any node accepts writes and forwards to the leader

**Node joins:**
- New node joins as a Raft learner (non-voting) for assigned queue groups
- Receives a Raft snapshot from the leader + replays subsequent log entries
- Once caught up, promoted to voter
- Cluster rebalances some queue leaderships to the new node to spread load

**Network partition:**
- Majority partition elects a new leader and continues serving
- Minority partition cannot commit writes (no quorum) — clients on those nodes get redirected to the majority
- On partition heal, minority nodes catch up from the Raft log

#### Failure impact summary

| Failure | Producers | Consumers | Data Loss | Recovery |
|---------|-----------|-----------|-----------|----------|
| Node failure (within quorum) | Unaffected | Stall 1–2 sec for queues whose leader was on that node, then resume | None (Raft state on followers) | Automatic leader election |
| Quorum lost for a queue | Blocked for that queue | Blocked for that queue | None (state persisted on surviving nodes) | Wait for node recovery to restore quorum |
| Network partition (minority side) | Redirected to majority | Redirected to majority | None | Automatic catch-up on heal |

**Key property:** There is no distinction between "storage failure" and "scheduler failure" — they are the same node, replicating the same Raft log. One failure model, one recovery path.

### Non-negotiable safety mechanisms

1. **Fencing tokens / epoch numbers** on every scheduling operation
2. **Ack-after-replicate** for message durability
3. **Lease records in Raft-replicated scheduler state** (not in-memory only)
4. **Idempotent consumers** as defense-in-depth
5. **Bounded lag monitoring** for any event stream between components

---

## Sources

### Architecture & Design
- [Large-scale cluster management at Google with Borg (2015)](https://research.google.com/pubs/archive/43438.pdf)
- [Omega: flexible, scalable schedulers for large compute clusters (2013)](https://research.google.com/pubs/archive/41684.pdf)
- [Borg, Omega, and Kubernetes (ACM Queue)](https://queue.acm.org/detail.cfm?id=2898444)
- [FOQS: Scaling a Distributed Priority Queue (Meta, 2021)](https://engineering.fb.com/2021/02/22/production-engineering/foqs-scaling-a-distributed-priority-queue/)
- [How Apache Pulsar Works (Jack Vanlightly)](https://jack-vanlightly.com/blog/2018/10/2/understanding-how-apache-pulsar-works)
- [Pulsar Architecture Overview](https://pulsar.apache.org/docs/next/concepts-architecture-overview/)

### Consensus & Raft
- [etcd Performance Guide](https://etcd.io/docs/v3.2/op-guide/performance/)
- [Raft in TiKV (PingCAP)](https://www.pingcap.com/blog/raft-in-tikv/)
- [CockroachDB Replication Layer](https://www.cockroachlabs.com/docs/stable/architecture/replication-layer)
- [Martin Kleppmann: How to do distributed locking](https://martin.kleppmann.com/2016/02/08/how-to-do-distributed-locking.html)

### Latency & Benchmarks
- [Network Latencies in the Data Center (Evan Jones)](https://www.evanjones.ca/network-latencies-2021.html)
- [Measuring Latencies Between AWS AZs](https://www.bitsand.cloud/posts/cross-az-latencies/)
- [Inside Apache Pulsar's Millisecond Write Path (StreamNative)](https://streamnative.io/blog/inside-apache-pulsars-millisecond-write-path-a-deep-performance-analysis)
- [Confluent: Tier-1 Bank Kafka Latency Tuning](https://www.confluent.io/blog/tier-1-bank-ultra-low-latency-trading-design/)

### Shard Rebalancing
- [Strimzi: Reassigning partitions in Kafka](https://strimzi.io/blog/2022/09/16/reassign-partitions/)
- [Redis Cluster Specification](https://redis.io/docs/latest/operate/oss_and_stack/reference/cluster-spec/)
- [CockroachDB Range Merges Tech Note](https://github.com/cockroachdb/cockroach/blob/master/docs/tech-notes/range-merges.md)
- [Vitess Resharding](https://vitess.io/docs/22.0/user-guides/configuration-advanced/resharding/)
- [BookKeeper Replication Protocol](https://bookkeeper.apache.org/docs/4.6.2/development/protocol/)

### Safety & Failure Modes
- [Jepsen: NATS 2.12.1 analysis (Dec 2025)](https://jepsen.io/analyses/nats-2.12.1)
- [How to lose messages on a Kafka cluster (Jack Vanlightly)](https://jack-vanlightly.com/blog/2018/9/18/how-to-lose-messages-on-a-kafka-cluster-part-2)
- [ZooKeeper Internals (ZAB)](https://zookeeper.apache.org/doc/r3.4.13/zookeeperInternals.html)
- [etcd failure modes docs](https://etcd.io/docs/v3.7/op-guide/failures/)

### Academic Papers
- [Pisces: Performance Isolation and Fairness (OSDI 2012)](https://www.usenix.org/system/files/conference/osdi12/osdi12-final-215.pdf)
- [2DFQ: Two-Dimensional Fair Queuing (SIGCOMM 2016)](https://cs.brown.edu/~jcmace/papers/mace162dfq.pdf)
- [DWRR: Distributed Weighted Round-Robin (PPoPP 2009)](https://happyli.org/tongli/papers/dwrr.pdf)
- [Calvin: Fast Distributed Transactions (SIGMOD 2012)](https://cs.yale.edu/homes/thomson/publications/calvin-sigmod12.pdf)
- [DRF: Dominant Resource Fairness (NSDI 2011)](https://amplab.cs.berkeley.edu/wp-content/uploads/2011/06/Dominant-Resource-Fairness-Fair-Allocation-of-Multiple-Resource-Types.pdf)
- [AWS Builders Library: Fairness in Multi-Tenant Systems](https://aws.amazon.com/builders-library/fairness-in-multi-tenant-systems/)
