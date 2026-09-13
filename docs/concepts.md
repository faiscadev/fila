# Core Concepts

This document explains the key concepts behind Fila's scheduling and message handling.

## Message lifecycle

A message moves through these states:

```
Producer                      Broker                        Consumer
   |                            |                              |
   |-- Enqueue ----------------->|                              |
   |                            |-- on_enqueue (Lua) --------->|
   |                            |   assigns fairness_key,      |
   |                            |   weight                     |
   |                            |                              |
   |                            |-- Stored (pending) --------->|
   |                            |                              |
   |                            |-- DRR scheduler picks ------>|
   |                            |   checks consumer throttles  |
   |                            |                              |
   |                            |-- Consume (leased) -------->|-- Processing
   |                            |                              |
   |                            |<-------- Ack ----------------|  (success)
   |                            |   message deleted            |
   |                            |                              |
   |                            |<-------- Nack ---------------|  (failure)
   |                            |-- on_failure (Lua) --------->|
   |                            |   retry or dead-letter       |
   |                            |                              |
   |                            |-- Visibility timeout ------->|
   |                            |   re-enqueue if not acked    |
```

1. **Enqueue** — producer sends a message to a queue. If the queue has an `on_enqueue` Lua script, it runs to assign fairness key, weight and attributes. If the script fails, the queue's script failure policy decides what happens to the message.
2. **Pending** — the message is persisted to the storage engine and indexed by fairness key.
3. **Scheduled** — the DRR scheduler picks the next fairness key and checks the throttles declared by the queue's consumers. If every bucket the message draws from has a token, the message is delivered to a waiting consumer.
4. **Leased** — the consumer is processing the message. A visibility timeout timer starts.
5. **Acked** — the consumer confirms success. The message is deleted.
6. **Nacked** — the consumer reports failure. The `on_failure` hook, or the queue's retry policy, decides: retry or dead-letter.
7. **Expired** — the visibility timeout fires before an ack or nack. This is a failed attempt too, and is decided the same way as a nack.

## Fairness groups

Every message belongs to a **fairness group** identified by its `fairness_key`. The key is set by the producer or assigned by the queue's `on_enqueue` script; when both are present, the script wins.

A message with no fairness key joins the **unkeyed group**. It is a reserved group that no real key can collide with — a tenant named `default` is not the unkeyed group. A queue that never uses fairness keys is simply one group.

Common fairness key strategies:
- **Per-tenant**: `msg.headers["tenant_id"]` — prevents one tenant from monopolizing the queue
- **Per-customer**: `msg.headers["customer_id"]` — fair delivery across customers
- **Per-priority**: `msg.headers["priority"]` — combined with weights for priority scheduling

### Deficit Round Robin (DRR)

Fila uses the DRR algorithm to schedule delivery across fairness groups:

1. Each fairness key has a **deficit counter** (starts at 0) and a **weight** (default 1).
2. In each scheduling round, every key receives `weight * quantum` additional deficit.
3. The scheduler delivers messages from a key as long as its deficit is positive, decrementing by 1 per delivery.
4. When a key's deficit reaches 0 or it has no pending messages, the scheduler moves to the next key.

**Example**: Two tenants with equal weight and quantum=1000. Each gets 1000 deficit per round — the scheduler delivers ~1000 messages from tenant A, then ~1000 from tenant B, then back to A. A noisy tenant sending 100x more messages doesn't starve the quiet tenant.

**Weights**: A key with weight=3 gets 3x the deficit of a key with weight=1, so it receives ~3x the delivery bandwidth. Use weights for priority lanes.

## Throttling

Consumers declare the limits of the services they call, and the broker holds messages
until delivering them stays within those limits. The consumer never receives a message
it would have to reject for rate limiting.

```rust
let mut orders = consumer
    .subscribe("orders")
    .throttle(
        Throttle::named("stripe-per-customer")
            .key([Key::header("customer")])
            .limit(10, Duration::from_secs(1)),
    )
    .await?;
```

Each throttle has:

- **name** — declarations with the same name, from any consumer on any queue, share one limit
- **limits** — one or more `limit(N, W)`: at most N deliveries in any window of length W
- **key** (optional) — one bucket per distinct combination of headers, fairness key and
  attributes; messages missing the key share a bucket or are left unthrottled

A throttle applies to every message in the queue that has its key, whichever consumer
receives it. Before delivering a message, the scheduler checks every bucket the message
draws from; if any is full, the message stays pending.

See [throttling.md](throttling.md) for keys, conflict rules, and how limits hold across
a cluster.

## Lua hooks

Fila embeds a Lua 5.4 runtime for user-defined scheduling policy. Scripts run inside a sandbox with configurable timeouts and memory limits.

### on_enqueue

Runs when a message is enqueued. Returns scheduling metadata:

```lua
function on_enqueue(msg)
  -- msg.headers       — table of string key-value pairs
  -- msg.payload_size  — byte count of the payload
  -- msg.queue         — queue name

  return {
    fairness_key = msg.headers["tenant"],
    weight = tonumber(msg.headers["priority"]) or 1,
    attributes = { account = msg.headers["account"] }
  }
end
```

