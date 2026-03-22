# Consumer Group Semantics — Design Analysis

**Date:** 2026-03-22
**Context:** Epic 18 correct-course. Original implementation (PR #85) reverted.

## What Was Shipped (Wrong)

PR #85 implemented consumer groups as **labeled subsets that split throughput**:

- Each consumer group = 1 delivery target
- Independent consumers = 1 delivery target each
- Round-robin across all targets → groups compete for messages
- Within a group, round-robin picks which member gets the message
- A message is delivered to exactly one target (group or independent consumer)

**Example:** 2 groups + 1 independent consumer → each gets ~33% of messages. Groups split throughput.

## Correct Semantics (Kafka-Style)

Each consumer group gets an **independent view of every message**:

- Every message is delivered independently to each consumer group
- Within a group, messages are distributed so each is processed by exactly one member
- Each group has its own delivery state: pending set, lease tracking, ack/nack, retry count
- A message is "done" only when all groups have processed it

**Example:** 2 groups → each group receives 100% of messages. Ack in group A does not affect group B.

## Per-Group Implications

The fundamental shift: many things that are currently **per-queue** need to become **per-consumer-group**.

| Concept | Currently | Needs to be |
|---------|-----------|-------------|
| Pending set (undelivered messages) | Per-queue | Per-group |
| Lease / visibility tracking | Per-queue | Per-group |
| Ack/Nack state | Per-queue (single lifecycle) | Per-group (independent lifecycles) |
| Retry count (nack count per message) | Per-message | Per-message-per-group |
| DLQ routing (max_retries exhaustion) | Per-queue | Per-group (group A DLQs, group B still processing) |
| DRR state (deficit, weights, round-robin) | Per-queue | Per-group (each group runs its own DRR) |
| Delivery round-robin index | Per-queue | Per-group (within-group member selection) |
| `on_failure` Lua hook | Per-message failure | Per-group failure (group A retries, group B unaffected) |
| Queue stats (pending count, lag) | Per-queue | Per-group + per-queue aggregate |
| Message lifecycle completion | Acked once → done | All groups acked → done |

### Things that likely stay per-queue (shared)

| Concept | Rationale |
|---------|-----------|
| Throttle state (token buckets) | Rate limits protect downstream systems, not per-group |
| `on_enqueue` Lua hook | Runs once at enqueue time, before group fan-out |
| Message storage (payload) | Stored once, referenced by all groups |

## Open Design Questions

### 1. Fairness key and weight — per-message or per-group?

Currently `on_enqueue` sets `fairness_key` and `weight` at enqueue time — baked into the message. These drive DRR scheduling, which is a consumption-time concern.

**Options:**
- **A) Keep per-message:** All groups use the same fairness_key/weight. Simpler, but all groups must share the same fairness model.
- **B) Per-group Lua hook:** Each group has its own `on_deliver` (or similar) hook that computes fairness_key/weight at delivery time. Maximum flexibility but significantly more complex.
- **C) Per-group DRR config:** Groups can opt into or out of fairness scheduling. The fairness_key stays per-message, but whether DRR is applied is per-group.

### 2. Throttle keys — per-message or per-group?

Currently `on_enqueue` sets `throttle_keys`. If throttling is a consumption concern:

**Options:**
- **A) Shared throttle:** All groups share the same token buckets. A rate limit affects all groups.
- **B) Per-group throttle:** Each group has its own token bucket state. Group A can be throttled while group B is unthrottled.
- **C) Configurable:** Default shared, per-group override available.

### 3. `on_enqueue` scope

If fairness and throttle are consumption concerns, should `on_enqueue` still set them? Or should there be a separate hook that runs at delivery time (per-group)?

This would mean `on_enqueue` becomes purely about message classification (metadata, routing), and a new `on_deliver` or per-group config handles scheduling policy.

### 4. Independent consumers behavior

What happens with consumers that don't specify a group?

**Options:**
- **A) Implicit default group:** All independent consumers form one implicit group that shares messages (current behavior, backward compatible).
- **B) Each independent = own group:** Every independent consumer gets all messages. Breaking change.
- **C) Mixed:** Independent consumers share a pool (current behavior). Named groups get independent views. Most backward-compatible.

### 5. Message completion and storage cleanup

When is a message eligible for deletion?

- When all currently-subscribed groups have acked it?
- What if a new group joins after a message was enqueued — does it see historical messages or only new ones?
- What about groups with no active members — do messages accumulate indefinitely?

### 6. Storage model

Per-group state needs persistence. Options:

- **A) Per-group column family in RocksDB:** Clean isolation but potentially many CFs.
- **B) Composite key prefix:** `(queue_id, group_id, message_id)` in existing CFs.
- **C) Separate pending index per group:** Extend the existing pending index pattern.

### 7. Raft log implications

`ClusterRequest::Ack` and `ClusterRequest::Nack` need group context. New field with `#[serde(default)]` for backward compat. But the completion check (all groups acked?) needs to know which groups are subscribed — group membership may need to be part of Raft state, not just scheduler-local.

### 8. Interaction with existing features

- **Redrive:** Redrive from DLQ — per-group or global? If group A DLQ'd a message but group B acked it, redrive for group A should only affect group A.
- **Queue deletion:** Deleting a queue removes all group state.
- **Stats:** `GetStats` needs per-group metrics (lag, pending count, consumer count) alongside queue-level aggregates.
- **CLI:** `fila stats` should show per-group breakdown.

## Recommendation

This is a significant architectural change. The stories should not be defined until these design questions are resolved. Recommended approach:

1. Resolve design questions (especially #1, #2, #3 which determine whether the Lua hook model changes)
2. Write a technical design document with the chosen approach
3. Break into stories based on the design
4. Implement with the full test safety net (432 tests + e2e suite)

The feature is post-MVP and there is no timeline pressure. Getting the semantics right is more important than shipping fast.
