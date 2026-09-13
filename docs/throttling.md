# Throttling

> **Status:** design, not working code. Open questions are listed at the end.

## What it is for

Workers usually call something with a rate limit — a payment API, an email provider,
a partner's endpoint. Exceed it and the call fails, so the worker has to back off,
retry, or requeue work it has already been handed.

Fila holds a message in the broker until delivering it would stay within the limit.
A worker never receives a job it cannot perform yet: no lease is taken, no attempt is
counted, no retry is burned.

```
producer ──enqueue──> Fila ──deliver (paced)──> worker ──call──> rate-limited API
```

## Consumers declare throttles

A throttle is declared by the consumer, when it subscribes:

```rust
let mut orders = consumer
    .subscribe("orders")
    .throttle(Throttle::named("stripe").rate(100, Duration::from_secs(1)).burst(150))
    .await?;
```

The consumer is the right owner because it is the code that knows what it calls.
Producers do not tag messages with throttle information and know nothing about
downstream limits. Moving from one provider to another means redeploying workers;
producers and already-enqueued messages are unaffected.

## Anatomy of a throttle

| Part | Meaning |
|------|---------|
| **Name** | Identifies the limit. Every declaration with the same name, from any consumer on any queue, draws from the same limit. |
| **Rate** | Tokens added per unit of time. |
| **Burst** | Maximum tokens a bucket holds. |
| **Partition** | Optional. Splits the limit into one bucket per distinct value. |

Without a partition, a throttle is one bucket. With one, a bucket is identified by
`name + partition value`, and each message draws from the bucket for its own value.

### Sharing by name

Two services calling Stripe from different queues both declare `stripe`, and together
they are held to one limit. Nothing needs to be coordinated through producers or
queue configuration — only the name.

## Partitioning

A throttle can be partitioned by a value read from each message:

```rust
// 10/s per customer, customer read from each message's headers
Throttle::named("stripe-per-customer")
    .partition_by_header("customer")
    .rate(10, Duration::from_secs(1))

// per tenant, using the fairness key
Throttle::named("reports-per-tenant")
    .partition_by_fairness_key()
    .rate(2, Duration::from_secs(1))

// by a value computed in Lua (see Attributes)
Throttle::named("stripe-per-account")
    .partition_by_attribute("account")
    .rate(10, Duration::from_secs(1))
```

A message may be subject to several throttles, and must have a token in every bucket
it draws from before it is delivered.

### Messages without a partition value

What happens to a message that lacks the value is part of the declaration:

```rust
Throttle::named("stripe-per-customer")
    .partition_by_header("customer")
    .when_missing(Missing::SharedBucket)   // default
    // .when_missing(Missing::Unthrottled)
```

| Policy | Effect |
|--------|--------|
| `SharedBucket` (default) | All messages lacking the value share one bucket. Safe: a producer that forgets the header shows up as one slow bucket rather than an unthrottled stream. |
| `Unthrottled` | Messages lacking the value are not subject to this throttle. |

The safe option is the default, so it is what you get by not choosing.

**`Unthrottled` can be bypassed by producers.** A producer that omits the header, or
sends a message the script returns no attribute for, skips the limit. Choose it only
where messages without the value genuinely do not use the throttled resource.

## Attributes

When a partition value has to be derived rather than read — parsed out of a header,
looked up in runtime config, combined from several fields — the queue's `on_enqueue`
script returns it as an attribute:

```lua
function on_enqueue(msg)
  return {
    fairness_key = msg.headers["tenant"],
    attributes = { account = account_for(msg.headers["customer"]) },
  }
end
```

The script belongs to the queue, so the rule is written by whoever owns the queue.
Consumers refer to an attribute by name; producers are unaware of it.

Attributes are computed **once, at enqueue**, stored with the message and never
recomputed. A change to the script applies to messages enqueued afterwards. Messages
already queued keep the attributes they were given, and a message enqueued before the
script produced an attribute does not have it, so the throttle's missing-value policy
applies.

Declaring a new partitioned throttle on a queue with a backlog works without running the
script again: partition values come from what is already stored — headers, fairness
keys and attributes.

