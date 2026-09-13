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
6. **Nacked** — the consumer reports failure. The `on_failure` hook decides: retry (re-enqueue) or dead-letter.
7. **Expired** — if the visibility timeout fires before ack/nack, the message is automatically re-enqueued.

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

## Token bucket throttling

Consumers declare the rate limits of the services they call, and the broker holds
messages until delivering them stays within those limits. The consumer never receives a
message it would have to reject for rate limiting.

```rust
let mut orders = consumer
    .subscribe("orders")
    .throttle(Throttle::named("stripe").rate(100, Duration::from_secs(1)).burst(150))
    .await?;
```

Each throttle has:

- **name** — declarations with the same name, from any consumer on any queue, share one limit
- **rate** — tokens refilled per unit of time
- **burst** — maximum tokens a bucket holds
- **partition** (optional) — one bucket per distinct value, read from a header, the
  fairness key, or an attribute computed by the `attributes` hook

A queue's deliveries are paced by the combined throttles of everyone subscribed to it.
Before delivering a message, the scheduler checks every bucket the message draws from;
if any is empty, the message stays pending.

See [throttling.md](throttling.md) for partitioning, conflict rules, and how limits
hold across a cluster.

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
| `attributes` | table of strings | no attributes | Named values that throttles can partition by and ordering keys can include |

`on_enqueue` runs **once per message, at enqueue**, and its results are stored with the
message. They are never recomputed: changing the script affects messages enqueued after
the change, not the backlog.

### on_failure

Runs when a consumer nacks a message. Decides retry vs. dead-letter:

```lua
function on_failure(msg)
  -- msg.headers   — table of string key-value pairs
  -- msg.id        — message UUID
  -- msg.attempts  — current attempt count
  -- msg.queue     — queue name
  -- msg.error     — error description from the nack

  if msg.attempts >= 3 then
    return { action = "dlq" }
  end
  return { action = "retry", delay_ms = 1000 * msg.attempts }
end
```

**Return fields:**
| Field | Type | Description |
|-------|------|-------------|
| `action` | `"retry"` or `"dlq"` | Whether to re-enqueue or dead-letter |
| `delay_ms` | number (optional) | Delay before re-enqueue (retry only) |

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
escaping its fairness group, gaining weight, or skipping a partitioned throttle.

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

The message is retried, as if the script had returned `{ action = "retry" }`. On an
ordered queue the retry holds the message's group.

## Ordering

Queues make no promise about delivery order unless they ask for it. An ordered queue
delivers each **ordering group** — messages sharing values for the ordering key — in
arrival order, with at most one message per group in flight. See
[ordering.md](ordering.md).

## Dead letter queue

Messages that exhaust retries (when `on_failure` returns `{ action = "dlq" }`) are moved to a dead letter queue named `<queue>.dlq`. For example, messages dead-lettered from `orders` go to `orders.dlq`.

### Inspecting and redriving

```sh
# Check how many messages are in the DLQ
fila queue inspect orders.dlq

# Move 10 messages back to the source queue
fila redrive orders.dlq --count 10
```

Redrive moves pending (non-leased) messages from the DLQ back to the original source queue, where they go through the normal enqueue flow again.

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

## Visibility timeout

When a consumer receives a message via `Consume`, the message is "leased" for a configurable duration (set per-queue at creation time via `visibility_timeout_ms`). During this lease:

- The message is not delivered to other consumers
- A timer tracks the lease expiry

If the consumer does not `Ack` or `Nack` the message before the timeout expires, the message is automatically re-enqueued and becomes available for delivery again. This prevents messages from being lost when consumers crash.

The default visibility timeout is set per-queue at creation:

```sh
fila queue create orders --visibility-timeout 30000  # 30 seconds
```
