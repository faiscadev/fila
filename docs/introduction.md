# Fila

A message broker that makes fair scheduling and per-key throttling first-class primitives.

## The problem

Every existing broker delivers messages in FIFO order. When multiple tenants, customers, or workload types share a queue, a single noisy producer can starve everyone else. Rate limiting is pushed to the consumer — which means the consumer has to fetch a message, check the limit, and re-enqueue it. That wastes work and adds latency.

Fila moves scheduling decisions into the broker:

- **Deficit Round Robin (DRR) fair scheduling** — each fairness key gets its fair share of delivery bandwidth. No tenant starves another.
- **Token bucket throttling** — per-key rate limits enforced at the broker, before delivery. Consumers only receive messages that are ready to process.
- **Lua rules engine** — `on_enqueue` and `on_failure` hooks let you define scheduling policy (assign fairness keys, set weights, decide retry vs. dead-letter) in user-supplied Lua scripts.
- **Zero wasted work** — consumers never receive a message they can't act on.

## Quickstart

### Docker

```sh
docker run -p 5555:5555 ghcr.io/faiscadev/fila:dev
```

### Install script

```sh
curl -fsSL https://raw.githubusercontent.com/faiscadev/fila/main/install.sh -o install.sh
less install.sh
bash install.sh
fila-server
```

### Cargo

```sh
cargo install fila-server fila-cli
fila-server
```

### Try it out

Once the broker is running on `localhost:5555`:

```sh
# Create a queue
fila queue create orders

# Enqueue a message
fila enqueue orders --header tenant=acme --payload hello

# Check queue stats
fila queue inspect orders
```