**Return fields:**
| Field | Type | When absent | Description |
|-------|------|-------------|-------------|
| `fairness_key` | string | the producer's value, or the unkeyed group | Groups the message for DRR scheduling |
| `weight` | number | `1` | DRR weight for this fairness key |
| `attributes` | table of strings | no attributes | Named values that throttle keys and ordering keys can include |

`on_enqueue` runs **once per message, at enqueue**, and its results are stored with the
message. They are never recomputed: changing the script affects messages enqueued after
the change, not the backlog.

### on_failure

Runs on every failed attempt — a nack, or a lease that expired — and decides retry vs.
dead-letter:

```lua
function on_failure(msg)
  -- msg.headers   — table of string key-value pairs
  -- msg.id        — message UUID
  -- msg.attempts  — deliveries so far, including this one; reset by redrive
  -- msg.redrives  — how many times the message has been redriven from the DLQ
  -- msg.queue     — queue name
  -- msg.reason    — "nack" or "lease_expired"
  -- msg.error     — error description from the nack; empty when the lease expired

  if msg.reason == "lease_expired" and msg.attempts >= 2 then
    return { action = "dlq" }   -- it keeps crashing workers
  end
  return { action = "retry", delay_ms = 1000 * msg.attempts }
end
```

**Return fields:**
| Field | Type | Description |
|-------|------|-------------|
| `action` | `"retry"` or `"dlq"` | Whether to retry or dead-letter |
| `delay_ms` | number (optional) | Delay before the retry is delivered (retry only) |

