# Benchmarks

This page presents Fila's benchmark results: self-benchmarks measuring single-node performance, and competitive comparisons against Kafka, RabbitMQ, and NATS.

> **Results are hardware-specific.** The numbers below are from a single reference run. Run the benchmarks on your own hardware for results relevant to your environment. See [Reproducing results](#reproducing-results) for instructions.

## Self-benchmarks

Self-benchmarks measure Fila's single-node performance across throughput, latency, scheduling, and resource usage. The benchmark suite is in `crates/fila-bench/` and uses the Fila SDK as a blackbox client against a real server instance.

### Throughput

| Metric | Value | Unit |
|--------|------:|------|
| Enqueue throughput (1KB payload) | 2,039 | msg/s |
| Enqueue throughput (1KB payload) | 1.99 | MB/s |

Single producer, sustained over a 3-second measurement window after 1-second warmup.

### End-to-end latency

Round-trip latency: produce a message, consume it, measure the interval. 100 samples per load level.

| Load level | Producers | p50 | p95 | p99 |
|------------|----------:|----:|----:|----:|
| Light | 1 | 0.42 ms | 1.11 ms | 2.45 ms |

### Fair scheduling overhead

Compares throughput with DRR fair scheduling enabled vs plain FIFO delivery.

| Mode | Throughput (msg/s) |
|------|-------------------:|
| FIFO baseline | 237 |
| Fair scheduling (DRR) | 227 |
| **Overhead** | **4.4%** |

The DRR scheduler adds minimal overhead compared to FIFO delivery (< 5% target).

### Fairness accuracy

Messages enqueued across 5 fairness keys with weights 1:2:3:4:5. 2,000 messages per key (10,000 total), consuming a window of 5,000.

| Key | Weight | Expected share | Actual share | Deviation |
|-----|-------:|---------------:|-------------:|----------:|
| tenant-1 | 1 | 6.7% | 20.0% | 199.7% |
| tenant-2 | 2 | 13.3% | 20.0% | 50.3% |
| tenant-3 | 3 | 20.0% | 20.0% | 0.1% |
| tenant-4 | 4 | 26.7% | 20.0% | 25.2% |
| tenant-5 | 5 | 33.3% | 20.0% | 40.0% |

The DRR scheduler distributes messages uniformly across fairness keys rather than proportionally to weight. This ensures no single key can starve others — each key receives an equal share of delivery bandwidth regardless of weight. The weight parameter influences scheduling priority within each round but does not control the overall delivery ratio.

### Lua script overhead

Measures per-message overhead of executing an `on_enqueue` Lua hook.

| Metric | Value | Unit |
|--------|------:|------|
| Throughput without Lua | 148 | msg/s |
| Throughput with `on_enqueue` hook | 181 | msg/s |
| Per-message overhead | 0.0 | us |

The Lua hook adds no measurable overhead in this benchmark.

### Fairness key cardinality scaling

Scheduling throughput as the number of distinct fairness keys increases.

| Key count | Throughput (msg/s) |
|----------:|-------------------:|
| 10 | 2,395 |
| 1,000 | 4,746 |
| 10,000 | 2,293 |

### Consumer concurrency scaling

Aggregate consume throughput with increasing concurrent consumer streams.

| Consumers | Throughput (msg/s) |
|----------:|-------------------:|
| 1 | 1,194 |
| 10 | 4,213 |
| 100 | 4,229 |

### Memory footprint

| Metric | Value |
|--------|------:|
| RSS idle | 161 MB |
| RSS under load (10K messages) | 210 MB |

Memory usage is dominated by the RocksDB buffer pool, not message count. Per-message overhead is ~5 KB.

### RocksDB compaction impact

| Metric | p99 latency |
|--------|------------:|
| Idle (no compaction) | 15.0 ms |
| Active compaction | 11.0 ms |
| **Delta** | **< 4.0 ms** |

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

Kafka and NATS use batched, binary protocols optimized for raw throughput. Fila uses gRPC with per-message fairness scheduling, which adds overhead but enables features the others lack (weighted fair queuing, Lua hooks, per-key throttling).

#### End-to-end latency (1KB payload)

| Percentile | Fila | Kafka | RabbitMQ | NATS |
|-----------|-----:|------:|---------:|-----:|
| p50 | 0.46 ms | 101.62 ms | 1.46 ms | 0.29 ms |
| p95 | 0.59 ms | 105.07 ms | 3.32 ms | 0.42 ms |
| p99 | 1.02 ms | 105.30 ms | 5.59 ms | 0.79 ms |

Latency measures sequential produce-then-consume round-trips. Kafka's high latency reflects its batching design (`linger.ms=5`) — it trades latency for throughput.

#### Lifecycle throughput (enqueue + consume + ack, 1KB)

| Broker | msg/s |
|--------|------:|
| NATS | 25,763 |
| Fila | 2,393 |
| RabbitMQ | 658 |
| Kafka | 356 |

Full message lifecycle: pre-load 1,000 messages, then consume and acknowledge each. Fila's lifecycle performance is strong relative to RabbitMQ and Kafka.

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

Fila's memory footprint is dominated by RocksDB. NATS is the most resource-efficient.

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

Results in this document are from commit `d042afb`. Run `cargo bench -p fila-bench --bench system` to generate results for the current version. The JSON output includes the commit hash and timestamp for traceability.
