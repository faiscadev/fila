# Deduplication

> **Status:** design, not working code.

## Why it is needed

A producer that sends a message and gets no answer cannot tell whether it arrived. The
leader may have committed the message and crashed before responding. Retrying is the only
safe choice for the producer, and without deduplication a retry after a committed enqueue
creates a second copy.

## Off by default

A queue does not deduplicate unless a message carries a key or the queue defines one.

### A key from the producer

```rust
producer.send(
    Message::new("orders", payload).idempotency_key("order-123")
).await?;
```

### A key defined by the queue

A queue can decide what makes two messages duplicates, using the same key model as ordering
and throttling — a combination of message properties:

```rust
QueueSpec::new("orders").deduplicate_by([Key::header("order_id")])
QueueSpec::new("orders").deduplicate_by([Key::payload()])                       // body
QueueSpec::new("orders").deduplicate_by([Key::all_headers()])                   // headers
QueueSpec::new("orders").deduplicate_by([Key::all_headers(), Key::payload()])   // whole message
QueueSpec::new("orders").deduplicate_by([Key::attribute("dedup")])              // from on_enqueue
QueueSpec::new("orders").deduplicate_by([Key::fairness_key(), Key::idempotency_key()])
```

| Key part | Value |
|----------|-------|
| `Key::header(name)` | That header's value |
| `Key::fairness_key()` | The message's fairness key |
| `Key::attribute(name)` | An attribute returned by `on_enqueue` |
| `Key::idempotency_key()` | The producer's idempotency key |
| `Key::payload()` | A hash of the payload |
| `Key::all_headers()` | A hash of every header |

**When a queue defines `deduplicate_by`, it decides the key**, and uses the producer's
idempotency key only if it includes `Key::idempotency_key()`. Otherwise a producer's key is
used when present. The queue owner's rule wins, as `on_enqueue` does over a producer's
fairness key.

A message missing a key part — a header that is absent, an attribute not returned — is not
deduplicated. Nothing breaks by delivering it; unlike ordering, there is no guarantee to
protect.

## How long a key is remembered

A key is remembered **while its message is in the queue or its dead-letter queue, and for
the dedup window after it leaves**. The window is set per queue and defaults to 10 minutes.

```rust
QueueSpec::new("orders")
    .deduplicate_by([Key::header("order_id")])
    .dedup_window(Duration::from_secs(600))
```

With the default window:

| Message | Key forgotten |
|---------|---------------|
| Acked 30 seconds after enqueue | 10 minutes after the ack |
| Waits an hour in a backlog, then acked | 10 minutes after the ack |
| Delayed 24 hours | 10 minutes after it is finally acked |
| Dead-lettered, sits in the DLQ for a day, then redriven and acked | 10 minutes after the ack |

Counting from when the message leaves means a key never expires while a copy of the message
could still be processed, however long it waits. A message in the dead-letter queue counts
as still in the queue: it is the same message, and a later redrive would otherwise process
a second copy. Redrive keeps the message's ID and key, so the key stays continuous.

## What a duplicate gets

An enqueue whose key matches a remembered key **succeeds**, returning the **original
message's ID** and a flag marking it a duplicate. No new message is created. A retry is
indistinguishable from the first attempt having succeeded, which is what the producer
needs.

## Guarantees

- **A key is committed in the same write as its message.** A leader crash cannot lose one
  without the other — a retry after a failover is exactly the case deduplication exists
  for.
- **Redriven messages are not checked.** They are not new messages.
- **Dedup is per queue.** The same key on two queues does not collide.

## Caveats

### A producer-supplied key can suppress another producer's messages

Keys are per queue, not per producer. If two tenants share a queue and one sends
`order-123` first, the other's `order-123` is treated as a duplicate and silently absorbed.

When producers are not trusted with each other's traffic, compute the key in `on_enqueue`
and include the tenant:

```lua
function on_enqueue(msg)
  local tenant, order = msg.headers["tenant"], msg.headers["order_id"]
  local attrs = {}
  if tenant and order then
    attrs.dedup = tenant .. ":" .. order   -- absent otherwise: not deduplicated
  end
  return { fairness_key = tenant, attributes = attrs }
end
```

### Hashing catches transport retries, not application retries

A client resending the same message produces the same hash. An application that builds the
message again — with a new timestamp or trace header — does not. And two messages that are
intentionally identical, such as the same reminder sent twice, are collapsed into one.
Prefer an explicit key when retries happen above the client.

### Script-computed keys and parked messages

When the key comes from `on_enqueue` and the queue parks messages whose script fails
([concepts.md](concepts.md#when-on_enqueue-fails)), the key is only known once the message
is classified. A duplicate found then is dropped — **after** the producer already received a
message ID for it. If that matters, use the `Reject` script failure policy on queues with
script-computed dedup keys.

### Sharded queues

Every message with the same key must reach the same shard, so a sharded queue's shard key
must be part of its dedup key. See [clustering.md](clustering.md#sharding).
