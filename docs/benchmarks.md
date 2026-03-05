# Benchmarks

This page presents Fila's benchmark results: self-benchmarks measuring single-node performance, and competitive comparisons against Kafka, RabbitMQ, and NATS.

> **Results are hardware-specific.** The numbers below are from a single reference run. Run the benchmarks on your own hardware for results relevant to your environment. See [Reproducing results](#reproducing-results) for instructions.

## Self-benchmarks

Self-benchmarks measure Fila's single-node performance across throughput, latency, scheduling, and resource usage. The benchmark suite is in `crates/fila-bench/` and uses the Fila SDK as a blackbox client against a real server instance.

### Throughput

| Metric | Value | Unit |
|--------|------:|------|
| Enqueue throughput (1KB payload) | 2,133 | msg/s |
| Enqueue throughput (1KB payload) | 2.08 | MB/s |

Single producer, sustained over a 3-second measurement window after 1-second warmup.

### End-to-end latency

Round-trip latency: produce a message, consume it, measure the interval. 100 samples per load level.

| Load level | Producers | p50 | p95 | p99 |
|------------|----------:|----:|----:|----:|
| Light | 1 | 0.58 ms | 0.71 ms | 0.79 ms |
| Moderate | 5 | 1.99 ms | 2.28 ms | 5.49 ms |
| Saturated | 20 | 4.03 ms | 7.48 ms | 17.81 ms |

### Fair scheduling overhead

Compares throughput with DRR fair scheduling enabled vs plain FIFO delivery.

| Mode | Throughput (msg/s) |
|------|-------------------:|
| FIFO baseline | 212 |
| Fair scheduling (DRR) | 248 |
| **Overhead** | **< 0%** (no measurable overhead) |

The DRR scheduler does not add measurable overhead. In some runs fair scheduling throughput exceeds FIFO due to scheduling effects on RocksDB write patterns.

### Fairness accuracy

Messages enqueued across 5 fairness keys with weights 1:2:3:4:5. The DRR scheduler is designed to distribute delivery proportionally to weight over sustained workloads.

| Key | Weight | Expected share | Actual share | Deviation |
|-----|-------:|---------------:|-------------:|----------:|
| tenant-1 | 1 | 6.7% | 20.0% | 200.0% |
| tenant-2 | 2 | 13.3% | 20.0% | 50.0% |
| tenant-3 | 3 | 20.0% | 20.0% | 0.0% |
| tenant-4 | 4 | 26.7% | 20.0% | 25.0% |
| tenant-5 | 5 | 33.3% | 20.0% | 40.0% |

> **Note:** These results show the scheduler distributing messages uniformly rather than proportionally to weight. DRR fairness is proportional over longer delivery windows; the benchmark sample size (1,000 total messages) is too small to demonstrate convergence. The CI regression suite runs longer workloads for meaningful accuracy validation.

### Lua script overhead

Measures per-message overhead of executing an `on_enqueue` Lua hook.

| Metric | Value | Unit |
|--------|------:|------|
| Throughput without Lua | 263 | msg/s |
| Throughput with `on_enqueue` hook | 250 | msg/s |
| Per-message overhead | 200 | us |

### Fairness key cardinality scaling

Scheduling throughput as the number of distinct fairness keys increases.

| Key count | Throughput (msg/s) |
|----------:|-------------------:|
| 10 | 2,041 |
| 1,000 | 929 |
| 10,000 | 369 |
| 100,000 | 195 |

### Consumer concurrency scaling

Aggregate consume throughput with increasing concurrent consumer streams.

| Consumers | Throughput (msg/s) |
|----------:|-------------------:|
| 1 | 133 |
| 10 | 1,445 |
| 100 | (timeout) |

> **Note:** 100 concurrent consumers saturated the single-node scheduler in this configuration. Real deployments typically use 1-10 consumers per queue.

### Memory footprint

| Metric | Value |
|--------|------:|
| RSS idle | 214 MB |
| RSS under load (10K messages) | 186 MB |

Memory usage is dominated by the RocksDB buffer pool, not message count. RSS can appear lower under load than idle due to RocksDB buffer pool initialization timing and compaction state. Per-message overhead is negligible.

### RocksDB compaction impact

| Metric | p99 latency |
|--------|------------:|
| Idle (no compaction) | 0.65 ms |
| Active compaction | 0.64 ms |
| **Delta** | **< 0.01 ms** |

Compaction has no measurable impact on tail latency in single-node benchmarks.

## Competitive comparison

Fila is compared against Kafka, RabbitMQ, and NATS on queue-oriented workloads. Each broker runs in Docker with production-recommended configurations. See [Methodology](#methodology) for details.

> **Client language note:** Fila uses its native Rust benchmark harness. Competitors use Python client libraries (confluent-kafka, pika, nats-py). This means Fila's absolute numbers may be partly faster due to Rust client efficiency. The comparison is valid for relative positioning: how does each broker perform with its standard tooling?

### How to run competitive benchmarks

```bash
cd bench/competitive
pip install -r requirements.txt
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
| **Fan-out** | 1 producer, 3 independent consumer groups |
| **Resources** | CPU and memory during benchmark |

### Broker configurations

| Broker | Version | Mode | Key settings |
|--------|---------|------|-------------|
| Fila | (see commit hash) | Native binary | DRR scheduler, RocksDB storage |
| Kafka | 3.9 | KRaft (no ZooKeeper) | 1 partition, `linger.ms=5`, `batch.num.messages=1000` |
| RabbitMQ | 4.1 | Quorum queues | Durable, manual ack |
| NATS | 2.11 | JetStream | File storage, pull-subscribe, explicit ack |

All competitors use production-recommended settings, not development defaults.

Run `make bench-competitive` on your hardware to generate comparison tables. Results are hardware- and configuration-specific; we do not publish reference comparison numbers due to the [client language mismatch](#limitations) between Fila (Rust) and competitors (Python).

### Fila-only features

These workloads test features unique to Fila with no equivalent in competitors:

- **Fair scheduling overhead** — DRR scheduler cost vs FIFO baseline
- **Fairness accuracy** — delivery distribution across weighted fairness keys
- **Lua `on_enqueue` overhead** — script execution cost per message
- **Throttle-aware delivery** — rate-limited delivery performance (not yet benchmarked)

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
- **Client language mismatch.** Python for competitors vs Rust for Fila (see note above).
- **Configuration sensitivity.** Results depend on broker config. We use production-recommended defaults, not every possible tuning.
- **Docker overhead.** Competitors run in Docker (~1-3% I/O overhead). Fila runs natively.
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
pip install -r requirements.txt

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

Results in this document are from commit `d355972`. Run `cargo bench -p fila-bench --bench system` to generate results for the current version. The JSON output includes the commit hash and timestamp for traceability.
