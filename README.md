# Fila

A message broker that makes fair scheduling and per-key throttling first-class primitives.

> **Status:** Design, not working code. This describes the system being built.

## The problem

Every existing broker delivers messages in FIFO order. When multiple tenants, customers, or workload types share a queue, a single noisy producer can starve everyone else. Rate limiting is pushed to the consumer — which means the consumer has to fetch a message, check the limit, and re-enqueue it. That wastes work and adds latency.

Fila moves scheduling decisions into the broker:

- **Deficit Round Robin (DRR) fair scheduling** — each fairness key gets its fair share of delivery bandwidth. No tenant starves another.
- **Token bucket throttling** — consumers declare the rate limits of what they call, and the broker paces delivery to stay within them. Consumers only receive messages that are ready to process.
- **Lua rules engine** — `on_enqueue` and `on_failure` hooks let you define scheduling policy in user-supplied Lua scripts, for the cases where static configuration isn't enough.
- **Zero wasted work** — consumers never receive a message they can't act on.

## Key concepts

| Concept | What it does |
|---------|-------------|
| **Fairness keys** | Messages are grouped by a `fairness_key`. The DRR scheduler gives each group its fair share of delivery bandwidth, in proportion to its `weight`. |
| **Throttling** | Consumers declare named rate limits when subscribing, optionally partitioned per message. The broker holds messages until delivering them stays within every limit. |
| **Lua hooks** | `on_enqueue` derives fairness key, weight, and attributes that throttles and ordering can use. `on_failure` decides retry vs. dead-letter. Both are optional. |
| **Ordering** | Off by default. A queue can ask for arrival order overall or per ordering key, with at most one message per group in flight. |
| **Retries** | A nack or an expired lease is a failed attempt. The queue's `on_failure` script decides what happens next; without one, a retry policy does — by default 3 deliveries with exponential backoff. |
| **Dead letter queue** | Every queue has one, named `<queue>.dlq`. Messages that exhaust retries move there. Redrive moves them back. |
| **Runtime config** | Key-value pairs, readable from Lua via `fila.get(key)`. Change behavior without restarting. |
| **Leases** | Delivered messages are leased for a visibility timeout. A lease that expires unacked counts as a failed attempt. |

See [docs/concepts.md](docs/concepts.md) for the model in depth and
[docs/lua-patterns.md](docs/lua-patterns.md) for hook recipes.

---

# API design

The client is Rust. One connection, from which you extract **capability handles**.

## Capabilities mirror permissions

The broker enforces three ACL kinds: `produce`, `consume`, `admin`. The client hands
out exactly three handles, named the same:

```rust
let client = FilaClient::connect("localhost:5555").await?;

client.producer()   // enqueue
client.consumer()   // subscribe, ack, nack, extend lease
client.admin()      // queues, config, redrive, API keys, ACLs
```

This is not decoration. A handle is the unit you pass into the code that needs it:
a worker gets a `Consumer`, so it cannot enqueue or delete a queue. If you needed
`.producer()`, the connection's credentials need `produce` on that queue. The API
shape teaches the permission model instead of restating it in prose.

The broker enforces permissions regardless of which handle you hold — extraction is
what makes a privileged call site visible in the code that makes it.

## Producing

The common case carries no ceremony:

```rust
let producer = client.producer();

let id = producer.enqueue("orders", b"payload").await?;
```

Everything beyond that is opt-in, on a message builder:

```rust
let id = producer.send(
    Message::new("orders", payload)
        .header("tenant", "acme")
        .fairness_key("acme")                    // direct — Lua is not required
        .weight(3)
        .delay(Duration::from_secs(30))          // deliver no earlier than
).await?;
```

`fairness_key` is a value you set, not something you reach through a script. Lua is
there for policy you can't express as a value — deriving a key from payload size,
consulting runtime config, computing a weight — and not as the price of entry to the
feature the broker exists for.

