# Benchmarks

What Fila measures, why, and the targets each measurement is held to.

> **No numbers here.** Measured results belonged to an implementation that no
> longer exists. This page defines the benchmark suite — the categories, the
> methodology, and the pass/fail targets — so results can be regenerated and
> compared against a stated bar rather than against a remembered one.
>
> Results are hardware-specific in any case. Run the suite on your own hardware
> for numbers relevant to your environment.

## Self-benchmarks

Single-node performance across throughput, latency, scheduling, and resource use.
Every benchmark drives a real server process through the SDK as a blackbox client —
no in-process shortcuts, because the transport is part of what is being measured.

| Category | What it measures | Target |
|----------|------------------|--------|
| **Throughput** | Sustained enqueue rate, single producer, 64B / 1KB / 64KB payloads. Measured over a fixed window after warmup. | — (tracked for regression) |
| **End-to-end latency** | Produce → consume round trip, p50 / p95 / p99, at several load levels. | sub-millisecond p50 |
| **Fair scheduling overhead** | DRR delivery throughput vs. plain FIFO delivery. | < 5% overhead |
| **Fairness accuracy** | Messages across N keys with weights 1:2:3:4:5; measures delivered share vs. weighted expectation within a delivery window. | < 5% deviation |
| **Lua hook overhead** | Per-message cost of running an `on_enqueue` hook vs. no hook. | < 50 µs per message |
| **Fairness key cardinality** | Throughput as the number of distinct active fairness keys grows. | graceful degradation |
| **Consumer concurrency** | Aggregate throughput as concurrent consumers scale. | scales with consumers |
| **Memory footprint** | Resident memory under sustained load. Dominated by the storage engine's buffer pool, not by message count. | flat in message count |
| **Compaction impact** | Throughput and latency during storage-engine compaction. | no sustained stall |

Two of these are correctness benchmarks wearing performance clothes. **Fairness
accuracy** is the product working or not working — a DRR scheduler that does not
distribute proportionally to weight is broken regardless of its throughput. **Fair
scheduling overhead** is the argument for the whole design: if fairness costs
meaningful throughput against FIFO, the tradeoff stops being free.

## Competitive comparison

Fila against Kafka, RabbitMQ, and NATS on identical workloads.

| Workload | Description |
|----------|-------------|
| **Throughput** | Sustained production rate (64B, 1KB, 64KB payloads) |
| **Latency** | Produce-consume round trip (p50 / p95 / p99) |
| **Lifecycle** | Full enqueue → consume → ack cycle |
| **Multi-producer** | Aggregate throughput, several concurrent producers |
| **Resources** | CPU and memory during the run |

### Ground rules

These matter more than the numbers they produce:

- **Competitors run production-recommended settings, not development defaults.**
  A benchmark that beats a misconfigured competitor measures nothing.
- **All brokers run in equivalent containers**, on the same host, in the same run.
- **All clients are native Rust libraries** — `rdkafka`, `lapin`, `async-nats` —
  so client-side overhead is comparable.
- **Durability settings are matched** as closely as each broker allows: Kafka in
  KRaft mode, RabbitMQ quorum queues with manual ack, NATS JetStream with file
  storage and explicit ack.

### The honest caveat

Fila is not faster than these brokers at being them. The comparison exists to show
that fair scheduling and broker-side throttling do not cost an order of magnitude —
that you can have per-key fairness without dropping to a fraction of a FIFO
broker's throughput. A competitive table that omits this framing is marketing.

## Methodology

- Warmup before every measurement window; discard warmup samples.
- Report percentiles, not just means — a mean latency hides the tail that
  actually hurts.
- Multiple runs, report the median, so a single noisy run cannot set a baseline.
- State payload size, producer count, and consumer count with every number. A
  throughput figure without them is not a result.

### Limitations

- Single-host benchmarks measure the broker, not the network. Cross-host numbers
  will differ.
- Container runtimes add variance, especially on virtualized hosts.
- Fairness accuracy is measured over a delivery window; DRR makes no guarantee
  about any single round.

## Regression detection

The benchmark suite is a regression gate, not a marketing exercise. It should run
in CI, save a baseline from the main branch, and compare pull requests against it,
flagging changes that exceed a threshold. Results are worth keeping as artifacts on
every run.

The suite is most valuable for the changes it catches that tests do not: a hot-path
allocation, a lock held slightly too long, an instrumentation macro formatting a
value it did not need to.
