# Story 21.2: Competitive Benchmark Overhaul

Status: ready-for-dev

## Story

As an evaluator comparing Fila against other brokers,
I want competitive benchmarks that use concurrent produce/consume, multiple runs, and comprehensive resource measurement,
so that published comparison numbers reflect realistic behavior, not best-case sequential latency.

## Acceptance Criteria

1. **Given** the competitive latency benchmark for any broker
   **When** the benchmark runs
   **Then** producers and consumers run concurrently (not sequentially)
   **And** the producer sends at a fixed rate, the consumer processes independently
   **And** end-to-end latency is measured as `consume_time - produce_timestamp` (timestamp embedded in payload)
   **And** at least 10,000 latency samples are collected per broker using HdrHistogram

2. **Given** the competitive benchmark orchestration (`Makefile`)
   **When** `make bench-competitive` runs
   **Then** each broker benchmark runs 3 times
   **And** results are aggregated using histogram merging (for latency) and median (for throughput)
   **And** the final `bench-{broker}.json` contains the aggregated results

3. **Given** the competitive resource benchmark
   **When** resource usage is measured
   **Then** disk I/O (bytes read/written) is captured alongside CPU% and memory MB
   **And** disk I/O is obtained via `docker stats --format` block I/O fields

4. **Given** the competitive benchmark results
   **When** the JSON report is emitted
   **Then** latency results include p50, p95, p99, p99.9, p99.99, and max (matching Story 21.1 format)

5. **Given** the competitive benchmark measurement
   **When** the benchmark runs for any broker
   **Then** measurement duration is at least 30 seconds per workload (up from 3 seconds)
   **And** a warmup period of at least 5 seconds precedes measurement (data discarded)

## Tasks / Subtasks

- [ ] Task 1: Update measurement constants (AC: 5)
  - [ ] 1.1 Change `WARMUP_SECS` from 1 to 5
  - [ ] 1.2 Change `MEASURE_SECS` from 3 to 30
  - [ ] 1.3 Remove `LATENCY_SAMPLES` constant (replaced by duration-based collection)

- [ ] Task 2: Implement concurrent produce/consume latency (AC: 1, 4)
  - [ ] 2.1 Refactor latency benchmarks: spawn producer and consumer as concurrent tasks
  - [ ] 2.2 Producer embeds nanosecond timestamp in first 8 bytes of payload
  - [ ] 2.3 Consumer extracts timestamp from payload, computes `now - embedded_timestamp`
  - [ ] 2.4 Run for MEASURE_SECS duration, collect into HdrHistogram
  - [ ] 2.5 Emit 6 percentile metrics (p50 through max) with serialized histogram in metadata

- [ ] Task 3: Add disk I/O to docker stats (AC: 3)
  - [ ] 3.1 Update `container_stats()` format string to include `{{.BlockIO}}`
  - [ ] 3.2 Parse block I/O read/write values (handle MiB/GiB/KiB units)
  - [ ] 3.3 Emit `{broker}_disk_io_read_mb` and `{broker}_disk_io_write_mb` metrics

- [ ] Task 4: Update Makefile for 3-run aggregation (AC: 2)
  - [ ] 4.1 Each `bench-{broker}` target runs the benchmark 3 times
  - [ ] 4.2 Use `bench-aggregate` binary to merge results
  - [ ] 4.3 Store aggregated result as `bench-{broker}.json`

- [ ] Task 5: Apply changes to all 4 broker implementations (AC: 1-5)
  - [ ] 5.1 Kafka: concurrent latency, 30s duration, 5s warmup, 10K+ samples
  - [ ] 5.2 RabbitMQ: concurrent latency, 30s duration, 5s warmup, 10K+ samples
  - [ ] 5.3 NATS: concurrent latency, 30s duration, 5s warmup, 10K+ samples
  - [ ] 5.4 Fila: concurrent latency, 30s duration, 5s warmup, 10K+ samples

## Dev Notes

### Key Files to Modify

- `crates/fila-bench/src/bin/bench-competitive.rs` — main competitive benchmark binary (all 4 brokers)
- `bench/competitive/Makefile` — orchestration, add 3-run aggregation

### Current Architecture

- `bench-competitive.rs` (1,193 lines): 4 broker modules (kafka, rabbitmq, nats, fila), each with sequential produce→consume latency
- `WARMUP_SECS=1`, `MEASURE_SECS=3`, `LATENCY_SAMPLES=100`
- `container_stats()` captures CPU% and memory only (no disk I/O)
- Makefile runs each broker once

### Implementation Guidance

**Concurrent latency approach**: Spawn producer task (sends at fixed rate using `tokio::time::interval`) and consumer task (processes as fast as possible). Producer writes current timestamp (8 bytes, `Instant::now().elapsed().as_nanos() as u64`) into the first 8 bytes of payload. Consumer reads those 8 bytes, computes latency. Both tasks share a `LatencyHistogram` (via `Arc<Mutex<>>` or channel). Run for `MEASURE_SECS`.

**Timestamp encoding**: Use a monotonic clock reference. Store `start_time` at benchmark begin. Producer writes `start_time.elapsed().as_nanos() as u64` into first 8 bytes (little-endian). Consumer reads those bytes, computes `current_elapsed - embedded_elapsed`.

**Docker stats block I/O**: Format `{{.BlockIO}}` returns something like `1.23GB / 456MB`. Parse both values.

### References

- [Source: _bmad-output/planning-artifacts/epics.md — Epic 21, Story 21.2 ACs]
- [Source: crates/fila-bench/src/bin/bench-competitive.rs — current implementation]
- [Source: bench/competitive/Makefile — orchestration]

## Dev Agent Record

### Agent Model Used

### Debug Log References

### Completion Notes List

### File List