Batching is first-class, because the wire protocol is batch-native:

```rust
let ids = producer.send_batch(messages).await?;
```

## Consuming

```rust
let consumer = client.consumer();
let mut orders = consumer.subscribe("orders").await?;

while let Some(delivery) = orders.next().await {
    let delivery = delivery?;

    println!("{} attempt {}", delivery.fairness_key(), delivery.attempt());
    handle(delivery.payload())?;

    delivery.ack().await?;
}
```

A `Delivery` knows its own queue and ID, so acking does not restate them. The old
`ack(queue, message_id: &str)` made you carry two strings back to a call that
already had both.

The full set of things you can do with a delivery:

```rust
delivery.ack().await?;                                  // done
delivery.nack("downstream timeout").await?;             // failed; on_failure or retry policy decides
delivery.retry_after(Duration::from_secs(60)).await?;   // failed; retry no sooner than
delivery.extend_lease(Duration::from_secs(60)).await?;  // still working
```

`retry_after` and `extend_lease` are both load-bearing:

- **`retry_after`** sets the backoff explicitly. A client holding a `Retry-After`
  from a rate-limited upstream knows the right delay in a way the broker cannot.
  Without backoff, one failing dependency turns into a hot retry loop.
- **`extend_lease`** keeps a long job's lease alive. Otherwise any work outlasting
  the queue's visibility timeout is simply unprocessable.

### Bounding in-flight work

A subscription grants the broker **delivery credit**. The broker spends one credit
per message and stops at zero, so a slow consumer cannot be buried:

```rust
let mut orders = consumer
    .subscribe("orders")
    .prefetch(100)          // at most 100 unacked at a time
    .await?;
```

Unset means unlimited, which is right for a consumer that acks immediately. The
credit is replenished as you ack.

Flow control belongs in the protocol rather than in the socket. Throttling by
pausing TCP reads slows the connection without telling the broker anything, so it
keeps producing work with nowhere to put it.

### Throttling what you call

A worker knows which rate-limited services it calls, so it declares their limits when
it subscribes, and the broker paces delivery to stay within them:

```rust
let mut orders = consumer
    .subscribe("orders")
    .throttle(Throttle::named("stripe").rate(100, Duration::from_secs(1)))
    .throttle(
        Throttle::named("stripe-per-customer")
            .partition_by_header("customer")
            .rate(10, Duration::from_secs(1)),
    )
    .await?;
```

Declarations with the same name share one limit, across consumers and across queues.
Producers know nothing about it. See [docs/throttling.md](docs/throttling.md).

Batch acking, and more than one subscription per connection:

```rust
consumer.ack_all(&deliveries).await?;

let orders  = consumer.subscribe("orders").await?;
let billing = consumer.subscribe("billing").await?;   // concurrent, one connection
```

The protocol multiplexes on request ID, so subscriptions are independent.

## Administering

```rust
let admin = client.admin();

admin.create_queue(
    QueueSpec::new("orders")
        .visibility_timeout(Duration::from_secs(30))
        .retry(RetryPolicy::new().max_attempts(5))
        .on_enqueue(script)
        .on_failure(script)
).await?;

admin.create_queue(
    QueueSpec::new("account-events")
        .ordered_by([Key::header("account")])     // in order per account
).await?;

admin.delete_queue("orders").await?;
admin.list_queues().await?;
admin.queue_stats("orders").await?;      // depth, in-flight, per-key fairness, active throttles

admin.set_config("routing.default_region", "eu").await?;
admin.get_config("routing.default_region").await?;
admin.list_config("routing.").await?;

admin.redrive("orders.dlq", 100).await?;
```

Auth and ACLs are the same handle:

```rust
let key = admin.create_api_key(ApiKeySpec::new("ci")).await?;
// key.secret is returned exactly once — the broker stores only its hash

admin.set_acl(&key.key_id, &[
    Permission::produce("orders.*"),
    Permission::consume("orders.eu"),
]).await?;

admin.get_acl(&key.key_id).await?;
admin.revoke_api_key(&key.key_id).await?;
```

