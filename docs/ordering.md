# Ordering

> **Status:** design, not working code.

## No order by default

A queue makes no promise about delivery order. The scheduler prefers the oldest message
it can deliver, but a message may be delivered before an older one — because the older
one is waiting on a throttle, is delayed, or is being retried.

This is what lets a queue stay fair and throttled without one waiting message holding up
unrelated work behind it.

## Ordered queues

A queue that needs order asks for it when it is created:

```rust
// the whole queue, in arrival order
admin.create_queue(QueueSpec::new("audit").ordered()).await?;

// in arrival order per combination of properties
admin.create_queue(
    QueueSpec::new("account-events")
        .ordered_by([Key::fairness_key(), Key::header("account")])
).await?;
```

Messages with the same values for the ordering key form an **ordering group**. Each group
is delivered in arrival order. Key components may be headers, the fairness key, or
attributes returned by `on_enqueue`.

Ordering is a property of the queue rather than of consumers: whether events must be
applied in sequence is a statement about the data, made by whoever owns the queue.

**The ordering key is fixed when the queue is created.** Redefining groups over a live
backlog cannot preserve order under either the old or the new definition.

## What the guarantee is

Delivering messages in order is not enough on its own: two consumers can receive `m1`
and `m2` in order and still finish `m2` first. The guarantee is therefore:

> **At most one message per ordering group is in flight.** The next message in a group is
> delivered only after the current one is acknowledged or dead-lettered.

It follows that:

- A **nack that retries** holds the group until that message succeeds or is dead-lettered.
- A nack with **`retry_after`** holds the group for the delay.
- An **expired lease** is a failed attempt. Unless it is dead-lettered, the same message
  is redelivered before anything behind it.
- A **dead-lettered** message leaves its group, and the group continues without it.
  Redriving it later places it after messages that arrived in the meantime.
- **Throughput is bounded by the number of active groups.** An ordered queue with one
  group — `.ordered()` — processes one message at a time, however many consumers it has.

### Delayed messages

A message enqueued with a delay holds its group until it is delivered. `retry_after` and
enqueue delays follow one rule:

> The next message in a group is not delivered before its not-before time, and nothing
> behind it in the group is delivered first.

On a whole-queue ordered queue, a delayed message holds the entire queue.

## Example

Queue `account-events`, ordered by `account`, with `Unordered` for messages that have no
account. Two workers, each taking one message at a time.

Arrivals, in order:

```
a1 (A)   a2 (A)   s1 (-)   b1 (B)   s2 (-)   a3 (A)   b2 (B)
```

A keyed message is deliverable when it is the oldest pending message in its group and
nothing from its group is in flight. An unordered message is always deliverable. The
oldest deliverable message goes first.

| t | Event | W1 | W2 | Waiting |
|---|-------|----|----|---------|
| 0 | start | **a1** | **s1** | a2, a3 behind a1 |
| 1 | W2 acks s1 | a1 | **b1** | a2 behind a1 |
| 2 | W1 acks a1 | **a2** | b1 | a3 behind a2; b2 behind b1 |
| 3 | W2 acks b1 | a2 | **s2** | a3 behind a2 |
| 4 | W1 nacks a2, retry after 5s | **b2** | s2 | a2 not before t=9; a3 behind a2 |
| 5 | W2 acks s2 | b2 | idle | group A waits on a2 |
| 6 | W1 acks b2 | idle | idle | |
| 9 | a2 eligible | **a2** | | a3 behind a2 |
| 10 | W1 acks a2 | **a3** | | |

Group A is processed `a1, a2, a2 (retry), a3`. Group B is processed `b1, b2` and is
unaffected by A's failure. `s1` and `s2` are delivered whenever a worker is free.

## Messages without an ordering key

What happens to a message missing a value for the ordering key is chosen with the key:

```rust
QueueSpec::new("account-events")
    .ordered_by([Key::header("account")])
    .when_ordering_key_missing(MissingOrderingKey::Reject)      // default
    // .when_ordering_key_missing(MissingOrderingKey::Unordered)
```

| Policy | Effect |
|--------|--------|
| `Reject` (default) | The enqueue fails. A producer that forgot the key finds out immediately, and the message never enters the queue in the wrong place. |
| `Unordered` | The message belongs to no group and is delivered whenever a consumer is free. For queues that intentionally mix keyed and unkeyed messages. |

There is no shared group for messages without a key. It would serialize unrelated
messages and let one failing message hold up all the others, while still not ordering a
keyless message relative to the group it was meant to belong to.

## Ordering and fairness

Ordering groups may span fairness keys. The effect on fair scheduling depends on how
they relate:

| Ordering key | Effect on fairness |
|--------------|--------------------|
| Includes the fairness key | None. Every group lies within one fairness key. |
| Spans fairness keys | The scheduler must follow group order even when it would otherwise serve another fairness key next. |
| Whole queue (`.ordered()`) | Fairness no longer applies: arrival order decides delivery. |

## Ordering and throttling

A message waiting on an empty throttle bucket holds back only its own ordering group. On
a queue without ordering it holds back nothing; other messages are delivered.

## Ordering and sharding

Every message in an ordering group must live in the same shard, so **a sharded ordered
queue's shard key must be part of its ordering key**. See
[clustering.md](clustering.md#sharding).

## Future: ordered batches

A consumer could receive several messages of one group together, as an ordered batch.
The guarantee would become at most one batch in flight per group; if message *i* of a
batch fails, messages *i* onward return to pending in their original order.
