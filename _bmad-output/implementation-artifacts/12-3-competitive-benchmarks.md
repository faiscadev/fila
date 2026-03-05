# Story 12.3: Competitive Benchmarks

Status: in-progress

## Story

As an evaluator,
I want to compare Fila's benchmark results against Kafka, RabbitMQ, and NATS for queue workloads,
so that I can make informed adoption decisions based on data.

## Acceptance Criteria

1. **Given** Docker Compose configurations exist for each competitor (Kafka, RabbitMQ, NATS), **when** the competitive benchmark suite runs, **then** each broker is tested with identical workloads.

2. **Given** the benchmark suite runs, **when** workloads complete, **then** results include: single-producer/single-consumer throughput, fan-out (1 producer / N consumers), multi-producer, and varying message sizes (64B, 1KB, 64KB).

3. **Given** queue-specific workloads, **when** tested, **then** results include: enqueue → consume → ack lifecycle throughput, visibility timeout / redelivery overhead.

4. **Given** Fila-only workloads, **when** tested, **then** results include: fair scheduling overhead, throttle-aware delivery (noted as no equivalent in competitors).

5. **Given** latency workloads, **when** tested, **then** latency is measured at p50/p95/p99 for each broker under identical load.

6. **Given** results exist, **when** published, **then** methodology is documented: hardware specs, broker configuration, warmup period, measurement window, number of runs.

7. **Given** the full suite, **when** run, **then** results are reproducible via a single make target (e.g., `make bench-competitive`).

8. **Given** competitor configurations, **when** used, **then** they use recommended production settings (not default development settings).

9. **Given** the benchmark runs, **when** results are collected, **then** resource utilization (CPU, memory, disk I/O) is included per broker during the benchmark.

## Tasks / Subtasks

- [x] Task 1: Docker Compose configurations for competitors (AC: 1, 8)
  - [x] Create `bench/competitive/docker-compose.yml` with Kafka (KRaft mode, no ZooKeeper), RabbitMQ (quorum queues), NATS JetStream
  - [x] Production-tuned configs for each broker (not defaults)
  - [x] Health checks and readiness probes
  - [x] Each broker exposable on a unique port

- [x] Task 2: Competitive benchmark harness (AC: 2, 3, 5, 9)
  - [x] Create `bench/competitive/bench.py` (Python with native client libraries for each broker)
  - [x] Implement workloads: throughput (single-producer/single-consumer), fan-out (1→3), multi-producer (3→1), message sizes (64B, 1KB, 64KB)
  - [x] Queue lifecycle: enqueue → consume → ack throughput
  - [x] Latency measurement: p50/p95/p99 for each broker
  - [x] Resource monitoring via `docker stats` during benchmark runs
  - [x] JSON output format compatible with `BenchReport` schema (for unified comparison)

- [x] Task 3: Fila self-benchmark wrapper (AC: 4)
  - [x] Run Fila's existing bench suite from Story 12.1 for baseline comparison
  - [x] Fila-only workload results (fair scheduling, throttle, Lua) included in existing bench suite
  - [x] Results in same JSON format as competitive benchmarks

- [x] Task 4: Makefile orchestration (AC: 7)
  - [x] Create `bench/competitive/Makefile` with targets: `bench-kafka`, `bench-rabbitmq`, `bench-nats`, `bench-fila`, `bench-competitive` (all)
  - [x] `bench-competitive` target: start all brokers → run suite → collect results
  - [x] Include `bench-clean` to tear down containers

- [x] Task 5: Methodology documentation (AC: 6, 8)
  - [x] Create `bench/competitive/METHODOLOGY.md` documenting hardware specs, broker configs, warmup period, measurement window, number of runs, and limitations
  - [x] Reference competitor configuration choices with justifications

## Dev Notes

### Architecture & Design Decisions

**Python for competitive benchmarks, not Rust.**
Each competitor has native client libraries in Python (confluent-kafka, pika/aio-pika, nats-py). Writing Rust clients for each would be massive scope. Python is the lingua franca for benchmark scripts and every broker has a well-maintained Python client.

**Docker Compose for broker management.**
All competitors run in containers. This gives us consistent environments, easy setup/teardown, and resource isolation. Fila can run either containerized or locally.

**Production-tuned configs, not defaults.**
Default Kafka uses 1 partition, default RabbitMQ uses classic queues. We use KRaft mode (no ZooKeeper), quorum queues, and JetStream respectively — the modern, production-recommended configurations.

**Single JSON output format.**
All results (Fila and competitors) use the same JSON schema. This allows unified comparison tables and reuse of the compare/aggregate tools from Story 12.2.

### What NOT To Do

- Do NOT use the Kafka Java client — Python confluent-kafka wraps librdkafka and is the standard for benchmarking
- Do NOT run benchmarks inside Docker (except the brokers) — the benchmark client runs on the host
- Do NOT compare default configs — use production-recommended settings
- Do NOT include the competitive benchmarks in CI — they require Docker and take too long

### References

- [Source: crates/fila-bench/src/report.rs] — BenchReport JSON schema
- [Source: _bmad-output/planning-artifacts/epics.md#12.3] — Epic plan with ACs

## Dev Agent Record

### Agent Model Used

### Debug Log References

### Completion Notes List

### File List