If the script fails on a message, the queue's script failure policy applies. See
[concepts.md](concepts.md#when-on_enqueue-fails).

## Which deliveries a throttle paces

A throttle applies to **the whole queue**: a queue's deliveries are paced by the
combined throttles declared by everyone currently subscribed to it.

Consumers of a queue compete for its messages — each message goes to exactly one of
them — so they all perform the same job. Consumers on one queue declaring different
throttles is therefore almost always a deployment in progress or a mistake, not a
design:

- **A rolling deploy adding a throttle.** New workers declare it, old workers do not
  yet, but old workers call the same API. Applying the throttle to the whole queue
  protects the deploy from the first new worker onward.
- **A missing declaration.** One service forgot it. The queue is still protected.

The cost: a consumer that genuinely does not call the throttled resource is paced
anyway when it shares a queue with one that does. Give it its own queue. Queue stats
show a queue's active throttles, so the pacing is never unexplained.

A throttle stays in effect while at least one subscriber of the queue declares it.

## Conflicting declarations

| Situation | Outcome |
|-----------|---------|
| Same name, different rate or burst | **The strictest wins.** The effective values are shown in stats. A rolling deploy that lowers a limit takes effect as soon as the first new worker subscribes. |
| Same name, different partition | **The subscription is rejected** with `ThrottleConflict`. One name identifies one set of buckets; two partitionings under one name is a naming mistake. |

## Scheduling

Before delivering a message, the scheduler checks that every bucket the message draws
from has a token.

- If any bucket is empty, the message stays pending, untouched. No lease, no attempt.
- Tokens are consumed only after the message is successfully handed to a consumer.
- A message waiting on a bucket holds back nothing else on a queue without ordering,
  and only its own ordering group on an ordered queue. See [ordering.md](ordering.md).
- **A large backlog waiting on one empty bucket must not slow delivery of messages that
  draw from other buckets.** One customer's backlog cannot delay another customer.

## Bucket lifetime

A bucket is evicted once it has refilled to full. A full bucket is indistinguishable
from a new one, so evicting it loses nothing, and a partly-empty bucket is never
evicted, so eviction cannot hand out a free burst. There is no separate grace period.

This keeps high-cardinality partitions — one bucket per customer — bounded by the
number of *recently active* values rather than by every value ever seen.

## The guarantee

For every bucket, cluster-wide:

> In any window of length *t*, at most **B + R·t** deliveries,

where *R* is the rate and *B* the burst.

It holds across queue leaders on different nodes and across failover of the node
enforcing it, assuming node clocks measure elapsed time with bounded drift. Clocks do
not need to agree on the time of day.

### What it covers

The guarantee is on **deliveries**, not on calls the downstream receives. A worker may
call the API some time after receiving a message, calls from several workers can
bunch together, and a redelivered message draws another token. Downstream traffic
tracks the delivery rate closely but not exactly, which is why a real limit should be
declared with some headroom.

## Enforcement in a cluster

Queues sharing a throttle can have leaders on different nodes, and delivery happens on
each queue's leader. Enforcement must hold one limit across all of them without a
network call on the delivery path.

### Grantor and leases

One node acts as the **grantor**, holding the real bucket state. Queue leaders lease
tokens from it in batches and spend them locally, so delivery itself never waits on
the network.

- The grantor role runs on the meta group leader. It is located by throttle name, so
  grantors can later move to their own groups without changing the design.
- The grantor needs to be a leader; it does not need a replicated log. Nothing about
  throttle state goes through consensus.
- **Declarations are soft state.** Leaders send their subscribers' declarations with
  each lease request. The grantor derives the effective rate — strictest wins — from
  the requests it receives, and rebuilds this state from requests after a failover.

### The rules

The guarantee depends on each of these holding exactly:

1. **Leased tokens expire after *h*.** Expiry is measured from when the leader *sent
   the request*, not from when the grant arrived. A grant delayed in the network
   therefore cannot outlive its bound.
2. **The grantor runs its bucket with burst B − R·h.** This absorbs the slack that
   expiry introduces. It requires B ≥ R·h.
3. **The grantor confirms it is still leader before granting**, with a quorum check,
   after the requests it answers have arrived. One check covers a batch of requests.
4. **A new grantor waits *h*, plus a margin for clock drift, before its first grant,
   and starts every bucket empty.** By then every token issued by the previous grantor
   has expired, and starting empty prevents a burst from each side of the failover
   landing in one window.

### Why it holds

A token spent at time *s* was granted at some *g* with *g ≤ s ≤ g + h*, by rules 1 and
3. So tokens spent in a window *[x, x + t]* were granted in *[x − h, x + t]*, a window
of length *t + h*. The grantor's bucket admits at most *(B − R·h) + R·(t + h)* grants
in such a window, which is *B + R·t*.

Across a failover, rules 3 and 4 ensure the old grantor's tokens have all expired
before the new grantor issues any, and the new grantor's empty start means the two
sides together still fit the bound.

### Rate changes

The guarantee is measured against the rate in effect when tokens were granted. When a
stricter declaration lowers a rate, tokens already leased at the old rate can still be
spent, so the lower rate is fully in effect within *h*. There is no stall.

### Availability

When the grantor is lost, deliveries paced by its throttles pause for the election
plus *h*. Unthrottled deliveries continue.

This is a property of the guarantee rather than of the design: while nodes cannot
reach each other, a system either pauses or risks exceeding the limit. A strict limit
pauses.

## Open questions

- **Default burst and choosing *h*.** Rule 2 requires B ≥ R·h. Small bursts force a
  small *h*, approaching network round-trip time and multiplying lease requests. Needs
  a rule: derive *h*, require a minimum burst, or handle small bursts differently.
- **Messages drawing from several buckets.** A token leased from one bucket while
  another is empty expires unused. Safe, but it wastes capacity under contention.
  Accept, or reserve across buckets together.
- **Stats.** Partitioned throttles can have millions of buckets, so stats cannot list
  every one. What a queue's stats show for throttles is undecided.
- **Wire encoding** of throttle declarations in `Consume`.
- **Verify** that openraft exposes the leadership check rule 3 relies on.