`Permission` is typed — `produce` / `consume` / `admin` — so an invalid kind is
unrepresentable rather than a string the broker rejects at runtime.

Administration belongs in the SDK. Anything reachable only by shelling out to the
CLI is unreachable from a test, a deploy script, or an operator tool.

## Errors

Every operation returns only the errors it can actually produce. There is no god
enum in which `enqueue` can fail with `MessageNotFound`.

```rust
match producer.enqueue("orders", payload).await {
    Ok(id) => ...,
    Err(EnqueueError::QueueNotFound(q)) => ...,
    Err(EnqueueError::Status(StatusError::Forbidden(_))) => ...,
    Err(EnqueueError::Status(e)) => ...,
}
```

Each type carries its own domain variants plus a shared `StatusError` for the
transport- and server-level failures common to everything. Mapping from a wire
error code to an error type is an exhaustive match, so a new code added to the
protocol fails to compile until it is handled.

## Identifiers

Message IDs are UUIDv7 — time-ordered, so they sort by insertion. They travel the
wire as 16 bytes, not as a 36-character string.

---

# Configuration design

Three layers, each with a different lifetime and a different audience.

| Layer | Set by | Changes | Readable from Lua |
|-------|--------|---------|-------------------|
| **File** — `fila.toml` | operator, at build/deploy | restart | no |
| **Environment** | orchestrator, per deployment | restart | no |
| **Runtime store** | operator or admin API, live | immediately | yes, via `fila.get(key)` |

Precedence: environment overrides file. The runtime store is a separate namespace
— it holds policy values, not boot parameters, and it is the only layer Lua can see.

## Boot configuration

`fila.toml`, read from the working directory or `/etc/fila/fila.toml`. Every setting
has a default; the broker runs with no config file at all.

```toml
[server]
listen_addr = "0.0.0.0:5555"

[storage]
data_dir = "data"

[scheduler]
quantum = 1000              # DRR deficit granted per weight unit, per round

[queue]
visibility_timeout = "30s"  # default lease duration; per-queue override at creation

[lua]
default_timeout = "10ms"
memory_limit = "1MB"

[auth]
enabled = false
bootstrap_apikey = ""       # first credential; can mint real keys, then remove

[tls]
cert_file = ""
key_file = ""
client_ca_file = ""         # set to require mTLS

[telemetry]
otlp_endpoint = ""          # empty disables export
```

Two conventions worth holding to:

- **Durations are strings with units** (`"30s"`, `"10ms"`), not bare integers with
  the unit hidden in the field name. `visibility_timeout_ms = 30000` puts the unit
  in the identifier, where it cannot be changed without renaming the field.
- **Sizes are strings with units** (`"1MB"`), for the same reason.

Every key is overridable by environment variable, upper-cased and prefixed:
`[scheduler] quantum` → `FILA_SCHEDULER_QUANTUM`.

## Runtime configuration

A flat key-value store, mutable while the broker runs, readable from Lua hooks.
This is where operational policy lives — the values you want to change at 3am
without a deploy.

```rust
admin.set_config("routing.default_region", "eu").await?;
```

```lua
function on_enqueue(msg)
  local region = fila.get("routing.default_region") or "us"
  return { fairness_key = msg.headers["tenant"] .. ":" .. region }
end
```

Namespace keys by prefix — `list_config("routing.")` returns every key under it.

---

## CLI

`fila` is a thin client over the same SDK — it has no privileged access and no
operations the SDK lacks.

