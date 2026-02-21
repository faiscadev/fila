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
   |                            |   weight, throttle_keys      |
   |                            |                              |
   |                            |-- Stored (pending) --------->|
   |                            |                              |
   |                            |-- DRR scheduler picks ------>|
   |                            |   checks throttle tokens     |
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

1. **Enqueue** — producer sends a message to a queue. If the queue has an `on_enqueue` Lua script, it runs to assign scheduling metadata.
2. **Pending** — the message is persisted in RocksDB and indexed by fairness key.
3. **Scheduled** — the DRR scheduler picks the next fairness key and checks throttle tokens. If tokens are available, the message is delivered to a waiting consumer.
4. **Leased** — the consumer is processing the message. A visibility timeout timer starts.
5. **Acked** — the consumer confirms success. The message is deleted.
6. **Nacked** — the consumer reports failure. The `on_failure` hook decides: retry (re-enqueue) or dead-letter.
7. **Expired** — if the visibility timeout fires before ack/nack, the message is automatically re-enqueued.

## Fairness groups

Every message belongs to a **fairness group** identified by its `fairness_key`. The key is assigned during enqueue — either by an `on_enqueue` Lua script or defaulting to `"default"`.

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

Fila supports per-key rate limiting via token bucket throttlers. Each throttle key has:

- **rate** — tokens refilled per second
- **burst** — maximum tokens the bucket can hold

When the scheduler is about to deliver a message, it checks all of the message's `throttle_keys`. If any bucket is empty, the message is held until tokens refill. The consumer never receives a message it would have to reject for rate limiting.

### Setting up throttle rates

Throttle rates are managed via runtime configuration:

```sh
# Allow 10 requests/second with burst of 20 for the "api" throttle key
fila config set throttle:api:rate 10
fila config set throttle:api:burst 20
```

Messages are assigned throttle keys in the `on_enqueue` Lua hook:

```lua
function on_enqueue(msg)
  return {
    fairness_key = msg.headers["tenant"],
    throttle_keys = { msg.headers["api_endpoint"] }
  }
end
```

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
    fairness_key = msg.headers["tenant"] or "default",
    weight = tonumber(msg.headers["priority"]) or 1,
    throttle_keys = { msg.headers["endpoint"] }
  }
end
```

**Return fields:**
| Field | Type | Default | Description |
|-------|------|---------|-------------|
| `fairness_key` | string | `"default"` | Groups the message for DRR scheduling |
| `weight` | number | `1` | DRR weight for this fairness key |
| `throttle_keys` | list of strings | `[]` | Token bucket keys to check before delivery |

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
| `lua.default_timeout_ms` | 10 | Max script execution time |
| `lua.default_memory_limit_bytes` | 1 MB | Max memory per script |
| `lua.circuit_breaker_threshold` | 3 | Consecutive failures before circuit break |
| `lua.circuit_breaker_cooldown_ms` | 10000 | Cooldown period after circuit break |

When the circuit breaker trips, Lua hooks are bypassed and messages use default scheduling (fairness_key=`"default"`, weight=1, no throttle keys). The circuit breaker resets automatically after the cooldown period.

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

The broker maintains a key-value configuration store that persists across restarts. Values are accessible from Lua scripts via `fila.get(key)` and managed through the CLI or gRPC API.

```sh
fila config set feature:new_flow enabled
fila config get feature:new_flow
fila config list --prefix feature:
```

Common use cases:
- **Feature flags**: toggle behavior in Lua scripts without redeployment
- **Throttle rates**: `throttle:<key>:rate` and `throttle:<key>:burst`
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
