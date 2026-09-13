# Throttling

> **Status:** design, not working code.

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

The broker's advantage over a worker simply waiting is that it can **skip a message
that cannot go and deliver one that can**. That depends on throttles covering some
messages and not others, or splitting them into separate buckets — see
[Keys](#keys).

## Consumers declare throttles

A throttle is declared by the consumer, when it subscribes:

```rust
let mut orders = consumer
    .subscribe("orders")
    .throttle(Throttle::named("stripe").limit(100, Duration::from_secs(1)))
    .await?;
```

The consumer is the right owner because it is the code that knows what it calls.
Producers know nothing about downstream limits. Moving from one provider to another
means redeploying workers; producers and already-enqueued messages are unaffected.

Declarations with the same name — from any consumer, on any queue — share one limit.
Two services calling Stripe from different queues both declare `stripe` and together
stay within it.

## Limits

A limit is a cap over a sliding window:

```rust
Throttle::named("stripe").limit(100, Duration::from_secs(1))
```

> **At most N deliveries in any window of length W.**

Up to N may go out at once. Nothing more goes out until those leave the window, and
capacity comes back exactly as it was used, W later. A quiet period does not use up any
allowance.

### Several limits

A throttle can carry several limits, and a message is delivered only if it fits all of
them. A longer limit caps the total; a shorter one caps how concentrated it can be:

```rust
Throttle::named("stripe")
    .limit(60, Duration::from_secs(60))   // at most 60 per minute
    .limit(20, Duration::from_secs(1))    // and at most 20 in any second
```

With these, `10, 20, 10, 5, 5, 1, 1, 1, 1, 1, 5` in consecutive seconds is allowed: no
second exceeds 20 and the total is 60. Nothing more goes out until the first of those
deliveries is a minute old.

A declaration containing a limit that can never apply is rejected, since it is almost
certainly a mistake:

| Declaration | Why a limit never applies |
|-------------|---------------------------|
| 60 per minute, 100 per second | At most 60 can ever go in a second |
| 20 per second, 2,000 per minute | 20 per second allows at most 1,200 per minute |

This applies within one declaration only. Different consumers declaring different limits
under one name is normal — see [Conflicting declarations](#conflicting-declarations).

## Keys

A throttle can have a **key**, a combination of message properties: headers, the
fairness key, or attributes returned by `on_enqueue`. Messages with the same key values
share a bucket; each bucket is held to the throttle's limits separately.

```rust
// 10/s per customer
Throttle::named("stripe-per-customer")
    .key([Key::header("customer")])
    .limit(10, Duration::from_secs(1))

// per tenant, using the fairness key
Throttle::named("reports-per-tenant")
    .key([Key::fairness_key()])
    .limit(2, Duration::from_secs(1))
```

A throttle without a key is one bucket for every message it applies to.

Keys work the same way as ordering keys ([ordering.md](ordering.md)): a key defines
groups, and a message missing the key is handled by a policy.

### Messages without the key

```rust
Throttle::named("stripe-per-customer")
    .key([Key::header("customer")])
    .when_key_missing(Missing::SharedBucket)   // default
    // .when_key_missing(Missing::Unthrottled)
```

| Policy | Effect |
|--------|--------|
| `SharedBucket` (default) | Every message lacking the key shares one bucket. Safe: a producer that forgets the header shows up as one slow bucket rather than an unthrottled stream. |
| `Unthrottled` | Messages lacking the key are not subject to this throttle. |

### Throttling only some of a queue's messages

`Unthrottled` is how a queue mixing different kinds of work keeps the rest flowing. The
queue's `on_enqueue` script sets an attribute only on messages that use the resource:

```lua
function on_enqueue(msg)
  local attrs = {}
  if msg.headers["job_type"] == "charge" then
    attrs.stripe_account = msg.headers["account"]
  end
  return { attributes = attrs }
end
```

```rust
Throttle::named("stripe-per-account")
    .key([Key::attribute("stripe_account")])
    .when_key_missing(Missing::Unthrottled)
    .limit(10, Duration::from_secs(1))
```

Email jobs in the same queue have no `stripe_account`, never touch the throttle, and are
delivered while Stripe's buckets are empty.

- **Use one attribute per resource** — `stripe_account`, `sendgrid_domain`. A shared
  attribute such as `provider` would put every provider's messages under one throttle,
  with one limit.
- **Keys built from headers can be bypassed by producers.** A producer that omits the
  header avoids a throttle whose missing-key policy is `Unthrottled`. Keys built from
  attributes are computed by the queue's script and cannot be bypassed that way.

Attributes are computed once, at enqueue, and stored with the message. A new keyed
throttle on a queue with a backlog uses the stored headers, fairness keys and attributes
without running the script again. See [concepts.md](concepts.md#lua-hooks).

## Which messages a throttle applies to

A throttle declared by any current subscriber of a queue applies to **every message in
that queue that has its key** — or to every message, when it has no key or its
missing-key policy is `SharedBucket` — no matter which consumer receives the message.

Consumers of a queue compete for its messages, so they all perform the same job, and
consumers declaring different throttles on one queue is almost always a deployment in
progress or a mistake:

- **A rolling deploy adding a throttle.** New workers declare it, old workers do not yet,
  but old workers call the same API. The throttle applies from the first new worker
  onward, whoever receives the message.
- **A missing declaration.** One service forgot it. The queue is still protected.

A throttle stays in effect while at least one subscriber of the queue declares it. Queue
stats show a queue's active throttles, so pacing is never unexplained.

## Conflicting declarations

| Same name, but different… | Outcome |
|---------------------------|---------|
| Limits | **Every declared limit applies** — the strictest combination. A rolling deploy that lowers a limit takes effect as soon as the first updated worker subscribes; one that raises it takes effect once no running worker declares the old value. |
| Missing-key policy | **The strictest wins:** `SharedBucket`. |
| Key | **The subscription is rejected** with `ThrottleConflict`. One name identifies one set of buckets. |

## Scheduling

Before delivering a message, the scheduler checks that every throttle bucket the message
draws from allows one more delivery.

- If any does not, the message stays pending, untouched. No lease, no attempt.
- Allowance is counted when tokens are granted. Tokens a queue leader cannot use are
  returned, so what stays counted is what was delivered. See
  [Granting without waste](#granting-without-waste).
- A waiting message holds back nothing else on a queue without ordering, and only its
  own ordering group on an ordered queue. See [ordering.md](ordering.md).
- **A large backlog waiting on one bucket must not slow delivery of messages that draw
  from other buckets.** One customer's backlog cannot delay another customer.

## Bucket lifetime

A bucket's only state is the deliveries still inside its longest window. A bucket with
none is indistinguishable from a new one and can be evicted; a bucket with deliveries
still in the window is kept.

This keeps keyed throttles — one bucket per customer — bounded by the number of
*recently active* key values rather than every value ever seen.

## The guarantee

For every limit of every bucket, cluster-wide:

> **At most N deliveries in any window of length W.**

It holds across queue leaders on different nodes and across failover of the node
enforcing it, assuming node clocks measure elapsed time with bounded drift. Clocks do not
need to agree on the time of day.

### What it covers

The guarantee is on **deliveries**, not on calls the downstream receives:

- A worker calls the API some time after receiving a message, and calls from workers
  with uneven processing time can bunch together.
- A worker retrying a call internally makes calls Fila never sees. A redelivered message
  does count as another delivery.
- Traffic to the same API that does not go through Fila is not counted.
- A limit on **concurrent** requests is a different constraint, controlled by consumer
  credit (`prefetch`), not by a throttle.

When declaring a provider's limit, leave headroom below it — `limit(90, 1s)` for a
documented 100 per second.

## Enforcement in a cluster

Queues sharing a throttle can have leaders on different nodes, and delivery happens on
each queue's leader. Enforcement must hold one limit across all of them without a network
call on the delivery path.

### Grantor and leases

One node acts as the **grantor**, holding the authoritative count for each bucket. Queue
leaders lease **tokens** — each permission for one delivery — in batches and spend them
locally, so delivery itself never waits on the network.

- The grantor role runs on the meta group leader and is located by throttle name, so
  grantors can later move to groups of their own without changing the design.
- **Declarations are soft state.** Leaders send their subscribers' declarations with each
  lease request. The grantor derives the effective limits, keys and policies from the
  requests it receives, and rebuilds them from requests after a failover.

### Granting without waste

The grantor counts a token against a limit when it is **granted**, and cannot know it
went unused. An unused token therefore uses up allowance for the rest of the window — for
an hourly limit, an hour. Two rules keep grants close to what is actually delivered:

- **A delivery's tokens are granted together or not at all.** A leader requests
  deliveries, each naming every bucket it draws from — `stripe` plus one customer's
  `stripe-per-customer` bucket — and the grantor grants a delivery only when all of them
  have room, counting them at once. No token is granted for a delivery another bucket
  would block.
- **Leaders return tokens they will not use** — when a consumer disconnects, its credit
  drops, or the backlog drains. The leader invalidates the tokens first and then returns
  them, so a returned token can never also be spent, and a returned token stops counting.
  Returns are best effort: a lost return only wastes allowance, never exceeds a limit.
  Returns need not be recorded for recorded windows; after a failover an unrecorded
  return is simply counted, which is conservative.

Granting together relies on every bucket of a delivery being on one grantor, which holds
while the grantor role runs on the meta group leader. If grantors are later split across
groups, throttles used together must share a grantor, or deliveries spanning grantors fall
back to returns alone.

### The rules

The guarantee depends on each of these holding exactly:

1. **Leased tokens expire after *h*,** measured from when the leader *sent the request*,
   not from when the grant arrived. A grant delayed in the network cannot outlive its
   bound.
2. **For each limit, the grantor grants at most N tokens in any window of length W + h.**
   This absorbs the slack expiry introduces.
3. **The grantor confirms it is still leader before granting**, with a quorum check,
   after the requests it answers have arrived. One check covers a batch of requests. The
   check must be a quorum round trip (Raft's ReadIndex) — **never a clock-based lease
   read**, which assumes clocks agree more closely than this guarantee does.
4. **A new grantor does not grant until it can account for every token that might still
   be spent** — by waiting, or from recorded grants. See [Failover](#failover).

### Why it holds

A token spent at time *s* was granted at some *g* with *g ≤ s ≤ g + h*, by rules 1 and 3.
Tokens spent in a window *[x, x + W]* were therefore granted in *[x − h, x + W]*, a window
of length *W + h*, and rule 2 allows at most N grants in it.

The cost is a sustained ceiling of *N / (W + h)* rather than *N / W*: about 98 per second
for a limit of 100 per second with a 20ms lease.

### Failover

A window limit depends on recent history — how many deliveries are already inside the
window — so a new grantor must know it or wait for it to pass. Which applies is decided
per throttle by its longest window and the broker's `throttle.max_failover_pause`
(default 10s; see [configuration.md](configuration.md#throttle)):

| Longest window | Enforcement | Throttled delivery after grantor failover |
|----------------|-------------|------------------------------------------|
| Up to `max_failover_pause` | Grants are soft state. The new grantor waits **W + h**, plus a margin for clock drift, before its first grant. | Pauses for the election plus about W |
| Longer | **Grants are recorded** through the meta group. A grant is handed out only after it is committed, and a grantor that has lost leadership cannot commit. The new grantor resumes from the committed grants. | Pauses only for the election |

**Why waiting works.** A leadership check proves the grantor was leader when its probes
went out, not that it still is when it hands tokens out; a grantor can be deposed in
between. That gap is closed by rule 1: every token answers a request sent *before* the
check, and expires within *h* of that request, so every token the previous grantor issued
is spent or expired within *h* of a moment before the new grantor's election. A new
grantor granting only after a further *W + h* ensures that any window containing a new
delivery starts after every old one.

Recorded grants need no such argument: they are committed through the meta group before
they are handed out, and a deposed grantor cannot commit.

**Why recording is affordable for long windows.** A lease's length costs throughput in
proportion to *h / W*, so long windows can use long leases and need few lease requests —
and therefore few recorded writes. Recording one grant also covers the throttle's shorter
limits, so none of them pause either.

A queue leader's delivery records surviving failover, if they do, would let a new grantor
rebuild recent history without recording grants. That depends on what the cluster
replicates, which is still open ([clustering.md](clustering.md#open-decisions)).

### Limit changes

The guarantee is measured against the limits in effect when tokens were granted. When a
stricter declaration lowers a limit, deliveries already inside the window stay counted,
so the lower limit is fully in effect within *W + h*. There is no stall.

### Availability

When the grantor is lost, deliveries paced by its throttles pause as described above.
Unthrottled deliveries continue.

This is a property of the guarantee rather than of the design: while nodes cannot reach
each other, a system either pauses or risks exceeding the limit. A strict limit pauses.

## Stats

A keyed throttle can have millions of buckets, so stats never list them all. They answer
three questions: is a throttle holding my queue, which key values are being held, and is
my limit set right.

### Queue stats

- **`throttled`** — messages pending because a throttle bucket has no room, alongside
  `delayed` and `unclassified`.
- **Active throttles** — for each throttle currently declared on the queue: name,
  effective limits after combining every declaration, key, missing-key policy, how many
  subscribers declare it, and whether its grants are soft state or recorded.

### Throttle stats

Across every queue where a throttle is active:

```rust
let stripe = admin.throttle_stats("stripe-per-customer", TopBuckets(10)).await?;
// effective limits, key, missing-key policy, enforcement
// queues where it is active
// active_buckets     — buckets with deliveries still inside the window
// buckets_at_limit   — buckets with no room right now
// waiting            — messages held by this throttle
// top_buckets        — the 10 with the most messages waiting

let acme = admin.throttle_bucket("stripe-per-customer", ["acme"]).await?;
// deliveries counted against each limit, messages waiting, when room next frees up
```

`TopBuckets` defaults to 10. An unkeyed throttle has one bucket, reported directly.

### Metrics

Throttle metrics carry the throttle's **name, never a key value** — a label per customer
would explode metrics storage. Individual buckets are inspected through the admin API.

| Metric | Type | Meaning |
|--------|------|---------|
| `fila.throttle.limited` | Counter | Messages held by a throttle limit |
| `fila.throttle.active_buckets` | Gauge | Buckets with deliveries inside the window |
| `fila.throttle.buckets_at_limit` | Gauge | Buckets with no room |
| `fila.throttle.waiting` | Gauge | Messages held by the throttle |
| `fila.throttle.tokens_expired` | Counter | Leased tokens that expired unused — steady growth means grants are outrunning deliveries |

Which node answers a stats request is part of routing, still open in
[clustering.md](clustering.md#open-decisions): throttle counts live with the grantor,
and waiting messages with each queue's leader.
