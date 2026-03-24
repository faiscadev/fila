# Competitive Benchmark Methodology

## Overview

This benchmark suite compares Fila against Kafka, RabbitMQ, and NATS on queue-oriented workloads. The goal is a fair, reproducible comparison that helps evaluators make informed adoption decisions.

## Broker Configurations

### Fila

- Runs in a Docker container (multi-stage build from repo Dockerfile, runtime base `debian:trixie-slim`)
- Default configuration (DRR scheduler, RocksDB storage)
- All brokers use identical containerisation for fair comparison

### Apache Kafka 3.9 (KRaft Mode)

- **Mode**: KRaft (combined controller + broker) — no ZooKeeper
- **Partitions**: 1 per topic (matches Fila's single-queue semantics)
- **Replication factor**: 1 (single-node benchmark)
- **Producer tuning**: `linger.ms=5`, `batch.num.messages=1000` for throughput tests; `linger.ms=0` for latency tests
- **Why KRaft**: Recommended for all new Kafka deployments since 3.3+; ZooKeeper is deprecated

### RabbitMQ 3.13

- **Queue type**: Quorum queues (`x-queue-type: quorum`) — production-recommended since 3.8+
- **Durability**: Durable queues with manual ack
- **Why quorum queues**: They are RabbitMQ's recommended queue type for data safety. Classic queues are legacy.

### NATS 2.11 (JetStream)

- **Persistence**: JetStream enabled with file-based storage
- **Consumer**: Pull-subscribe with explicit ack
- **Why JetStream**: Required for persistent messaging, message acknowledgment, and replay — features needed for queue-style workloads

## Batching & Fair Comparison

Each broker uses its recommended high-throughput configuration for **throughput** and **multi-producer** scenarios. This is how real users deploy these systems — comparing unbatched Fila against batched Kafka would be misleading.

| Broker | Throughput batching | Lifecycle batching | Rationale |
|--------|--------------------|--------------------|-----------|
| **Fila** | `BatchMode::Auto` (Nagle-style, 4 concurrent producers) | Unbatched (`BatchMode::Disabled`) | Auto-batching is the SDK default; concurrent producers trigger batch accumulation |
| **Kafka** | `linger.ms=5`, `batch.num.messages=1000` | Unbatched (`linger.ms=0`, `batch.num.messages=1`) | Standard Kafka high-throughput tuning |
| **RabbitMQ** | Per-message `basic_publish` | Per-message | RabbitMQ's AMQP protocol has no client-side batching equivalent |
| **NATS** | Per-message `publish` | Per-message | NATS JetStream has no linger-style batching |

**Why throughput uses batching:** Throughput benchmarks answer "how fast can I produce messages?" Using each broker's recommended client configuration reflects the real-world performance developers will see.

**Why lifecycle stays unbatched:** The lifecycle benchmark (enqueue→consume→ack) measures per-message round-trip latency. Batching would mask the true per-message overhead. All brokers use unbatched, serial operations for this scenario.

All results include a `batching` metadata field in the JSON output indicating the strategy used (e.g., `"auto"`, `"none"`, `"linger_ms=5"`).

## Workloads

### 1. Throughput (Producer)

Measures sustained message production rate over a fixed time window.

- **Message sizes**: 64B, 1KB, 64KB
- **Producers**: 4 concurrent producers for Fila (triggers auto-batching), 1 producer for Kafka (linger.ms handles batching), 1 for RabbitMQ/NATS
- **Batching**: Each broker uses its recommended batching (see table above)
- **Warmup**: 5 seconds (discarded)
- **Measurement window**: 30 seconds (configurable via `FILA_BENCH_DURATION_SECS`)
- **Metric**: messages/second

### 2. End-to-End Latency

Measures round-trip time: produce a message, consume it, measure the interval.

- **Message size**: 1KB
- **Batching**: Unbatched for all brokers (latency-focused)
- **Warmup**: 5 seconds (discarded)
- **Measurement window**: 30 seconds
- **Metrics**: p50, p95, p99, p99.9, p99.99, max (milliseconds)
- **Method**: Concurrent producer/consumer with embedded nanosecond timestamps

### 3. Multi-Producer Throughput

Measures sustained aggregate production rate from multiple concurrent producers.

- **Producers**: 3 concurrent producers (threads for Kafka, async tasks for others)
- **Message size**: 1KB
- **Batching**: Each broker uses its recommended batching (see table above)
- **Measurement window**: 30 seconds
- **Metric**: aggregate messages/second across all producers

### 4. Lifecycle Throughput (Produce → Consume → Ack)

Measures the full message lifecycle: pre-load 1,000 messages, then consume and acknowledge each one.

- **Message size**: 1KB
- **Batching**: Unbatched for all brokers — serial enqueue→consume→ack
- **Messages**: 1,000
- **Metric**: messages/second for the consume+ack phase

### 5. Resource Utilization

Captures CPU and memory usage of each broker's Docker container after benchmarks.

- **Method**: `docker stats --no-stream` after workload completes
- **Metrics**: CPU percentage, memory (MB)
- **Disk I/O**: Not captured via `docker stats`. For disk I/O analysis, use `docker stats` with `--format` including BlockIO, or external monitoring tools (e.g., `iostat`). The current suite focuses on CPU and memory as the primary resource indicators.

## Measurement Notes

### Warmup

All throughput benchmarks include a warmup period (default: 1 second) where messages are produced but not counted. This ensures the broker is in a steady state before measurement begins.

### Multiple Runs

For CI regression detection, the benchmark suite runs 3 times and uses the median. For competitive benchmarks, a single run is typically sufficient since the focus is relative comparison (all brokers experience the same CI environment variance).

### Rust Clients for All Brokers

All benchmarks — including competitors — use native Rust client libraries:
- **Kafka**: `rdkafka` (librdkafka bindings, the standard high-performance Kafka client)
- **RabbitMQ**: `lapin` (async AMQP 0-9-1 client)
- **NATS**: `async-nats` (official NATS Rust client)
- **Fila**: `fila-sdk` (native gRPC client)

This ensures the benchmark measures broker performance, not client language overhead. All clients run in the same Rust async runtime with equivalent optimization levels.

### Hardware

Results are hardware-specific. When publishing results, always include:

- CPU model and core count
- Memory (total and available)
- Disk type (SSD/NVMe/HDD)
- OS and kernel version
- Docker version

The `bench-competitive` binary records the git commit hash and timestamp for traceability.

## Limitations

- **Single-node only**: All brokers run as single instances. Clustering performance is not tested.
- **No network latency**: Brokers run on localhost. Real-world deployments have network overhead.
- **Client library maturity**: Different Rust client libraries may have varying levels of optimization (e.g., rdkafka wraps C librdkafka; lapin is pure Rust).
- **Configuration sensitivity**: Results depend on broker configuration. We use production-recommended defaults but not every possible tuning option.
- **Docker containers**: All brokers run in Docker. Docker adds ~1-3% overhead for I/O-intensive workloads compared to native execution.

## Reproducing Results

```bash
# Prerequisites: Docker, Rust toolchain

# Run everything
cd bench/competitive
make bench-competitive

# Or run individual brokers
make bench-kafka
make bench-rabbitmq
make bench-nats
make bench-fila

# Clean up
make bench-clean
```

Results are written to `bench/competitive/results/bench-{broker}.json`.
