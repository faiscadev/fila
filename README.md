# Fila

A message broker that makes fair scheduling and per-key throttling first-class primitives.

> **Status:** Under active development. Not production-ready.

## The problem

Every existing broker delivers messages in FIFO order. When multiple tenants, customers, or workload types share a queue, a single noisy producer can starve everyone else. Rate limiting is pushed to the consumer — which means the consumer has to fetch a message, check the limit, and re-enqueue it. That wastes work and adds latency.

Fila moves scheduling decisions into the broker:

- **Deficit Round Robin (DRR) fair scheduling** — each fairness key gets its fair share of delivery bandwidth. No tenant starves another.
- **Token bucket throttling** — per-key rate limits enforced at the broker, before delivery. Consumers only receive messages that are ready to process.
- **Lua rules engine** — `on_enqueue` and `on_failure` hooks let you define scheduling policy (assign fairness keys, set weights, decide retry vs. dead-letter) in user-supplied Lua scripts.
- **Zero wasted work** — consumers never receive a message they can't act on.

## Quickstart

### Option 1: Docker

```sh
docker run -p 5555:5555 ghcr.io/faiscadev/fila:dev
```

### Option 2: Install script

```sh
# Download, inspect, then run:
curl -fsSL https://raw.githubusercontent.com/faiscadev/fila/main/install.sh -o install.sh
less install.sh
bash install.sh
fila-server
```

### Option 3: Cargo

```sh
cargo install fila-server fila-cli
fila-server
```

### Try it out

Once the broker is running on `localhost:5555`:

```sh
# Create a queue
fila queue create orders

# Enqueue a message (via any gRPC client — grpcurl shown here)
grpcurl -plaintext -d '{
  "queue": "orders",
  "headers": {"tenant": "acme"},
  "payload": "aGVsbG8="
}' localhost:5555 fila.v1.FilaService/Enqueue

# Check queue stats
fila queue inspect orders
```

### Fair scheduling demo (under 5 minutes)

Create a queue with a Lua hook that assigns fairness keys from headers:

```sh
fila queue create fair-demo \
  --on-enqueue 'function on_enqueue(msg)
    return { fairness_key = msg.headers["tenant"] or "default" }
  end'
```

Enqueue messages from two tenants — one sends 100x more than the other:

```sh
# Noisy tenant: 100 messages
for i in $(seq 1 100); do
  grpcurl -plaintext -d "{\"queue\":\"fair-demo\",\"headers\":{\"tenant\":\"noisy\"},\"payload\":\"$(echo -n msg-$i | base64)\"}" \
    localhost:5555 fila.v1.FilaService/Enqueue
done

# Quiet tenant: 5 messages
for i in $(seq 1 5); do
  grpcurl -plaintext -d "{\"queue\":\"fair-demo\",\"headers\":{\"tenant\":\"quiet\"},\"payload\":\"$(echo -n msg-$i | base64)\"}" \
    localhost:5555 fila.v1.FilaService/Enqueue
done
```

Now consume. Fila's DRR scheduler interleaves delivery — the quiet tenant's messages aren't stuck behind all 100 of the noisy tenant's:

```sh
fila queue inspect fair-demo
```

## Key concepts

| Concept | What it does |
|---------|-------------|
| **Fairness groups** | Messages are grouped by a `fairness_key`. The DRR scheduler gives each group its fair share of delivery bandwidth. |
| **Throttling** | Token bucket rate limiters keyed by `throttle_keys`. The broker holds messages until tokens are available. |
| **Lua hooks** | `on_enqueue` assigns fairness keys, weights, and throttle keys. `on_failure` decides retry vs. dead-letter. |
| **Dead letter queue** | Messages that exhaust retries are moved to a `<queue>.dlq` queue. Use `fila redrive` to move them back. |
| **Runtime config** | Key-value pairs readable from Lua via `fila.get(key)`. Change behavior without restarting the broker. |
| **Visibility timeout** | Consumed messages are "leased" for a configurable duration. If not acked, they're re-delivered. |

See [docs/concepts.md](docs/concepts.md) for a deep dive.

## Client SDKs

| Language | Package | Repository |
|----------|---------|------------|
| Rust | `fila-sdk` | [crates/fila-sdk](crates/fila-sdk) |
| Go | `github.com/faiscadev/fila-go` | [faiscadev/fila-go](https://github.com/faiscadev/fila-go) |
| Python | `fila` | [faiscadev/fila-python](https://github.com/faiscadev/fila-python) |
| JavaScript | `@anthropic/fila` | [faiscadev/fila-js](https://github.com/faiscadev/fila-js) |
| Ruby | `fila` | [faiscadev/fila-ruby](https://github.com/faiscadev/fila-ruby) |
| Java | `dev.fila:fila-client` | [faiscadev/fila-java](https://github.com/faiscadev/fila-java) |

## CLI

The `fila` CLI manages queues, configuration, and dead-letter redrives:

```
fila queue create <name>       Create a queue
fila queue delete <name>       Delete a queue
fila queue list                List all queues
fila queue inspect <name>      Show queue stats and per-key breakdown

fila config set <key> <value>  Set a runtime config key
fila config get <key>          Get a runtime config value
fila config list [--prefix p]  List all config entries

fila redrive <dlq> --count N   Move messages from DLQ back to source queue
```

Use `--addr` to connect to a non-default broker address (default: `http://localhost:5555`).

## Configuration

Fila reads `fila.toml` from the current directory or `/etc/fila/fila.toml`. All settings have sensible defaults — the broker runs with zero configuration.

```toml
[server]
listen_addr = "0.0.0.0:5555"

[scheduler]
quantum = 1000            # DRR quantum per fairness key

[lua]
default_timeout_ms = 10   # script execution timeout
circuit_breaker_threshold = 3

[telemetry]
otlp_endpoint = "http://localhost:4317"  # optional
```

See [docs/configuration.md](docs/configuration.md) for all options.

## API

Fila exposes two gRPC services on the same port:

**Hot path** (`fila.v1.FilaService`) — for producers and consumers:
- `Enqueue` — add a message to a queue
- `Consume` — server-streaming delivery of messages
- `Ack` — acknowledge successful processing
- `Nack` — reject a message (triggers `on_failure` hook)

**Admin** (`fila.v1.FilaAdmin`) — for operators and the CLI:
- `CreateQueue`, `DeleteQueue`, `ListQueues`
- `SetConfig`, `GetConfig`, `ListConfig`
- `GetStats` — queue depth, in-flight, per-key fairness stats
- `Redrive` — move messages from DLQ back to source

See [docs/api-reference.md](docs/api-reference.md) for the full reference.

## Architecture

Fila uses a single-threaded scheduler core with multi-threaded I/O (inspired by Redis). The scheduler loop processes commands from a channel, making scheduling decisions without locks. gRPC handlers and consumer delivery run on tokio's thread pool and communicate with the scheduler through bounded channels.

Data is persisted in RocksDB with crash recovery on startup — no messages are lost.

## Building from source

### Prerequisites

- Rust 1.75+ (stable)
- `protoc` (protobuf compiler)
- System libraries for RocksDB (usually installed automatically; on some systems you may need `libclang` and `cmake`)

```sh
cargo build
cargo nextest run   # or: cargo test
```

## License

[AGPLv3](LICENSE)
