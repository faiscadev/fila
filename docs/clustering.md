# Clustering

> **Status:** design, not working code. Several decisions are still open; they are
> listed at the end.

A Fila cluster is several nodes that agree on state using Raft. A Raft **group** is a
set of nodes keeping identical copies of some state; one member is the **leader**,
which accepts writes, and the others replicate them.

## One group per queue

Every queue has its own Raft group. Queues never share a leader's write path, so a
busy queue does not slow down an unrelated one, and write capacity grows with the
number of queues spread across nodes.

A separate **meta group** holds state that is cluster-wide rather than per queue.

### Why this is viable

Per-queue groups work because a Fila deployment has few queues — tens, not thousands.
Tenants do not get their own queues: they share a queue and are separated by
**fairness key**, which is what the scheduler exists to balance.

The two decisions depend on each other. A use case that calls for a queue per tenant
would multiply the group count, and this design would have to be revisited first.

### Cost per group

Each group is an independent Raft instance with its own log, election timer and
heartbeats. Network connections between nodes are shared by all groups, but heartbeat
traffic grows with groups × peers. That cost is negligible at tens of groups and is
the reason group count should stay modest.

## The meta group

The meta group owns state that must be identical on every node and is written rarely
enough to afford consensus:

| State | Notes |
|-------|-------|
| Cluster membership | Which nodes exist |
| Queue registry | Which queues exist, their configuration (scripts, visibility timeout), group members and preferred leader |
| Runtime configuration | The key-value store Lua reads with `fila.get()` |
| API keys and ACLs | Read on every request, on every node |

These live in **one** group. They are small and rarely written; splitting them would
add elections and heartbeats, introduce partial-failure states such as authentication
reachable while the registry is not, and lose a single ordering across related
changes.

### Rules

- **Every administrative write goes through the meta group.** No admin operation may
  change state only on the node that received it. A key revoked on one node and still
  valid on another is a security failure, not an inconsistency.
- Creating a queue is one meta-group operation that registers the queue and
  establishes its Raft group.

### When the meta leader is unavailable

Administrative operations stop: no queue creation, no configuration or key changes.
Message delivery continues, because each queue's group is independent. This is the
intended failure mode.

### What the meta group does not own

| State | Where it lives |
|-------|----------------|
| Messages | The queue's own group |
| Throttle token state | The throttle grantor — soft state, not replicated; see [throttling.md](throttling.md) |
| Leases | Undecided; see open questions |

### The throttle grantor

The node enforcing cluster-wide throttle limits is a role, not a group. It runs on
the meta group leader and is located by throttle name, so grantors can move to groups
of their own — spread across nodes — without a design change. It needs leadership,
not a replicated log. The full mechanism is in [throttling.md](throttling.md).

## Dead-letter queues

A dead-letter queue lives in its parent queue's Raft group. Redrive moves messages
within one group and is therefore atomic; moving messages between groups atomically is
something per-queue groups cannot do.

Any future placement mechanism, including sharding, must keep a queue and its
dead-letter queue in the same group.

## Placement

When a queue is created, the cluster chooses its group automatically:

- **Preferred leader:** the node currently leading the fewest queues. Ties go to the
  lowest node ID.
- **Members:** when the cluster has more nodes than the replication factor, the queue's
  group is formed from that many least-loaded nodes.
- Operators do not place queues manually.

This decides initial placement only. Rebalancing is an open question.

## Sharding

Sharding lets a single queue span several Raft groups when one leader cannot keep up
with it. It is **added to** the per-queue model, never a replacement for it: a sharded
queue is split across groups of its own, and unrelated queues never share a group.

- Sharding is **opt-in, per queue**. A queue that does not need it pays nothing.
- **The user chooses the shard key**, and in doing so chooses between two guarantees:

  > Shard by the fairness key to keep per-key ordering, at the cost of exact fairness.
  > Shard by anything else to keep fairness, at the cost of per-key ordering.

  Sharding by fairness key means each tenant's messages live in one shard, and each
  shard balances only the tenants that landed in it. Sharding by an independent key
  spreads every tenant across all shards, so each shard's scheduler sees every tenant
  and the combined result stays fair, but one tenant's messages no longer arrive in
  order.

- A consumer of a sharded queue receives from every shard's leader. Merging those
  streams happens in the client, in the shared sans-io core, so every SDK inherits one
  implementation.

### Keeping sharding possible

Two rules keep sharding an addition rather than a rewrite:

- **Nothing derives a Raft group from a queue name.** Group lookup goes through one
  routing function; today it maps a queue to its own group.
- **Every replicated log entry carries its queue name**, even while a group holds one
  queue and the name looks redundant.

## Inter-node protocol

Nodes talk to each other over a protocol **separate from the client protocol**, with
its own opcode space and its own version. The two share only the frame format and
encoding primitives from [protocol.md](protocol.md).

They are kept apart because they differ in every property that matters:

| | Client protocol | Inter-node protocol |
|-|-----------------|---------------------|
| Audience | Anyone writing a client | Fila nodes deployed together |
| Change | Public contract; fields are never removed | Free to change between releases |
| Authentication | API keys and per-queue ACLs | Mutual TLS between nodes; no ACLs |
| Version skew | Unbounded and long-lived | Adjacent releases during a rolling upgrade |

The inter-node protocol is not yet specified.

## Open decisions

- **What replicates.** Whether leases are replicated state or held only by the queue
  leader, and what happens to in-flight messages when a leader changes. A nack with
  `retry_after` changes message state and must replicate; whether lease extension does
  depends on the lease decision.
- **Routing.** Which node serves which request: forwarding writes to a queue's leader,
  redirecting consumers with `NotLeader` and `leader_addr`, which node answers queue
  statistics, and how quickly a revoked API key must stop working on every node.
- **Rebalancing.** Moving leadership after failover, when nodes join, and when load is
  uneven. Placement today is decided only at queue creation.
- **Membership and upgrades.** Adding and removing nodes, bootstrapping a cluster,
  rolling upgrades, and a versioned format for replicated log entries so that adding a
  field never breaks replay of existing entries.
- **Sharding details.** How acks reach the shard holding a message when the shard key
  is not derivable from the message ID, how clients discover shards and their leaders,
  and whether shard placement should balance by fairness weight.
