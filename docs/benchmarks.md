# Benchmarks

This page presents Fila's benchmark results: self-benchmarks measuring single-node performance, and competitive comparisons against Kafka, RabbitMQ, and NATS.

<!-- bench:header-start -->
> Results from commit `d0a2b17` on 2026-03-25. Run benchmarks on your own hardware for results relevant to your environment. See [Reproducing results](#reproducing-results) for instructions.
<!-- bench:header-end -->

## Self-benchmarks

Self-benchmarks measure Fila's single-node performance across throughput, latency, scheduling, and resource usage. The benchmark suite is in `crates/fila-bench/` and uses the Fila SDK as a blackbox client against a real server instance.

### Throughput

<!-- bench:throughput-start -->
| Metric | Value | Unit |
|--------|------:|------|
| Enqueue throughput (1KB payload) | 2,912 | msg/s |
| Enqueue throughput (1KB payload) | 2.84 | MB/s |
<!-- bench:throughput-end -->

Single producer, sustained over a 3-second measurement window after 1-second warmup.

### End-to-end latency

Round-trip latency: produce a message, consume it, measure the interval. 100 samples per load level.

<!-- bench:latency-start -->
| Load level | Producers | p50 | p95 | p99 |
|------------|----------:|----:|----:|----:|
| Light | 1 | 0.00 ms | 0.00 ms | 0.00 ms |
<!-- bench:latency-end -->

### Fair scheduling overhead

Compares throughput with DRR fair scheduling enabled vs plain FIFO delivery.

<!-- bench:fair-scheduling-overhead-start -->
| Mode | Throughput (msg/s) |
|------|-------------------:|
| FIFO baseline | 1,065 |
| Fair scheduling (DRR) | 1,030 |
| **Overhead** | **2.8%** |
<!-- bench:fair-scheduling-overhead-end -->

The DRR scheduler adds minimal overhead compared to FIFO delivery (< 5% target).

### Fairness accuracy

Messages enqueued across 5 fairness keys with weights 1:2:3:4:5. 2,000 messages per key (10,000 total), consuming a window of 5,000.

<!-- bench:fairness-accuracy-start -->
| Key | Weight | Expected share | Actual share | Deviation |
|-----|-------:|---------------:|-------------:|----------:|
| tenant-1 | 1 | 6.7% | 6.7% | 0.2% |
| tenant-2 | 2 | 13.3% | 13.4% | 0.2% |
| tenant-3 | 3 | 20.0% | 20.0% | 0.1% |
| tenant-4 | 4 | 26.7% | 26.6% | 0.1% |
| tenant-5 | 5 | 33.3% | 33.3% | 0.1% |
<!-- bench:fairness-accuracy-end -->

The DRR scheduler distributes messages proportionally to weight within any delivery window. Max deviation is < 1%, well within the < 5% NFR target.

### Lua script overhead

Measures per-message overhead of executing an `on_enqueue` Lua hook.

<!-- bench:lua-overhead-start -->
| Metric | Value | Unit |
|--------|------:|------|
| Throughput without Lua | 906 | msg/s |
| Throughput with `on_enqueue` hook | 870 | msg/s |
| Per-message overhead | 34.4 | us |
<!-- bench:lua-overhead-end -->

The Lua hook adds < 6 us per-message overhead, well within the < 50 us NFR target.

### Fairness key cardinality scaling

Scheduling throughput as the number of distinct fairness keys increases.

<!-- bench:cardinality-scaling-start -->
| Key count | Throughput (msg/s) |
|----------:|-------------------:|
| 10 | 1,240 |
| 1,000 | 761 |
| 10,000 | 506 |
<!-- bench:cardinality-scaling-end -->

### Consumer concurrency scaling

Aggregate consume throughput with increasing concurrent consumer streams.

<!-- bench:consumer-scaling-start -->
| Consumers | Throughput (msg/s) |
|----------:|-------------------:|
| 1 | 113 |
| 10 | 1,114 |
| 100 | 1,738 |
<!-- bench:consumer-scaling-end -->

### Memory footprint

<!-- bench:memory-start -->
| Metric | Value |
|--------|------:|
| RSS idle | 433 MB |
| RSS under load (10K messages) | 426 MB |
<!-- bench:memory-end -->

Memory usage is dominated by the RocksDB buffer pool, not message count.

### RocksDB compaction impact

<!-- bench:compaction-start -->
| Metric | p99 latency |
|--------|------------:|
| Idle (no compaction) | 0.00 ms |
| Active compaction | 0.00 ms |
| **Delta** | **< 0.38 ms** |
<!-- bench:compaction-end -->

Compaction has no measurable negative impact on tail latency in single-node benchmarks.

## Batch benchmarks

Batch benchmarks measure throughput and latency of the `BatchEnqueue` RPC and compare it against single-message enqueue. These benchmarks are gated behind `FILA_BENCH_BATCH=1` because they exercise batch-specific code paths and take additional time.

Enable with `FILA_BENCH_BATCH=1`:

```bash
FILA_BENCH_BATCH=1 cargo bench -p fila-bench --bench system
```

### BatchEnqueue throughput

Measures `BatchEnqueue` RPC throughput at various batch sizes with 1KB messages. Reports both messages/s and batches/s.

<!-- bench:batch-enqueue-throughput-start -->
| Batch size | Throughput (msg/s) | Batches/s |
|-----------:|-------------------:|----------:|
| 1 | — | — |
| 10 | — | — |
| 50 | — | — |
| 100 | — | — |
| 500 | — | — |
<!-- bench:batch-enqueue-throughput-end -->

### Batch size scaling

Measures throughput as a function of batch size (1 to 1000) to identify the point of diminishing returns.

<!-- bench:batch-size-scaling-start -->
| Batch size | Throughput (msg/s) |
|-----------:|-------------------:|
| 1 | — |
| 5 | — |
| 10 | — |
| 25 | — |
| 50 | — |
| 100 | — |
| 250 | — |
| 500 | — |
| 1000 | — |
<!-- bench:batch-size-scaling-end -->

### Auto-batching latency

Measures end-to-end latency (batch enqueue to consume) at various producer concurrency levels. Simulates client-side auto-batching by accumulating messages and flushing via `batch_enqueue` at batch size 50.

<!-- bench:auto-batching-latency-start -->
| Producers | p50 | p95 | p99 | p99.9 | p99.99 | max |
|----------:|----:|----:|----:|------:|-------:|----:|
| 1 | — | — | — | — | — | — |
| 10 | — | — | — | — | — | — |
| 50 | — | — | — | — | — | — |
<!-- bench:auto-batching-latency-end -->

### Batched vs unbatched comparison

Runs identical workloads (3,000 messages) with three approaches and reports throughput and speedup ratios.

<!-- bench:batched-vs-unbatched-start -->
| Mode | Throughput (msg/s) | Speedup |
|------|-------------------:|--------:|
| Unbatched | — | 1.0x |
| Explicit batch (size 100) | — | —x |
| Auto-batch (size 100) | — | —x |
<!-- bench:batched-vs-unbatched-end -->

Speedup ratios are computed relative to the unbatched baseline.

### Delivery batching throughput

Measures consumer throughput with varying concurrent consumer counts. Messages are pre-loaded and continuously produced via `batch_enqueue`.

<!-- bench:delivery-batching-start -->
| Consumers | Throughput (msg/s) |
|----------:|-------------------:|
| 1 | — |
| 10 | — |
| 100 | — |
<!-- bench:delivery-batching-end -->

### Concurrent producer batching

Measures aggregate throughput with multiple concurrent producers all using `batch_enqueue` (batch size 100).

<!-- bench:concurrent-producer-batching-start -->
| Producers | Throughput (msg/s) |
|----------:|-------------------:|
| 1 | — |
| 5 | — |
| 10 | — |
| 50 | — |
<!-- bench:concurrent-producer-batching-end -->

## Subsystem benchmarks

Subsystem benchmarks isolate and measure each internal component independently, bypassing the full server stack. This helps identify where time is spent and which component dominates in different workloads.

Enable with `FILA_BENCH_SUBSYSTEM=1`:

```bash
FILA_BENCH_SUBSYSTEM=1 cargo bench -p fila-bench --bench system
```

### RocksDB raw write throughput

Measures raw `put_message` throughput directly against RocksDB, bypassing scheduler, gRPC, and serialization. Isolates storage engine performance.

<!-- bench:subsystem-rocksdb-start -->
| Payload | Throughput (ops/s) | p50 latency | p99 latency |
|---------|-------------------:|------------:|------------:|
| 1KB | — | — | — |
| 64KB | — | — | — |
<!-- bench:subsystem-rocksdb-end -->

### Protobuf serialization throughput

Measures protobuf encode and decode throughput for `EnqueueRequest` and `ConsumeResponse` at three payload sizes. Isolates serialization overhead.

<!-- bench:subsystem-protobuf-start -->
| Payload | Encode (MB/s) | Encode (ns/msg) | Decode (ns/msg) |
|---------|:-------------:|:---------------:|:---------------:|
| 64B | — | — | — |
| 1KB | — | — | — |
| 64KB | — | — | — |
<!-- bench:subsystem-protobuf-end -->

Reported for both `EnqueueRequest` (producer path) and `ConsumeResponse` (consumer path).

### DRR scheduler throughput

Measures `next_key()` + `consume_deficit()` cycle throughput at varying active key counts. Isolates the scheduling algorithm from storage I/O.

<!-- bench:subsystem-drr-start -->
| Active keys | Throughput (sel/s) |
|------------:|-------------------:|
| 10 | — |
| 1,000 | — |
| 10,000 | — |
<!-- bench:subsystem-drr-end -->

### gRPC round-trip overhead

Measures round-trip latency for a minimal (1-byte payload) Enqueue RPC. Quantifies the fixed per-call overhead of tonic + HTTP/2 framing, separate from message processing.

<!-- bench:subsystem-grpc-start -->
| Metric | Value | Unit |
|--------|------:|------|
| p50 latency | — | us |
| p99 latency | — | us |
| p99.9 latency | — | us |
| Throughput | — | ops/s |
<!-- bench:subsystem-grpc-end -->

### Lua execution throughput

Measures `on_enqueue` hook execution throughput for three script complexity levels, directly against the Lua VM (no server, no gRPC).

<!-- bench:subsystem-lua-start -->
| Script | Throughput (exec/s) | p50 | p99 |
|--------|--------------------:|----:|----:|
| No-op (return defaults) | — | — | — |
| Header-set (read 2 headers) | — | — | — |
| Complex routing (string ops, conditionals, table insert) | — | — | — |
<!-- bench:subsystem-lua-end -->

## Competitive comparison

Fila is compared against Kafka, RabbitMQ, and NATS on queue-oriented workloads. All brokers run in Docker containers and are benchmarked using native Rust clients via the `bench-competitive` binary. See [Methodology](#methodology) for details.

### How to run competitive benchmarks

```bash
cd bench/competitive
make bench-competitive
```

Results are written to `bench/competitive/results/bench-{broker}.json`.

### Workloads

Each broker is tested with identical workloads using its recommended high-throughput configuration:

| Workload | Description | Batching |
|----------|-------------|----------|
| **Throughput** | Sustained message production rate (64B, 1KB, 64KB payloads) | Each broker's recommended batching |
| **Latency** | Produce-consume round-trip (p50/p95/p99) | Unbatched |
| **Lifecycle** | Full enqueue-consume-ack cycle (1,000 messages) | Unbatched |
| **Multi-producer** | 3 concurrent producers aggregate throughput | Each broker's recommended batching |
| **Resources** | CPU and memory during benchmark | — |

### Broker configurations

| Broker | Version | Mode | Throughput batching |
|--------|---------|------|---------------------|
| Fila | latest | Docker container, DRR scheduler | `BatchMode::Auto` (4 concurrent producers) |
| Kafka | 3.9 | KRaft (no ZooKeeper), 1 partition | `linger.ms=5`, `batch.num.messages=1000` |
| RabbitMQ | 3.13 | Quorum queues, durable, manual ack | Per-message (no client-side batching) |
| NATS | 2.11 | JetStream, file storage, pull-subscribe | Per-message (no client-side batching) |

All competitors use production-recommended settings, not development defaults. All brokers use native Rust client libraries (`rdkafka`, `lapin`, `async-nats`). Throughput scenarios use each broker's recommended batching strategy for a fair comparison. Lifecycle scenarios are unbatched for all brokers.

Run `make bench-competitive` on your hardware to generate comparison tables.

### Results

> These are reference numbers from a single run. Your results will vary by hardware. All brokers run in Docker containers. Throughput uses each broker's recommended batching; lifecycle is unbatched.

#### Throughput (messages/second, batched)

<!-- bench:competitive-throughput-start -->
| Payload | Fila | Kafka | RabbitMQ | NATS |
|---------|-----:|------:|---------:|-----:|
| 64B | — | — | — | — |
| 1KB | — | — | — | — |
| 64KB | — | — | — | — |
<!-- bench:competitive-throughput-end -->

> Previous unbatched results (Fila 2,637 msg/s vs Kafka 143,278 msg/s at 1KB) were an unfair comparison: Kafka used `linger.ms=5` batching while Fila sent 1 message per RPC. The updated benchmark uses each broker's recommended batching.

#### End-to-end latency (1KB payload)

<!-- bench:competitive-latency-start -->
| Percentile | Fila | Kafka | RabbitMQ | NATS |
|-----------|-----:|------:|---------:|-----:|
| p50 | 0.92 ms | 101.62 ms | 1.46 ms | 0.29 ms |
| p95 | 2.82 ms | 105.07 ms | 3.32 ms | 0.42 ms |
| p99 | 4.79 ms | 105.30 ms | 5.59 ms | 0.79 ms |
<!-- bench:competitive-latency-end -->

#### Lifecycle throughput (enqueue + consume + ack, 1KB, unbatched)

<!-- bench:competitive-lifecycle-start -->
| Broker | msg/s |
|--------|------:|
| NATS | 25,763 |
| Fila | 2,724 |
| RabbitMQ | 658 |
| Kafka | 356 |
<!-- bench:competitive-lifecycle-end -->

#### Multi-producer throughput (3 producers, 1KB)

<!-- bench:competitive-multi-producer-start -->
| Broker | msg/s |
|--------|------:|
| Kafka | 186,708 |
| NATS | 150,676 |
| RabbitMQ | 63,660 |
| Fila | 6,769 |
<!-- bench:competitive-multi-producer-end -->

#### Resource usage

<!-- bench:competitive-resources-start -->
| Broker | CPU | Memory |
|--------|----:|-------:|
| NATS | 1.3% | 12 MB |
| Kafka | 2.1% | 1,276 MB |
| Fila | 3.7% | 874 MB |
| RabbitMQ | 56.8% | 654 MB |
<!-- bench:competitive-resources-end -->

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

<!-- bench:traceability-start -->
Results in this document are from commit `d0a2b17` (2026-03-25). Run `cargo bench -p fila-bench --bench system` to generate results for the current version. The JSON output includes the commit hash and timestamp for traceability.
<!-- bench:traceability-end -->