```
fila queue create <name>        Create a queue
fila queue delete <name>        Delete a queue
fila queue list                 List queues
fila queue inspect <name>       Depth, in-flight, per-key fairness and throttle state

fila config set <key> <value>   Set a runtime config key
fila config get <key>           Read a runtime config key
fila config list [--prefix p]   List runtime config

fila redrive <dlq> --count N    Move messages from a DLQ back to its parent

fila auth create --name <n>     Mint an API key
fila auth revoke <key-id>       Revoke an API key
fila auth acl set <key-id> ...  Replace a key's permissions
fila auth acl get <key-id>      Show a key's permissions
```

`--addr` selects a broker (default `localhost:5555`); `--api-key` authenticates.

## Documentation

There are exactly two contracts, and they are the two things worth writing down.

| Contract | Document | Audience |
|----------|----------|----------|
| **Wire format** | [protocol.md](docs/protocol.md) | anyone implementing a client |
| **SDK surface** | rustdoc, generated from source | anyone using the Rust client |

Everything else is explanation, not contract:

| Document | What it covers |
|----------|----------------|
| [concepts.md](docs/concepts.md) | Fairness keys, DRR, throttling, leases, dead-lettering |
| [throttling.md](docs/throttling.md) | Consumer-declared rate limits, partitioning, and cluster-wide enforcement |
| [ordering.md](docs/ordering.md) | Ordered queues, ordering groups, and how order interacts with fairness and throttling |
| [configuration.md](docs/configuration.md) | The three config layers, every key, reserved prefixes |
| [lua-patterns.md](docs/lua-patterns.md) | Copy-paste `on_enqueue` and `on_failure` hooks |
| [tutorials.md](docs/tutorials.md) | Guided walkthroughs of the three core use cases |
| [sdk-examples.md](docs/sdk-examples.md) | Worked Rust examples beyond the tutorials |
| [clustering.md](docs/clustering.md) | Raft groups per queue, the meta group, placement, sharding |
| [benchmarks.md](docs/benchmarks.md) | What is measured, why, and the targets |
| [compatibility.md](docs/compatibility.md) | Versioning and compatibility policy |

There is deliberately no hand-written API reference. A binary protocol and a single
SDK need no language-neutral contract document, and a hand-maintained restatement of
a type signature only drifts from it.

## Architecture

A single-threaded scheduler core with multi-threaded I/O. The scheduler loop
processes commands from a channel and makes every scheduling decision without
locks. Protocol handlers and consumer delivery run on the async runtime's thread
pool and reach the scheduler through bounded channels.

Messages are persisted to an embedded key-value store behind a storage trait, so
the engine is a choice rather than an assumption. Crash recovery runs at startup.

The wire protocol is a hand-rolled binary protocol, specified in
[docs/protocol.md](docs/protocol.md). It is batch-native, multiplexes concurrent
requests over one connection, and is the only transport — there is no gRPC.

### The client is sans-io

The client splits in two, and this is a structural constraint rather than a
preference:

**A core with no I/O.** A state machine over bytes — feed it what arrived, ask it
what to send. It owns the codec, request-ID correlation, handshake and capability
negotiation, leader-redirect handling, delivery-credit accounting, and shard
discovery and merge. No sockets, no TLS, no async runtime, no timers it owns.

**An I/O shell.** Opens connections, does TLS, pumps bytes, and presents the host
language's native idiom.

Fila ships one client today, in Rust. The goal is several, and the reason to draw
the line here is that the two halves have opposite properties. The core is the part
that is hard to get right and identical everywhere; the shell is the part that
should look different in every language, because idiomatic is the whole point of a
native SDK.

Reimplementing the core per language is how five SDKs end up with five different
subtle bugs in credit accounting. Sharing an async client across languages fails a
different way: bridging one language's runtime into another's is worst exactly where
the value is, on long-lived server-push streams. Sans-io avoids both — the shared
part is pure functions over bytes, which every language can call, and the I/O stays
native.

The cost is honest: sans-io is harder to write than a straightforward async client,
and the Rust SDK pays it for SDKs that do not exist yet. It is worth paying only
because retrofitting it later is a rewrite, not a refactor.

## License

[AGPLv3](LICENSE)
