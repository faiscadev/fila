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
| Throttle grant state | The throttle grantor — soft state, rebuilt from the queues' delivery records after a failover; see [throttling.md](throttling.md#failover) |
| Leases and acks | The queue's own group, per its delivery durability — see [What replicates](#what-replicates) |

### The throttle grantor

The node enforcing cluster-wide throttle limits is a role, not a group. It runs on
the meta group leader and is located by throttle name, so grantors can move to groups
of their own — spread across nodes — without a design change. It needs leadership, not a
replicated log: after a failover, a new grantor rebuilds recent history from the delivery
records each queue keeps. The full mechanism is in [throttling.md](throttling.md).

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
- **The user chooses the shard key**, and the choice affects fairness:

  > Shard by a key independent of the fairness key to keep fairness exact. Shard by the
  > fairness key and each shard balances only the fairness keys that landed in it.

  Sharding by an independent key spreads every tenant across all shards, so each
  shard's scheduler sees every tenant and the combined result stays fair. Sharding by
  fairness key puts each tenant in one shard, so a tenant competes only with the tenants
  that share its shard.

- **An ordered queue's shard key must be part of its ordering key**, so that every
  message of an ordering group lives in the same shard. See [ordering.md](ordering.md).

- A consumer of a sharded queue receives from every shard's leader. Merging those
  streams happens in the client, in the shared sans-io core, so every SDK inherits one
  implementation.

### Keeping sharding possible

Two rules keep sharding an addition rather than a rewrite:

- **Nothing derives a Raft group from a queue name.** Group lookup goes through one
  routing function; today it maps a queue to its own group.
- **Every replicated log entry carries its queue name**, even while a group holds one
  queue and the name looks redundant.

## What replicates

A change to a queue counts once its Raft group has committed it. Whether a piece of state
must be committed first comes down to what breaks if a leader crash loses it.

| State | Committed | Why |
|-------|-----------|-----|
| Enqueued messages, with their classification | Before the producer receives a message ID | An accepted message must not disappear |
| Nacks, `retry_after`, not-before times | Yes | Losing them retries at once — a hot loop, and an ordering group released early |
| Attempt and redrive counts | Yes | Otherwise a message can exceed `max_attempts` by bouncing across failovers |
| Parking and later classification | Yes | Message state other nodes must agree on |
| Leases and acks | Depends on the queue's delivery durability | See below |
| Fairness scheduler state | No | Rebuilt by a new leader; fairness is briefly approximate |
| Subscriptions and delivery credit | No | Belong to connections; consumers reconnect |

### Delivery durability

Each queue chooses how its leases and acks are handled:

```rust
QueueSpec::new("emails").delivery_durability(DeliveryDurability::Fast)   // default: Committed
```

| | `Committed` (default) | `Fast` |
|-|-----------------------|--------|
| Leases | Committed before the message leaves the leader | Held by the leader only |
| Acks | Succeed after commit | Replicated, but succeed without waiting |
| After a leader crash | Nothing is lost | In-flight and recently acked messages may be delivered again |

#### `Committed`

- Deliveries are committed before messages leave the leader, **batched per scheduling
  pass** — one write for many deliveries — so the cost is a commit round trip of latency,
  not a write per message. Lease extensions are committed the same way.
- A successful ack means the message is never delivered again.
- A new leader knows every lease. It cannot know how much of a lease has already elapsed
  without trusting node clocks to agree, so it **restarts each in-flight lease at its full
  duration**. A crashed consumer's messages may be redelivered up to one visibility timeout
  later than otherwise, never earlier.

#### `Fast`

- Leases are held by the leader only, and acks succeed before they are committed.
- **On an ordered queue, acks still wait for commit.** A lost ack there would reprocess a
  message after the messages behind it, breaking the order itself.
- On a crash: messages in flight, and messages acked but not yet committed, can be
  delivered again; the attempt in flight is not counted; extensions of in-flight leases
  are lost.

#### Recovering a `Fast` queue after a crash

- Clients retry their pending acks, nacks and lease extensions against the new leader.
  This belongs to the shared client core, so every SDK does it.
- For the queue's **reclaim grace** — 5 seconds by default, configurable per queue:
  - an ack or nack resolves its message;
  - an `ExtendLease` for a lease the new leader does not know **re-establishes the lease**
    for that consumer, so long-running jobs that heartbeat are protected.
- During the grace period, only messages that existed before the new leader took over
  are held — on an ordered queue, only ordering groups containing such a message. Messages
  enqueued after the takeover are delivered normally.
- When the grace period ends, unresolved messages from before the takeover are delivered
  again.

A job that runs longer than the grace period without extending its lease is delivered
again. Such a job must extend its lease anyway to outlive the visibility timeout.

On an ordered queue, one exception to "at most one message per group in flight" remains:
a group's head message can be processed twice at once if its worker is alive but cannot
reach the new leader within the grace period.

### Controlled handovers

When leadership moves on purpose — a rolling upgrade, rebalancing, removing a node — the
old leader stops delivering, commits its in-flight leases, and then transfers leadership.
This applies to every queue, so a `Fast` queue loses nothing in a planned handover. The
pause is a single commit.

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

- **Routing.** Which node serves which request: forwarding writes to a queue's leader,
  redirecting consumers with `NotLeader` and `leader_addr`, which node answers queue and
  throttle statistics, and how quickly a revoked API key must stop working on every node.
- **Rebalancing.** Moving leadership after failover, when nodes join, and when load is
  uneven. Placement today is decided only at queue creation.
- **Membership and upgrades.** Adding and removing nodes, bootstrapping a cluster,
  rolling upgrades, and a versioned format for replicated log entries so that adding a
  field never breaks replay of existing entries.
- **Sharding details.** How acks reach the shard holding a message when the shard key
  is not derivable from the message ID, how clients discover shards and their leaders,
  and whether shard placement should balance by fairness weight.
