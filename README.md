# Fila

A Rust message broker with fair scheduling and per-key throttling as first-class primitives.

> **Status:** Under active development. Not production-ready.

## What makes Fila different

Every existing broker delivers messages in FIFO order with no awareness of which tenant is starved or which rate limit has been hit. Fila makes scheduling decisions at dequeue time:

- **Deficit Round Robin (DRR) fair scheduling** — tracks live fairness state across keys so no tenant starves another
- **Token bucket throttling** — per-key rate limits enforced at the broker, not the consumer
- **Lua rules engine** — `on_enqueue` and `on_failure` hooks for user-defined scheduling policy
- **Zero wasted work** — consumers only receive messages that are ready for processing

## Prerequisites

- Rust 1.75+ (stable)
- `protoc` (protobuf compiler)
- System libraries for RocksDB (usually installed automatically; on some systems you may need `libclang` and `cmake`)

## Build

```sh
cargo build
```

## Test

```sh
cargo nextest run
# or, without nextest:
cargo test
```

## License

[AGPLv3](LICENSE)