When the script runs successfully, **its decision is final**. The queue's retry policy does
not limit it: a script can retry more times than `max_attempts`, dead-letter earlier, or
retry forever. See [Retries](#retries).

### Lua API

Scripts can read runtime configuration from the broker:

```lua
local limit = fila.get("rate_limit:tenant_a")  -- returns string or nil
```

### Safety

| Setting | Default | Description |
|---------|---------|-------------|
| `lua.default_timeout` | `"10ms"` | Max script execution time |
| `lua.memory_limit` | `"1MB"` | Max memory per script |

**A slow or failing script on one queue must not affect delivery on any other queue.**
A script that times out on every message slows its own queue and applies backpressure to
that queue's producers; other queues are unaffected.

### When `on_enqueue` fails

A script error or timeout on a message is handled by the queue's **script failure
policy**, chosen at creation:

```rust
QueueSpec::new("orders")
    .on_enqueue(SCRIPT)
    .when_script_fails(ScriptFailure::Reject)            // default
    // .when_script_fails(ScriptFailure::Park { dead_letter_after: Some(10) })
```

| Policy | Effect |
|--------|--------|
| `Reject` (default) | The enqueue fails and nothing is stored. The error says whether retrying can help: a timeout may pass, a script error on the same input will not. |
| `Park` | The message is stored as **unclassified** and the script is retried later. |

There is no option to accept the message with default values. Falling back to defaults
would let a producer change how its messages are scheduled by making the script fail —
escaping its fairness group, gaining weight, or skipping a keyed throttle.

#### Parked messages

- Classification is retried when the queue's script changes, and periodically with
  backoff for failures that can pass on their own, such as timeouts.
- A parked message was never placed in a fairness or ordering group, so classifying it
  with a newer script is safe.
- **On an ordered queue, delivery stops at the first unclassified message.** It could
  belong to any group, so nothing that arrived after it can be delivered safely. On a
  queue without ordering, only the parked messages wait.
- `dead_letter_after` optionally moves a message to the dead-letter queue after that
  many failed classification attempts. An admin operation dead-letters unclassified
  messages on demand. Either way the message is kept, not dropped.
- Queue stats report the number of unclassified messages and when the oldest arrived.
  On an ordered queue, that is how long delivery has been stopped.

### When `on_failure` fails

The queue's retry policy decides, exactly as if the queue had no `on_failure` script.

Falling back to the policy is safe here in a way that falling back to defaults at enqueue
is not: it changes only how many times a message is retried, never where it is scheduled
or how much delivery share it gets. Making the script fail gains a producer nothing.

## Ordering

Queues make no promise about delivery order unless they ask for it. An ordered queue
delivers each **ordering group** — messages sharing values for the ordering key — in
arrival order, with at most one message per group in flight. See
[ordering.md](ordering.md).

## Retries

A delivery **fails** when the consumer nacks it or its lease expires. Every failure is an
attempt, and the next step is decided in this order:

1. If the queue has an `on_failure` script and it runs successfully, the script decides.
2. Otherwise — no script, or the script failed — the queue's **retry policy** decides.

### Retry policy

```rust
QueueSpec::new("orders")
    .retry(
        RetryPolicy::new()
            .max_attempts(3)
            .backoff(Backoff::exponential(Duration::from_secs(1)).max(Duration::from_secs(60))),
    )
```

| Setting | Default | Meaning |
|---------|---------|---------|
| `max_attempts` | `3` | Total deliveries before the message is dead-lettered — the first delivery and two retries |
| `backoff` | exponential from 1s, capped at 1m | Delay before each retry is delivered |

A queue without an explicit policy uses the defaults, so every queue has a limit unless an
`on_failure` script decides otherwise.

Retries are delayed by default because immediate retries turn one failing dependency into
a hot loop.

### Retry details

- **Expired leases count.** A message that crashes its worker is never nacked, so without
  counting expiry it would be redelivered forever.
- **`retry_after` on a nack** overrides the backoff delay for that retry, and still counts
  as an attempt.
- **On an ordered queue**, a retrying message holds its ordering group until it succeeds or
  is dead-lettered. See [ordering.md](ordering.md).

## Dead letter queue

**Every queue has a dead-letter queue**, created with it and named `<queue>.dlq`. Messages
go there when `on_failure` returns `{ action = "dlq" }` or the retry policy's attempts are
exhausted. For example, messages dead-lettered from `orders` go to `orders.dlq`.

- The `.dlq` suffix is reserved. A queue named `x.dlq` cannot be created directly.
- A dead-letter queue has no dead-letter queue of its own. A message that fails while
  being consumed from a dead-letter queue returns to it, delayed by backoff; the attempt
  limit does not apply there.
- On an ordered queue, a dead-lettered message leaves its ordering group, and the group
  continues without it. Redriving it later places it after messages that arrived in the
  meantime.

### Inspecting and redriving

```sh
# Check how many messages are in the DLQ
fila queue inspect orders.dlq

# Move 10 messages back to the source queue
fila redrive orders.dlq --count 10
```

Redrive moves pending (non-leased) messages from the DLQ back to its parent queue. People
redrive once the cause of the failures is fixed, so a redriven message gets a fresh start:

- **Attempts reset** to zero, so the message has its full retry budget again.
- **The redrive count increases.** `on_failure` sees it as `msg.redrives`, and deliveries
  carry it, so a script can leave a message dead-lettered once it has already been
  redriven several times.
- **The message ID stays the same**, so a message can be traced across redrives.
- **It re-enters as a new arrival**, behind messages already in the queue, and keeps its
  original `enqueued_at`. On an ordered queue it joins the back of its group.
- **Its enqueue delay still holds.** A message is not delivered before `enqueued_at +
  delay`; redriven after that time it is deliverable at once, redriven before it waits
  for the remainder. Retry delays do not carry over.

#### Reclassification

Each redrive chooses whether `on_enqueue` runs again:

```rust
admin.redrive("orders.dlq", 100, Reclassify::All).await?;          // default
admin.redrive("orders.dlq", 100, Reclassify::Unclassified).await?;
admin.redrive("orders.dlq", 100, Reclassify::None).await?;
```

| Mode | Classified messages | Unclassified messages |
|------|---------------------|-----------------------|
| `All` (default) | The current script runs again: new fairness key, weight and attributes | The current script runs |
| `Unclassified` | Keep their stored classification | The current script runs |
| `None` | Keep their stored classification | Not redriven; they stay in the DLQ |

Re-running the script is safe for ordering here, unlike on a live backlog: a dead-lettered
message left its ordering group, so it returns as a new arrival rather than moving between
groups while they have messages in flight. Unclassified messages — dead-lettered after
their script kept failing under `Park` — have no classification at all, which is why
`None` leaves them behind.

If the script fails during a redrive, the queue's script failure policy applies: under
`Reject` the message stays in the DLQ, under `Park` it is parked in the parent queue.

The result reports what stayed behind as well as what moved — messages left unclassified
under `None`, and messages the script rejected — so a partial redrive is never silent.

## Runtime configuration

The broker maintains a key-value configuration store that persists across restarts. Values are accessible from Lua scripts via `fila.get(key)` and managed through the admin API or the CLI.

```sh
fila config set feature:new_flow enabled
fila config get feature:new_flow
fila config list --prefix feature:
```

Common use cases:
- **Feature flags**: toggle behavior in Lua scripts without redeployment
- **Dynamic routing**: change fairness key assignment logic based on config values

## Delivery durability

In a cluster, each queue chooses whether its leases and acks are committed before they
take effect:

- **`Committed`** (default) — a leader crash loses nothing, and a successful ack means the
  message is never delivered again. Costs a commit round trip before messages go out.
- **`Fast`** — leases stay on the leader and acks succeed before commit. After a crash,
  in-flight and recently acked messages may be delivered again, after a short reclaim
  grace period in which reconnecting consumers can still ack or extend.

Planned leadership changes carry leases over for every queue. See
[clustering.md](clustering.md#what-replicates).

## Visibility timeout

When a consumer receives a message via `Consume`, the message is "leased" for a configurable duration (set per-queue at creation time via `visibility_timeout_ms`). During this lease:

- The message is not delivered to other consumers
- A timer tracks the lease expiry

If the consumer does not `Ack` or `Nack` the message before the timeout expires, the attempt has failed. It counts toward the retry limit and is decided like a nack — by `on_failure` with `msg.reason = "lease_expired"`, or by the retry policy — so a message is not lost when its consumer crashes, and a message that keeps crashing consumers is eventually dead-lettered.

The default visibility timeout is set per-queue at creation:

```sh
fila queue create orders --visibility-timeout 30000  # 30 seconds
```
