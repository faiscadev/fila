# Benchmarks

This page presents Fila's benchmark results: self-benchmarks measuring single-node performance, and competitive comparisons against Kafka, RabbitMQ, and NATS.

> **Results are hardware-specific.** The numbers below are from a single reference run. Run the benchmarks on your own hardware for results relevant to your environment. See [Reproducing results](#reproducing-results) for instructions.

## Self-benchmarks

Self-benchmarks measure Fila's single-node performance across throughput, latency, scheduling, and resource usage. The benchmark suite is in `crates/fila-bench/` and uses the Fila SDK as a blackbox client against a real server instance.

### Throughput

| Metric | Value | Unit |
|--------|------:|------|
| Enqueue throughput (1KB payload) | 5,348 | msg/s |
| Enqueue throughput (1KB payload) | 5.22 | MB/s |

Single producer, sustained over a 3-second measurement window after 1-second warmup.

### End-to-end latency

Round-trip latency: produce a message, consume it, measure the interval. 100 samples per load level.

| Load level | Producers | p50 | p95 | p99 |
|------------|----------:|----:|----:|----:|
| Light | 1 | 0.22 ms | 0.33 ms | 0.50 ms |

### Fair scheduling overhead

Compares throughput with DRR fair scheduling enabled vs plain FIFO delivery.

| Mode | Throughput (msg/s) |
|------|-------------------:|
| FIFO baseline | 2,140 |
| Fair scheduling (DRR) | 2,114 |
| **Overhead** | **1.2%** |

The DRR scheduler adds minimal overhead compared to FIFO delivery (< 5% target).

### Fairness accuracy

Messages enqueued across 5 fairness keys with weights 1:2:3:4:5. 2,000 messages per key (10,000 total), consuming a window of 5,000.

| Key | Weight | Expected share | Actual share | Deviation |
|-----|-------:|---------------:|-------------:|----------:|
| tenant-1 | 1 | 6.7% | 6.7% | 0.2% |
| tenant-2 | 2 | 13.3% | 13.4% | 0.2% |
| tenant-3 | 3 | 20.0% | 20.0% | 0.1% |
| tenant-4 | 4 | 26.7% | 26.6% | 0.1% |
| tenant-5 | 5 | 33.3% | 33.3% | 0.1% |

The DRR scheduler distributes messages proportionally to weight within any delivery window. Max deviation is < 1%, well within the < 5% NFR target.

### Lua script overhead

Measures per-message overhead of executing an `on_enqueue` Lua hook.

| Metric | Value | Unit |
|--------|------:|------|
| Throughput without Lua | 1,734 | msg/s |
| Throughput with `on_enqueue` hook | 1,717 | msg/s |
| Per-message overhead | 5.9 | us |

The Lua hook adds < 6 us per-message overhead, well within the < 50 us NFR target.

### Fairness key cardinality scaling

Scheduling throughput as the number of distinct fairness keys increases.

| Key count | Throughput (msg/s) |
|----------:|-------------------:|
| 10 | 4,943 |
| 1,000 | 4,124 |
| 10,000 | 2,053 |

### Consumer concurrency scaling

Aggregate consume throughput with increasing concurrent consumer streams.

| Consumers | Throughput (msg/s) |
|----------:|-------------------:|
| 1 | 1,012 |
| 10 | 3,628 |
| 100 | 3,544 |

### Memory footprint

| Metric | Value |
|--------|------:|
| RSS idle | 239 MB |
| RSS under load (10K messages) | 232 MB |

Memory usage is dominated by the RocksDB buffer pool, not message count.

### RocksDB compaction impact

| Metric | p99 latency |
|--------|------------:|
| Idle (no compaction) | 0.32 ms |
| Active compaction | 0.29 ms |
| **Delta** | **< 0.03 ms** |

Compaction has no measurable negative impact on tail latency in single-node benchmarks.

## Competitive comparison

Fila is compared against Kafka, RabbitMQ, and NATS on queue-oriented workloads. All brokers run in Docker containers and are benchmarked using native Rust clients via the `bench-competitive` binary. See [Methodology](#methodology) for details.

### How to run competitive benchmarks

```bash
cd bench/competitive
make bench-competitive
```

Results are written to `bench/competitive/results/bench-{broker}.json`.

### Workloads

Each broker is tested with identical workloads:

| Workload | Description |
|----------|-------------|
| **Throughput** | Sustained message production rate (64B, 1KB, 64KB payloads) |
| **Latency** | Produce-consume round-trip (p50/p95/p99) |
| **Lifecycle** | Full enqueue-consume-ack cycle (1,000 messages) |
| **Multi-producer** | 3 concurrent producers aggregate throughput |
| **Resources** | CPU and memory during benchmark |

### Broker configurations

| Broker | Version | Mode | Key settings |
|--------|---------|------|-------------|
| Fila | (see commit hash) | Docker container | DRR scheduler, RocksDB storage |
| Kafka | 3.9 | KRaft (no ZooKeeper) | 1 partition, `linger.ms=5`, `batch.num.messages=1000` |
| RabbitMQ | 3.13 | Quorum queues | Durable, manual ack |
| NATS | 2.11 | JetStream | File storage, pull-subscribe, explicit ack |

All competitors use production-recommended settings, not development defaults. All brokers use native Rust client libraries (`rdkafka`, `lapin`, `async-nats`).

Run `make bench-competitive` on your hardware to generate comparison tables.

### Results

> These are reference numbers from a single run. Your results will vary by hardware. All brokers run in Docker containers.

#### Throughput (messages/second)

| Payload | Fila | Kafka | RabbitMQ | NATS |
|---------|-----:|------:|---------:|-----:|
| 64B | 2,758 | 1,473,379 | 36,141 | 394,950 |
| 1KB | 2,326 | 143,278 | 38,321 | 137,748 |
| 64KB | 296 | 2,335 | 2,379 | 2,426 |

#### End-to-end latency (1KB payload)

| Percentile | Fila | Kafka | RabbitMQ | NATS |
|-----------|-----:|------:|---------:|-----:|
| p50 | 0.46 ms | 101.62 ms | 1.46 ms | 0.29 ms |
| p95 | 0.59 ms | 105.07 ms | 3.32 ms | 0.42 ms |
| p99 | 1.02 ms | 105.30 ms | 5.59 ms | 0.79 ms |

#### Lifecycle throughput (enqueue + consume + ack, 1KB)

| Broker | msg/s |
|--------|------:|
| NATS | 25,763 |
| Fila | 2,393 |
| RabbitMQ | 658 |
| Kafka | 356 |

#### Multi-producer throughput (3 producers, 1KB)

| Broker | msg/s |
|--------|------:|
| Kafka | 186,708 |
| NATS | 150,676 |
| RabbitMQ | 63,660 |
| Fila | 6,343 |

#### Resource usage

| Broker | CPU | Memory |
|--------|----:|-------:|
| NATS | 1.3% | 12 MB |
| Kafka | 2.1% | 1,276 MB |
| Fila | 23.1% | 107 MB |
| RabbitMQ | 56.8% | 654 MB |

## Methodology

### Measurement parameters

| Parameter | Value |
|-----------|-------|
| Warmup period | 1 second (discarded) |
| Measurement window | 3 seconds |
| Latency samples | 100 per level |
| Runs for CI regression | 3 (median) |
| Competitive runs | 1 (relative comparison) |

### Limitations

- **Single-node only.** All brokers run as single instances. Clustering performance is not tested.
- **No network latency.** Brokers run on localhost. Real deployments have network overhead.
- **Docker containers.** All brokers run in Docker containers for a fair comparison.
- **Hardware-specific.** Results will vary on different hardware. Always include hardware specs when citing numbers.

### Reproducing results

**Self-benchmarks:**

```bash
# Build and run the full benchmark suite
cargo bench -p fila-bench --bench system

# Results written to crates/fila-bench/bench-results.json
```

**Competitive benchmarks:**

```bash
cd bench/competitive

# Run all brokers
make bench-competitive

# Or individual brokers
make bench-kafka
make bench-rabbitmq
make bench-nats
make bench-fila

# Clean up Docker containers
make bench-clean
```

See [`bench/competitive/METHODOLOGY.md`](../bench/competitive/METHODOLOGY.md) for complete methodology documentation including broker configuration details and justifications.

### CI regression detection

The `bench-regression` GitHub Actions workflow runs on every push to `main` and on pull requests:

- Runs the self-benchmark suite 3 times, takes the median
- On `main` pushes: saves results as the baseline
- On PRs: compares against the baseline and flags regressions exceeding the threshold (default: 10%)
- Results are uploaded as workflow artifacts for every run

## Traceability

Results in this document are from commit `4535e4a`. Run `cargo bench -p fila-bench --bench system` to generate results for the current version. The JSON output includes the commit hash and timestamp for traceability.
