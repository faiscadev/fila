---
stepsCompleted: [1, 2, 3, 4, 5, 6]
inputDocuments: []
workflowType: 'research'
lastStep: 6
research_type: 'technical'
research_topic: 'message broker benchmarking methodology'
research_goals: 'understand what makes benchmarks trustworthy, review industry standards, identify workload profiles for queue-focused brokers, common pitfalls, and audit our competitive benchmark suite for gaps'
user_name: 'Lucas'
date: '2026-03-23'
web_research_enabled: true
source_verification: true
---

# Research Report: Message Broker Benchmarking Methodology

**Date:** 2026-03-23
**Author:** Lucas
**Research Type:** technical

---

## Executive Summary

Fila's benchmark suite (Epic 12) was a strong v1 — 10 self-benchmark categories, 4-broker competitive comparison, CI regression detection. But our research reveals that the numbers it produces cannot be trusted for optimization decisions yet. The most critical issue is **coordinated omission**: our closed-loop latency measurement can understate tail latency by orders of magnitude (demonstrated 2,670x in ScyllaDB's case). With only 100 latency samples, our p99 is a single data point — noise, not signal. And our 3-second measurement windows are too short to capture RocksDB compaction, memory pressure, or fairness scheduling artifacts.

The good news: **none of the major vendor tools (Kafka, RabbitMQ, OMB) fully solve these problems either.** The industry bar is low. Fila can leapfrog it with targeted fixes. The Rust `hdrhistogram` crate provides everything we need — CO correction, histogram merging, and arbitrary percentile queries at any sample count. Tokio's interval-based scheduling gives us open-loop load generation for gRPC without external tools.

This report identifies **12 specific gaps** in our current suite, prioritized into P0 (must fix before trusting numbers), P1 (needed for meaningful optimization), and P2 (completeness). A 4-phase implementation roadmap sequences the work: Foundation → Methodology → Completeness → Infrastructure. Phase 1 is small (mostly swapping `LatencySampler` for HdrHistogram and increasing sample counts) but transforms the statistical validity of every benchmark we run.

**Key Technical Findings:**
- Coordinated omission is the #1 methodological flaw in our latency benchmarks
- Queue brokers need fundamentally different benchmarks than log brokers (ack throughput, redelivery cost, fairness, processing time simulation)
- 100 latency samples is 100x too few for meaningful p99, and p99.9/max are where queue-broker problems actually manifest
- No off-the-shelf open-loop generator supports gRPC — we must build our own (pattern provided)
- The k6 executor model (closed-loop + open-loop in one framework) is the best architectural reference

**Top Recommendations:**
1. Replace `LatencySampler` with `hdrhistogram` crate (enables CO correction, merging, all percentiles)
2. Increase latency samples to 10,000+ and measurement duration to 30s+
3. Add open-loop load generation mode for correct latency-under-load measurement
4. Switch competitive latency to concurrent produce/consume (not sequential)
5. Add consumer processing time simulation — instant-ack hides 95% of real-world latency

---

## Table of Contents

1. [What Makes Benchmarks Trustworthy](#1-what-makes-benchmarks-trustworthy) — Coordinated omission, percentile reporting, HDR histograms, warmup, statistical confidence
2. [Industry Standards and Vendor Tooling](#2-industry-standards-and-vendor-tooling) — OMB, Kafka perf-test, RabbitMQ PerfTest, NATS bench
3. [Workload Profiles for Queue-Focused Brokers](#3-workload-profiles-for-queue-focused-brokers) — Queue vs log semantics, essential workload profiles, fairness, backpressure
4. [Common Pitfalls](#4-common-pitfalls-that-produce-misleading-results) — 11 pitfalls with real-world examples
5. [Fila's Current Benchmark Suite: Audit](#5-filas-current-benchmark-suite-audit) — What exists, strengths, 12 gaps, priority-ordered recommendations
6. [Academic and Industry References](#6-academic-and-industry-references)
7. [Tooling Ecosystem and Integration Patterns](#7-tooling-ecosystem-and-integration-patterns) — HdrHistogram Rust crate, open-loop for gRPC, result formats, CI integration
8. [Architectural Patterns for Benchmark Harness Design](#8-architectural-patterns-for-benchmark-harness-design) — Executor model, declarative workloads, orchestrator/worker, warmup, self-test
9. [Implementation Roadmap](#9-implementation-roadmap) — 4 phases with dependency ordering
10. [Key Decisions](#10-key-decisions-to-make) — 5 decisions requiring input before implementation

---

## Technical Research Scope Confirmation

**Research Topic:** Message broker benchmarking methodology
**Research Goals:** Understand what makes benchmarks trustworthy, review industry standards, identify workload profiles for queue-focused brokers, common pitfalls, and audit our competitive benchmark suite for gaps

**Technical Research Scope:**

- Statistical rigor — warmup, percentile reporting, confidence intervals, reproducibility
- Industry standards — OpenMessaging Benchmark Framework, vendor tooling
- Queue-specific workloads — ack/nack lifecycle, fairness, backpressure, tail latency
- Common pitfalls — coordinated omission, mismatched configs, insufficient duration
- Gap analysis — audit `bench/competitive/` and `fila-bench` against best practices

**Research Methodology:**

- Current web data with rigorous source verification
- Multi-source validation for critical technical claims
- Confidence level framework for uncertain information
- Direct code review of our benchmark suite against findings

**Scope Confirmed:** 2026-03-23

---

## 1. What Makes Benchmarks Trustworthy

### 1.1 Coordinated Omission

The single most important concept in latency benchmarking. Coined by Gil Tene, **coordinated omission** occurs when a load generator inadvertently synchronizes with the system under test, failing to send (and therefore measure) requests during periods of degraded performance.

**How it happens:** Most benchmark tools use a *closed-loop* model — a worker sends a request, waits for the response, then sends the next. If a response takes 1.5 seconds but requests are expected every 100ms, the worker cannot send the 14 requests it should have during that delay. Those missed requests (and their wait times) are never recorded.

**The core distinction:**
- **Service time** = how long the system takes to process a request once it starts
- **Response time** = waiting time in queue + service time
- Closed-loop benchmarks measure service time but report it as response time

**Magnitude of distortion:** ScyllaDB demonstrated that without CO correction, P99 latency showed 249 microseconds. With proper open-loop measurement, the actual P99 was **665 milliseconds** — a **2,670x distortion**.

**The CTRL+Z test:** Pause the system under test mid-benchmark. If your measurement tool doesn't capture the stall in its latency distribution, you have coordinated omission.

**Correction methods:**
1. **Open-loop load generation:** Send requests at a fixed rate regardless of responses
2. **HdrHistogram correction:** `recordValueWithExpectedInterval()` inserts synthetic samples for missing intervals
3. **Explicit scheduling:** Track `scheduled_time` per request; latency = `current_time - scheduled_time + service_time`

_Sources: [Brave New Geek](https://bravenewgeek.com/everything-you-know-about-latency-is-wrong/), [ScyllaDB](https://www.scylladb.com/2021/04/22/on-coordinated-omission/), [Gil Tene (InfoQ)](https://www.infoq.com/presentations/latency-response-time/)_

### 1.2 Percentile Reporting

Averages are nearly useless for latency. As Gil Tene notes: "the median is the number that 99.9999999999% of response times will be worse than."

**What to report:** At minimum p50, p95, p99, p99.9. Ideally also p99.99 and max.

**Never average percentiles across windows.** This is mathematically meaningless. You cannot derive accurate p99.9 from averaged per-window p99.9 values. Instead, merge the underlying histograms and compute percentiles from the merged distribution.

**The max value is signal, not noise.** For queue brokers, the maximum latency represents the worst-case message delivery time — what triggers visibility timeout expirations, redelivery storms, and DLQ routing.

**Why p99/p999 matter more for queues than logs:** In a log broker, a slow consumer simply falls behind on its offset — no server-side consequence. In a queue broker, a slow consumer triggers visibility timeout expiry, message redelivery, potential duplicate processing, and DLQ routing. Every p99 spike cascades.

### 1.3 HDR Histograms

The gold standard for latency recording. Solves the range-vs-precision tradeoff:
- Tracks values from 1 microsecond to 1 hour with 3 significant digits in ~185KB
- Recording cost: 3–6 nanoseconds on modern CPUs
- Supports coordinated omission correction
- Lock-free concurrent recording
- Available in Java, C, Rust, Python, Go, and more

_Source: [HdrHistogram](https://github.com/HdrHistogram/HdrHistogram)_

### 1.4 Warmup Periods

JVM-based systems (Kafka, RabbitMQ PerfTest) need warmup for JIT compilation. Native systems need warmup for cache priming, buffer pool allocation, and RocksDB memtable filling. Results collected during warmup must be discarded — they measure startup behavior, not steady-state performance.

### 1.5 Multiple Runs and Confidence

Single runs provide no statistical confidence. Best practice:
- Run N >= 3 independent trials (ideally 5+)
- Report median with min/max or standard deviation
- Jay Kreps' canonical Kafka benchmark (single run, point measurements, no confidence intervals) is the industry norm — but it's statistically weak

### 1.6 Test Duration

Short benchmarks capture only warm-cache performance. Redpanda latencies were stable for ~12 hours, then degraded as NVMe write amplification from random I/O kicked in. Production systems run for weeks. Testing on empty drives with no retention pressure shows artificially low latencies.

### Summary: Trustworthy Benchmark Checklist

| Requirement | Why |
|---|---|
| Open-loop load generation (or CO correction) | Closed-loop understates tail latency by orders of magnitude |
| Full latency distribution (p50–p99.99 + max) | Averages hide the latency that matters |
| Merge histograms, never average percentiles | Averaging percentiles is mathematically invalid |
| Warmup with discarded data | Startup != steady-state |
| Multiple independent runs (N >= 3) | Single runs have no confidence |
| Sufficient duration (hours, not minutes) | Short tests miss sustained-state degradation |
| Tested under retention/disk pressure | Empty disks hide real-world costs |
| Identical, audited configs for all systems | Mismatched configs invalidate comparisons |

---

## 2. Industry Standards and Vendor Tooling

### 2.1 OpenMessaging Benchmark Framework (OMB)

**Architecture:** Driver-worker model. A driver node orchestrates test execution, worker nodes execute and report back via HTTP. Each broker has a dedicated driver module.

**Workload definition:** YAML files specify topic count, partition count, message size, subscription type, producer throughput targets, test duration, and consumer backlog scenarios.

**Metrics reported:** Publish and consume rates (msgs/sec, MB/sec), latency percentiles (p50, p95, p99, p99.9, p99.99, max), backlog depth, production delay (signals saturation).

**Statistical approach:** Aggregates over 10-second measurement windows. Includes warmup phase. The project explicitly states: "We do not consider or plan to release any unilateral test results based on this standard" — positioning itself as a framework, not a results publisher.

**Critical known bug:** Jack Vanlightly found a histogram collection bug where failed retries reset metrics, causing "high percentile results to be significantly lower than actual performance." Framework bugs silently produce misleading numbers.

_Sources: [OpenMessaging](https://openmessaging.cloud/docs/benchmarks/), [GitHub](https://github.com/openmessaging/benchmark)_

### 2.2 Kafka: `kafka-producer-perf-test`

- **Sampling-based percentile calculation**, NOT HDR histograms
- Samples at intervals: `sampling = numRecords / Math.min(numRecords, 500000)` — at most 500K samples regardless of total records
- Latency stored as `int` milliseconds (loses sub-ms precision)
- Has explicit warmup via `--warmup-records`
- Reports: records/sec, MB/sec, avg latency, max latency, p50/p95/p99/p99.9
- **No coordinated omission correction. No HDR histogram support.**

_Source: [Kafka tools source](https://github.com/apache/kafka/blob/trunk/tools/src/main/java/org/apache/kafka/tools/ProducerPerformance.java)_

### 2.3 RabbitMQ: PerfTest

- Official throughput and latency testing tool (AMQP 0.9.1)
- Uses `System.nanoTime()` for sub-microsecond precision within a JVM
- Configurable sampling intervals (default 1-second windows)
- **No explicit warmup** — has `--producer-random-start-delay` to stagger startup
- Reports: msgs/sec published/consumed, publisher confirm latency, consumer latency (min, median, p75, p95, p99)
- **No HDR histogram. No coordinated omission correction. Closed-loop by default.**

_Source: [RabbitMQ PerfTest](https://perftest.rabbitmq.com/)_

### 2.4 NATS: `nats bench`

- CLI benchmarking built into `nats` CLI
- Each client runs as a Go routine with its own connection
- Modes: Core NATS pub/sub, request-reply, JetStream (sync/async/batch/KV)
- Per-client statistics: min, max, average rate, standard deviation
- **HDR histogram support** on the latency subcommand (p10 through p99.99999)
- **NATS is the only major broker whose built-in tool includes HDR histogram percentile output**

_Sources: [NATS docs](https://docs.nats.io/using-nats/nats-tools/nats_cli/natsbench), [GitHub](https://github.com/nats-io/natscli)_

### 2.5 Vendor Comparison

| Feature | Kafka perf-test | RabbitMQ PerfTest | NATS bench | OMB |
|---|---|---|---|---|
| HDR Histograms | No | No | Yes | Partial (buggy) |
| CO Correction | No | No | No | No |
| Warmup | Yes (--warmup-records) | No (stagger only) | No | Yes |
| Open-loop | No | No | No | Partial (target rate) |
| Multi-run | No | No | No | No |
| Percentiles | p50-p99.9 | p75-p99 | p10-p99.99999 | p50-p99.99 |

**Key insight:** None of the major vendor tools fully address coordinated omission. The industry standard is surprisingly weak.

---

## 3. Workload Profiles for Queue-Focused Brokers

### 3.1 Why Queue Brokers Need Different Benchmarks

The fundamental difference: in a queue broker, the server tracks per-message consumption state. Messages are individually acked, nacked for redelivery, and routed to DLQs. This creates workload patterns that don't exist in log-based systems.

| Dimension | Queue Broker (RabbitMQ, SQS, Fila) | Log Broker (Kafka, Pulsar) |
|---|---|---|
| **Ack model** | Per-message ack/nack, server-side state mutation | Batch offset commit, client-side state |
| **Failure handling** | Visibility timeout, redelivery, DLQ routing | Consumer restart from last committed offset |
| **Scaling unit** | Queue count (single queue ≈ single thread) | Partition count |
| **Key latency metric** | p99/p999 end-to-end (includes lock/visibility) | p99 produce latency |
| **Backpressure** | Prefetch, credit flow, publisher confirms | Consumer lag (no server-side pressure) |
| **Critical benchmark** | Ack throughput under contention | Batch produce throughput |
| **Failure-mode benchmark** | Poison pill isolation, redelivery storm cost | Partition rebalance latency |

### 3.2 Essential Workload Profiles

**Profile 1: Three-Phase Lifecycle (Send → Receive → Ack)**

The SoftwareMill mqperf benchmark (12 systems compared) explicitly tested this lifecycle. This is the correct model for queue brokers — log brokers skip phases 2 and 3 entirely. Ack throughput under contention is the dominant cost.

**Profile 2: Consumer Processing Time Simulation**

The most overlooked dimension. RabbitMQ's queuing theory: with 4ms processing and 50ms RTT, optimal prefetch is 26. With slower 40ms processing, the same prefetch creates 880ms of queuing delay — "95% of the latency comes from the buffer, not the network." Benchmarks that use instant-ack (zero processing time) measure best-case, not reality. Must test: 0ms, 1ms, 10ms, 100ms, 1s processing times.

_Source: [RabbitMQ Queuing Theory](https://www.rabbitmq.com/blog/2012/05/11/some-queuing-theory-throughput-latency-and-bandwidth)_

**Profile 3: Poison Pill / Redelivery Storm**

Nack-with-requeue on a message that always fails creates an infinite redelivery loop "burning CPU and blocking other messages." What happens to throughput and latency of *other* messages when a fraction are poison pills? SQS FIFO queues isolate failures to message groups — a key differentiator.

**Profile 4: Fair Scheduling Under Load**

**Jain's Fairness Index:** `f(x₁,...,xₙ) = (Σxᵢ)² / (n · Σxᵢ²)`. Ranges from 1/n (worst) to 1 (perfect).

Benchmark approaches:
1. N queues with equal weight → measure per-queue throughput ratio → compute Jain's index
2. N queues with varying weights → verify throughput ratios match weight ratios
3. Under overload: one queue receives 10x messages → do non-overloaded queues maintain fair share?
4. Time-series fairness: Jain's index on instantaneous throughput, not just aggregate

_Sources: [Jain](https://www.cse.wustl.edu/~jain/atmf/ftp/af_fair.pdf), [Springer](https://link.springer.com/chapter/10.1007/978-3-319-09465-6_42)_

**Profile 5: Backpressure and Queue Depth**

The hockey-stick curve: response time starts flat, curves sharply past ~80% saturation. Key insight: latency degrades proportionally to queue depth. Must test: latency at 0%, 50%, 80%, 100%, and 120% of max throughput.

RabbitMQ found that without publisher confirms during extreme stress, "TCP back pressure was not enough to stop overload." With confirms and in-flight limits → graceful degradation.

_Source: [RabbitMQ Stress Tests](https://www.rabbitmq.com/blog/2020/05/15/quorum-queues-and-flow-control-stress-tests)_

**Profile 6: Message Size Distribution**

Brave New Geek tested: 256B @ 3K/s, 1KB @ 3K/s, 5KB @ 2K/s, 1KB @ 20K/s, 1MB @ 100/s. Key finding: RabbitMQ and Kafka showed *superior* performance with 1MB messages compared to smaller payloads, with "dramatically lower tail latencies." Production workloads are rarely uniform.

**Profile 7: Many-Client vs Few-Fast-Client**

RabbitMQ's official quorum queue benchmarks distinguish:
- 20 publishers × 2K msg/s × 10 queues = 40K msg/s
- 500 publishers × 60 msg/s × 100 queues = 30K msg/s

Same total throughput, different stress: connection management vs queue throughput.

_Source: [RabbitMQ Quorum Queue Stress Tests](https://www.rabbitmq.com/blog/2020/05/15/quorum-queues-and-flow-control-stress-tests)_

---

## 4. Common Pitfalls That Produce Misleading Results

Drawing primarily from Jack Vanlightly's Kafka-vs-Redpanda analysis — the gold standard reference for broker benchmark pitfalls:

### Pitfall 1: Coordinated Omission (Closed-Loop Testing)

Measuring service time but presenting it as response time. Understates tail latency by orders of magnitude. See Section 1.1.

### Pitfall 2: Narrow Workload Optimization

Benchmarks tuned for specific conditions (4 producers, fixed partitions, empty drives) don't generalize. When changed to 50 producers or when drives fill, performance characteristics shift dramatically.

### Pitfall 3: Insufficient Test Duration

Redpanda latencies were stable for ~12 hours, then degraded as NVMe write amplification kicked in. Short benchmarks capture only warm-cache performance.

### Pitfall 4: Testing Before Retention Limits

Production systems operate with full disks under retention policies. Testing on empty drives shows artificially low latencies. "End-to-end latency results are really only valid once the retention size has been reached."

### Pitfall 5: Mismatched Configurations

Vanlightly found three significant Kafka misconfigurations in Redpanda's benchmark code (including inappropriate fsync settings) that artificially degraded Kafka's numbers. Always audit both sides' configurations.

### Pitfall 6: Ignoring Record Keys

Real applications use keys for ordering guarantees. Adding keys changed the throughput picture entirely: Kafka reached target 500 MB/s; Redpanda topped out at 330 MB/s.

### Pitfall 7: Omitting Backlog/Recovery Scenarios

Under sustained producer load, only Kafka could fully drain consumer backlogs. Most benchmarks test only steady-state, never failure recovery.

### Pitfall 8: Benchmark Framework Bugs

OMB's histogram reset bug caused high-percentile results to be "significantly lower than actual performance." The framework itself can be a source of error.

### Pitfall 9: Runtime Version Mismatch

Upgrading from Java 11 to Java 17 materially changed TLS workload performance. Comparing systems on different runtime versions invalidates results.

### Pitfall 10: Single-Run Reporting

Most published broker benchmarks report a single run with no confidence intervals, no variance analysis, and no reproducibility documentation.

### Pitfall 11: Instant-Ack (Queue-Specific)

Benchmarking queue brokers with zero consumer processing time measures only the broker's internal throughput, not the system's behavior under realistic load. Prefetch and queuing effects only appear when consumers take non-zero time to process.

_Primary source: [Jack Vanlightly — Kafka vs Redpanda: Do the Claims Add Up?](https://jack-vanlightly.com/blog/2023/5/15/kafka-vs-redpanda-performance-do-the-claims-add-up)_

---

## 5. Fila's Current Benchmark Suite: Audit

### 5.1 What Exists

**Self-Benchmarks (`fila-bench` crate, 10 categories):**

| Category | Warmup | Measurement | Samples | Percentiles |
|---|---|---|---|---|
| Enqueue Throughput (1KB) | 1s | 3s | continuous | msg/s, MB/s |
| E2E Latency (light/moderate/saturated) | 3s background traffic | 100 samples | 100 | p50, p95, p99 |
| Fairness Overhead (DRR vs FIFO) | 1s | 3s | continuous | % overhead |
| Fairness Accuracy (weighted) | — | 5,000 consumed of 10,000 | 5 tenants | deviation % |
| Lua on_enqueue Overhead | 1s | 3s | continuous | µs/msg |
| Memory Footprint | — | 10K messages | 1 | RSS MB |
| RocksDB Compaction Impact | — | during compaction | variable | p99 |
| Queue Depth Scaling (optional) | — | 1M, 10M msgs | 2 depths | msg/s |
| Key Cardinality Scaling | — | 10/1K/10K keys | 3 cardinalities | msg/s |
| Consumer Concurrency | — | 1/10/100 consumers | 3 levels | aggregate msg/s |

**Competitive Benchmarks (`bench/competitive/`, 5 workloads per broker):**

| Workload | Brokers | Payloads | Measurement |
|---|---|---|---|
| Throughput | Fila, Kafka 3.9, RabbitMQ 3.13, NATS 2.11 | 64B, 1KB, 64KB | 3s sustained production |
| Latency | all 4 | 1KB | 100 sequential produce-consume samples |
| Lifecycle | all 4 | 1KB | pre-load 1,000 → consume+ack throughput |
| Multi-Producer | all 4 | 1KB | 3 concurrent producers |
| Resource Usage | all 4 | — | docker stats CPU%, memory MB |

**Statistical Infrastructure:**
- `LatencySampler`: sorts samples, computes percentile via index
- `ThroughputMeter`: count / elapsed wall-clock time
- Multi-run aggregation: 3 runs in CI, median per metric
- Regression detection: 10% threshold, polarity-aware (higher-is-better vs lower-is-better)
- JSON output with commit hash, timestamp, version

**CI Integration:**
- `bench-regression.yml`: runs on every PR + push to main, 3 runs, median, baseline comparison
- `bench-competitive.yml`: runs on PR + main + manual, single run per broker

### 5.2 Design Strengths

1. **Native clients for all brokers** — uses Rust clients, avoiding language runtime overhead in measurement
2. **Explicit error handling** — failed ops aren't counted in throughput
3. **Reproducible Docker setup** — all brokers containerized with pinned versions
4. **Traceability** — every report includes commit hash, timestamp, version
5. **Per-category metrics** — separates throughput, latency, lifecycle (not a god-metric)
6. **Fairness validation** — consuming-a-window technique avoids full-consumption bias
7. **Polarity-aware regression detection** — correctly handles "lower is better" vs "higher is better"

### 5.3 Gaps and Issues

#### Gap 1: Coordinated Omission (CRITICAL)

**Current state:** Latency benchmarks use a closed-loop model — produce a message, consume it, record time, repeat. 100 sequential samples.

**Problem:** This measures service time, not response time. If the broker stalls for 500ms on one message, the benchmark just records that one slow sample — it doesn't account for all the requests that *should have been sent* during the stall. Under load, this understates tail latency by potentially orders of magnitude.

**Fix needed:** Either switch to open-loop load generation (constant-rate producer independent of consumer) or use HdrHistogram with coordinated omission correction. The Rust `hdrhistogram` crate supports both.

#### Gap 2: Sample Size Too Small for Latency

**Current state:** 100 latency samples per load level.

**Problem:** With 100 samples, p99 is determined by a single data point (the worst one). p99.9 is impossible to measure. The result is dominated by noise. No statistical confidence can be established.

**Fix needed:** Minimum 10,000 samples for meaningful p99, 100,000+ for p99.9. Use HdrHistogram to avoid memory issues at these sample counts.

#### Gap 3: No p99.9 or Max Reporting

**Current state:** Reports p50, p95, p99 only.

**Problem:** p99.9 and max are where queue-broker problems manifest — redelivery storms, compaction stalls, lock contention. Missing these hides the most actionable data.

**Fix needed:** Add p99.9, p99.99, and max to all latency reports.

#### Gap 4: Measurement Duration Too Short

**Current state:** 1s warmup + 3s measurement for throughput. 100 samples for latency.

**Problem:** 3 seconds doesn't capture sustained-state behavior. RocksDB compaction cycles, memory pressure, and fairness scheduling artifacts may not manifest in 3 seconds. The compaction test explicitly tries to address this but the timing is probabilistic (RocksDB may not have started compaction yet).

**Fix needed:** Configurable duration with defaults of at least 30s for throughput, 60s+ for latency under load. Optional long-running mode (10+ minutes) for CI nightly.

#### Gap 5: Single-Run Competitive Benchmarks

**Current state:** Self-benchmarks run 3x with median aggregation. Competitive benchmarks run once.

**Problem:** Single-run competitive results have high variance on CI runners. The methodology doc acknowledges this ("all brokers see same CI variance") but that's insufficient — the variance affects the comparison itself.

**Fix needed:** Run competitive benchmarks 3x with median aggregation, matching the self-benchmark approach.

#### Gap 6: No Consumer Processing Time Simulation

**Current state:** All benchmarks use instant-ack (zero processing time).

**Problem:** This measures only the broker's internal throughput. Real consumers take 1ms–1s to process each message. Prefetch tuning, queuing delay, and backpressure effects only appear with non-zero processing time. RabbitMQ demonstrated that buffer-induced latency can be "95% of the total latency."

**Fix needed:** Add workload profiles with simulated processing times: 0ms (current), 1ms, 10ms, 100ms. Measure how latency degrades as processing time increases.

#### Gap 7: No Backpressure / Saturation Testing

**Current state:** "Saturated" load level uses 20 producers, but there's no measurement of behavior past capacity.

**Problem:** The hockey-stick curve — response time flat until ~80% capacity, then exponential — is the most important operational characteristic. Without measuring the inflection point and degradation curve, we don't know Fila's capacity limits.

**Fix needed:** Ramp test: increase producer rate from 10% to 150% of max throughput, measure latency at each level. Identify the saturation point and degradation curve.

#### Gap 8: No Queue Depth / Backlog Effect on Latency

**Current state:** Queue depth scaling tests measure throughput at 1M and 10M messages. No latency measurement under backlog.

**Problem:** Queue depth directly affects message delivery latency. A consumer joining a queue with 1M pending messages has very different latency characteristics than one joining an empty queue. This is a critical operational dimension.

**Fix needed:** Measure e2e latency with varying pre-loaded queue depths: 0, 1K, 10K, 100K, 1M messages.

#### Gap 9: No Failure / Recovery Benchmarks

**Current state:** No tests for message redelivery cost, nack storms, or DLQ routing overhead.

**Problem:** Production queue brokers spend significant time in failure paths — redelivering messages, routing to DLQs, handling poison pills. These paths are often 10x more expensive than the happy path and are never benchmarked.

**Fix needed:** Add workloads: (a) nack 10% of messages, measure impact on overall throughput; (b) poison pill in one fairness group, verify other groups unaffected; (c) DLQ routing throughput under mixed ack/nack workload.

#### Gap 10: Percentile Computation Not Using HDR Histograms

**Current state:** `LatencySampler` collects raw samples into a Vec, sorts, indexes.

**Problem:** This works for small sample counts but doesn't scale. Cannot do coordinated omission correction. Cannot merge histograms across runs (currently takes median of per-run percentiles, which is better than averaging but still not histogram merging). No sub-microsecond precision.

**Fix needed:** Replace `LatencySampler` with `hdrhistogram::Histogram`. Enables CO correction, histogram merging, and arbitrary percentile queries at any sample count.

#### Gap 11: Competitive Latency Uses Wrong Model

**Current state:** Competitive latency benchmark does sequential produce-consume-record, 100 times.

**Problem:** This is closed-loop, low-volume, and measures best-case service time. It doesn't represent how any of these brokers behave under concurrent production and consumption — which is the actual use case. All brokers will look "fast" under this test because there's no contention.

**Fix needed:** Run producers and consumers concurrently. Producer at fixed rate, consumer processes independently. Measure time from produce to consume (timestamp in message payload). This is the open-loop model that reveals real latency differences.

#### Gap 12: No Disk I/O Measurement

**Current state:** Resource measurement captures only CPU% and memory via `docker stats`.

**Problem:** Disk I/O is the primary bottleneck for persistent queue brokers. `docker stats` doesn't expose block I/O by default. The methodology doc notes this gap but doesn't address it.

**Fix needed:** Use `docker stats --format '{{.BlockIO}}'` or mount `/proc` for detailed I/O stats. Alternatively, use `iostat` on the host.

### 5.4 Priority-Ordered Recommendations

**Must fix before trusting numbers (P0):**
1. Replace `LatencySampler` with HdrHistogram
2. Increase latency sample count to 10,000+
3. Add p99.9, p99.99, max to all latency reports
4. Switch competitive latency to concurrent produce/consume model
5. Run competitive benchmarks 3x with median aggregation

**Should fix for meaningful optimization (P1):**
6. Add open-loop load generation mode (constant-rate producer)
7. Add consumer processing time simulation (0/1/10/100ms)
8. Increase measurement duration to 30s+ (configurable)
9. Add backpressure ramp test (10%→150% capacity)
10. Add queue depth effect on latency test

**Nice to have for completeness (P2):**
11. Add failure/recovery benchmarks (nack storms, DLQ routing)
12. Add disk I/O measurement
13. Add many-client workload profile (500 connections × low rate)
14. Add Jain's Fairness Index computation to fairness tests

---

## 6. Academic and Industry References

| Source | Type | Key Contribution |
|---|---|---|
| [Gil Tene: How NOT to Measure Latency](https://www.infoq.com/presentations/latency-response-time/) | Talk | Coordinated omission, service time vs response time |
| [Brave New Geek: Everything You Know About Latency Is Wrong](https://bravenewgeek.com/everything-you-know-about-latency-is-wrong/) | Article | Practical CO explanation, percentile math |
| [Jack Vanlightly: Kafka vs Redpanda](https://jack-vanlightly.com/blog/2023/5/15/kafka-vs-redpanda-performance-do-the-claims-add-up) | Analysis | Gold standard for competitive benchmark pitfalls |
| [SoftwareMill: mqperf](https://softwaremill.com/mqperf/) | Benchmark | 12-system comparison using queue lifecycle model |
| [ScyllaDB: On Coordinated Omission](https://www.scylladb.com/2021/04/22/on-coordinated-omission/) | Article | 2,670x CO distortion example |
| [RabbitMQ: Queuing Theory](https://www.rabbitmq.com/blog/2012/05/11/some-queuing-theory-throughput-latency-and-bandwidth) | Blog | Prefetch tuning, buffer-induced latency |
| [RabbitMQ: Stress Tests](https://www.rabbitmq.com/blog/2020/05/15/quorum-queues-and-flow-control-stress-tests) | Blog | Workload profiles for queue brokers |
| [OpenMessaging Benchmark](https://github.com/openmessaging/benchmark) | Framework | Industry-standard benchmark framework |
| [HdrHistogram](https://github.com/HdrHistogram/HdrHistogram) | Library | Latency recording with CO correction |
| [Jain's Fairness Index](https://www.cse.wustl.edu/~jain/atmf/ftp/af_fair.pdf) | Paper | Standard fairness measurement |
| [Shreedhar & Varghese: DRR](https://dl.acm.org/doi/10.1145/217391.217453) | Paper | O(1) fair scheduling (Fila's algorithm) |
| [Retter et al: Benchmarking MQs (MDPI 2023)](https://www.mdpi.com/2673-4001/4/2/18) | Paper | 18-experiment cross-broker comparison |
| [Brave New Geek: MQ Latency](https://bravenewgeek.com/benchmarking-message-queue-latency/) | Benchmark | Multi-size, multi-broker latency comparison |

---

## 7. Tooling Ecosystem and Integration Patterns

### 7.1 HdrHistogram Rust Crate

**Crate:** `hdrhistogram` v7.5.4 ([docs.rs](https://docs.rs/hdrhistogram/latest/hdrhistogram/), [GitHub](https://github.com/HdrHistogram/HdrHistogram_rust))

**Core API:**
- `Histogram::new(sigfig)` — auto-resizing; `new_with_max`/`new_with_bounds` for fixed range
- `record(value)`, `record_n(value, count)`, `saturating_record(value)` — basic recording
- `value_at_quantile(q)` / `value_at_percentile(p)` — percentile extraction
- `mean()`, `stdev()`, `min()`, `max()` — summary stats
- `add(source)` — merge two histograms (for aggregating per-thread results)

**Coordinated omission correction — two mutually exclusive approaches:**
1. **At-recording:** `record_correct(value, interval)` — auto-generates additional decreasing value records down to `interval` for any value exceeding the expected interval. Use during the test.
2. **Post-recording:** `correct_for_coordinated_omission(interval)` — returns a corrected copy. Use after the test on a raw histogram.

These two must not be combined on the same dataset.

**Histogram merging:** `add(&other)` merges another histogram into the current one. Also `add_correct(source, interval)` which merges with CO correction. This is how to aggregate per-worker histograms.

**Serialization:** V2 format via the `serialization` module (base64 + flate2 compression). Compatible with Java HdrHistogram for cross-language interop.

**Thread safety:** `SyncHistogram` in the `sync` module. Uses `crossbeam-channel` internally.

**Gaps vs Java:** No `AtomicHistogram`, `ConcurrentHistogram`, `DoubleHistogram`, `Recorder`, or value shifting. Functional but less featureful.

**Practical recommendation:** Use per-worker `Histogram` instances, then `add()` to merge. Use `record_correct(value, expected_interval)` during recording for open-loop tests. 3 significant figures is standard.

### 7.2 Open-Loop Load Generation for gRPC

No off-the-shelf open-loop generator supports gRPC or custom protocols. wrk2 is HTTP-only. We must build our own.

**Core pattern — interval-based sending independent of response:**

```rust
use tokio::time::{interval, Duration, Instant};
use hdrhistogram::Histogram;

async fn open_loop_benchmark(
    client: FilaClient,
    target_rate: u64,  // requests per second
    duration: Duration,
) -> Histogram<u64> {
    let send_interval = Duration::from_secs_f64(1.0 / target_rate as f64);
    let mut ticker = interval(send_interval);
    let start = Instant::now();
    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();

    // Collector task — gathers latency results
    let collector = tokio::spawn(async move {
        let mut hist = Histogram::<u64>::new(3).unwrap();
        while let Some((scheduled_at, completed_at)) = rx.recv().await {
            let latency_us = (completed_at - scheduled_at).as_micros() as u64;
            hist.record_correct(latency_us, send_interval.as_micros() as u64).ok();
        }
        hist
    });

    // Sender loop — fires at fixed rate, does NOT await response
    while start.elapsed() < duration {
        let scheduled_time = ticker.tick().await;
        let mut client = client.clone();
        let tx = tx.clone();
        tokio::spawn(async move {
            let _response = client.enqueue(request).await;
            let completed = Instant::now();
            tx.send((scheduled_time, completed)).ok();
        });
    }
    drop(tx);
    collector.await.unwrap()
}
```

**Key design decisions:**
1. **`tokio::time::interval`** maintains fixed cadence. Missed ticks fire immediately to catch up — this is the open-loop behavior.
2. **Measure from scheduled time, not actual send time.** Latency = `completed - scheduled_time`, including queuing delay.
3. **`record_correct(value, interval)`** applies CO correction at recording time.
4. **`tokio::spawn` for each request** — fire and forget. Sender loop never blocks on response.
5. **Per-worker histograms merged with `add()`** for multi-worker scenarios.
6. **Clone tonic client** — cheaply cloneable (wraps HTTP/2 channel with connection pooling).

**Scaling:** Spawn N workers, each targeting `target_rate / N` rps. Merge histograms at the end.

_Sources: [wrk2](https://github.com/giltene/wrk2), [tokio interval](https://docs.rs/tokio/latest/tokio/time/index.html), [Artillery workload models](https://www.artillery.io/blog/load-testing-workload-models)_

### 7.3 Benchmark Harness: Custom vs Criterion

**Criterion.rs** is the de facto Rust microbenchmark framework — statistical analysis, regression detection, HTML reports. But it has fundamental limitations for system benchmarks:

- Cannot benchmark binary crates or non-`pub` functions
- Designed for sub-second iterations measuring wall-clock time
- No support for custom metrics (throughput, tail latency percentiles)
- No open-loop load generation or CO correction
- No long-running benchmark support

**Custom harness** (`harness = false` in `Cargo.toml`): Full control over measurement methodology. The existing `fila-bench` crate already takes this approach — this is correct.

_Sources: [Criterion.rs limitations](https://bheisler.github.io/criterion.rs/book/user_guide/known_limitations.html), [Bencher custom harness guide](https://bencher.dev/learn/benchmarking/rust/custom-harness/)_

### 7.4 Benchmark Result Formats

There is no universal standard. Practical options:

**github-action-benchmark format** ([GitHub](https://github.com/benchmark-action/github-action-benchmark)):
```json
[
  {
    "name": "enqueue_p99_latency",
    "unit": "microseconds",
    "value": 142.5,
    "range": "3.2",
    "extra": "messages: 100000\nworkers: 4"
  }
]
```
Use `customSmallerIsBetter` or `customBiggerIsBetter` tool type.

**Bencher Metric Format (BMF)** ([spec](https://bencher.dev/docs/reference/bencher-metric-format/)):
```json
{
  "enqueue_throughput": {
    "throughput": {
      "value": 150000.0,
      "lower_value": 148000.0,
      "upper_value": 152000.0
    }
  }
}
```
Structure: `benchmark_name -> measure_name -> {value, lower_value?, upper_value?}`.

**Fila's current format** is a custom JSON schema with `version`, `timestamp`, `commit`, and a `benchmarks` array. This works but doesn't plug into any CI visualization tooling.

**Recommendation:** Emit github-action-benchmark JSON alongside current format. Enables free CI tooling (chart visualization, PR commenting, regression alerts) without breaking existing regression detection.

### 7.5 CI Integration Patterns

**github-action-benchmark** ([GitHub](https://github.com/benchmark-action/github-action-benchmark)):

**Baseline storage strategies:**
1. **gh-pages branch** (default): Stores `data.js`, provides auto chart visualization at `https://{user}.github.io/{repo}/dev/bench/`
2. **External JSON file** via `external-data-json-path`: Store in GitHub Actions cache. Better for private repos.
3. **Separate branch**: `internal/benchmark-data`

**PR workflow:**
- `comment-on-alert: true` + `github-token` → posts comparison comment when regression detected
- `fail-on-alert: true` → blocks PR
- `alert-threshold`: configurable (default 200%; we should use 110-120%)
- `summary-always: true` → adds benchmark results to job summary on every PR

**Current Fila CI approach** (`bench-regression.yml`): 3 runs, median aggregation, baseline cached by commit hash, comparison posted to PR body. This is functional but lacks visualization and uses a custom comparison pipeline.

**Recommendation:** Layer github-action-benchmark on top of the existing pipeline. Keep the existing 3-run median aggregation (which github-action-benchmark doesn't do natively), then feed the aggregated result into the action for charting and alerting.

### 7.6 Continuous Benchmarking Services

| Service | Model | Key Feature | Fit for Fila |
|---|---|---|---|
| **github-action-benchmark** | Free, self-hosted | Chart visualization, PR comments | Best starting point |
| **Bencher** | Free tier + self-host | Student's t-test regression detection | Upgrade path if statistical sophistication needed |
| **CodSpeed** | Paid, dedicated runners | 0.56% CoV (vs 2.66% on GH runners) | If CI noise becomes a problem |
| **benchstat** (Go) | CLI tool | Multi-run median + 95% CI | Methodology reference only (Go-specific) |

_Sources: [github-action-benchmark](https://github.com/benchmark-action/github-action-benchmark), [Bencher](https://bencher.dev/), [CodSpeed](https://codspeed.io/blog/benchmarks-in-ci-without-noise), [benchstat](https://pkg.go.dev/golang.org/x/perf/cmd/benchstat)_

---

## 8. Architectural Patterns for Benchmark Harness Design

### 8.1 Closed-Loop vs Open-Loop in a Single Framework

**k6's executor model is the best reference architecture.** k6 supports both models through different "executors":
- **Closed model:** `shared-iterations`, `per-vu-iterations`, `constant-vus`, `ramping-vus` — iterations start only after the previous finishes
- **Open model:** `constant-arrival-rate`, `ramping-arrival-rate` — iterations start at a fixed rate independent of completion, with a pool of pre-allocated workers

The executor is selected per-scenario, and multiple scenarios run in parallel. Pattern to adopt for fila-bench:

```rust
enum WorkloadMode {
    ClosedLoop { concurrency: usize },
    OpenLoop { target_rate: u64, max_workers: usize },
}
```

**Why both matter:** Marc Brooker's analysis shows that beyond coordinated omission, open-loop systems exhibit **congestive collapse** from timeouts and retries that closed benchmarks completely miss. But closed-loop benchmarks still have value for measuring maximum throughput (saturation point discovery).

_Sources: [k6 Open vs Closed Models](https://k6.io/docs/using-k6/scenarios/concepts/open-vs-closed/), [Marc Brooker — Open and Closed, Omission and Collapse](https://brooker.co.za/blog/2023/05/10/open-closed.html)_

### 8.2 Declarative Workload Profiles

**OMB's YAML workload model** is the reference for messaging benchmarks:
```yaml
name: "throughput-sweep-1kb"
topics: 1
messageSize: 1024
producerRate: 50000
testDurationMinutes: 15
```

**Recommended hybrid for fila-bench:** Define workload *profiles* declaratively (TOML) for the standard catalog, with programmatic escape hatch for complex multi-phase scenarios:

```toml
[workload]
name = "throughput-1kb"
mode = "closed-loop"
message_size = 1024
producers = 1
consumers = 1
warmup_seconds = 5
measurement_seconds = 30

[workload.latency-under-load]
name = "latency-at-80pct"
mode = "open-loop"
target_rate = 40000  # 80% of measured max
measurement_seconds = 60
percentiles = [50, 95, 99, 99.9, 99.99]
```

The declarative files become the "benchmark catalog" that CI runs automatically. New workloads can be added without code changes.

_Sources: [OMB Workloads](https://github.com/openmessaging/benchmark), [Gatling Simulation DSL](https://docs.gatling.io/concepts/simulation/)_

### 8.3 Orchestrator / Worker Separation

**OMB's driver/worker pattern:**
- **Orchestrator:** Creates queues, configures the benchmark, launches workers, manages phases (warmup → measurement → cooldown), collects/aggregates metrics
- **Workers:** Dumb loops that produce/consume and record per-operation latency into HdrHistogram

The orchestrator should be the **only** component that knows about benchmark phases. Workers should be stateless loops that run until told to stop.

For fila-bench, this means restructuring:

```
Orchestrator
├── Phase: Warmup (discard all worker metrics)
├── Phase: Measurement (collect worker histograms)
├── Phase: Cooldown (drain in-flight, final collection)
└── Aggregation (merge per-worker histograms, compute percentiles, emit report)

Workers (N concurrent)
├── Producer worker: sends at target rate, records send timestamps
└── Consumer worker: receives messages, records e2e latency
```

_Sources: [OMB Architecture](https://openmessaging.cloud/docs/benchmarks/), [Redpanda Benchmarking](https://docs.redpanda.com/current/develop/benchmark/)_

### 8.4 Warmup: Adaptive vs Fixed-Duration

**Fixed warmup problems:** Research shows developers frequently misconfigure warmup duration. A 2022 study found benchmarks often fail to reach steady state within configured warmup.

**Recommended hybrid approach:**
1. Set a **minimum** warmup duration (e.g., 5s) for cache/connection stabilization
2. Use a **sliding-window coefficient-of-variation (CV) check**: if the last N measurement windows have CV below threshold (e.g., 5%), warmup is done
3. Cap at a **maximum** warmup duration to prevent infinite warmup on noisy systems

This is simpler than full changepoint analysis and sufficient for system benchmarks where the warmup curve is typically monotonic.

_Sources: [Dynamically Reconfiguring Software Microbenchmarks (FSE '20)](https://www.ifi.uzh.ch/dam/jcr:2e51ad81-856f-4629-a6e2-67d382d337c2/fse20_author-version.pdf), [Tratt — VM Warmup](https://tratt.net/laurie/blog/2022/more_evidence_for_problems_in_vm_warmup.html)_

### 8.5 Built-In Self-Benchmark (Redpanda Model)

**Redpanda's `rpk cluster self-test`:** An embedded benchmark that tests hardware capabilities. Runs inside the cluster, reports p50/p90/p99/p999/max.

**Adoption for Fila:** A `fila bench` CLI command that runs a standardized workload profile and reports throughput/latency. Useful for:
- Users validating their deployment performance
- CI regression detection (same command, different environments)
- Quick smoke tests after configuration changes

This would be a thin wrapper around the fila-bench harness, invocable via the existing CLI binary.

_Source: [Redpanda rpk cluster self-test](https://docs.redpanda.com/current/reference/rpk/rpk-cluster/rpk-cluster-self-test-start/)_

### 8.6 Result Storage and Trending

Three tiers of sophistication:

| Tier | Approach | Regression Detection | Visualization | Effort |
|---|---|---|---|---|
| 1 | github-action-benchmark (file-based) | Simple threshold | Chart.js on gh-pages | Low |
| 2 | Bencher (self-hosted) | Change-point detection + Student's t-test | Web console, multi-dimensional | Medium |
| 3 | Prometheus + Grafana (ScyllaDB model) | Custom alerting rules | Arbitrary dashboards | High |

**Recommendation:** Start with Tier 1 (github-action-benchmark) layered on top of existing 3-run median pipeline. Move to Tier 2 (Bencher self-hosted) when statistical sophistication is needed. Skip Tier 3 unless operating dedicated benchmark infrastructure.

_Sources: [github-action-benchmark](https://github.com/benchmark-action/github-action-benchmark), [Bencher Architecture](https://bencher.dev/docs/reference/architecture/), [ScyllaDB Benchmarking](https://docs.scylladb.com/manual/stable/operating-scylla/benchmarking-scylla.html)_

---

## 9. Implementation Roadmap

### Phase 1: Foundation (Trust the Numbers)

**Goal:** Make existing benchmarks statistically meaningful. Minimal new workloads.

| Change | Effort | Impact |
|---|---|---|
| Replace `LatencySampler` with `hdrhistogram` crate | Small | Enables CO correction, histogram merging, arbitrary percentiles |
| Increase latency samples to 10,000+ | Trivial | Meaningful p99, enables p99.9 |
| Add p99.9, p99.99, max to all reports | Trivial | Reveals tail behavior hidden by p99 |
| Increase measurement duration to 30s (configurable) | Small | Captures sustained-state behavior |
| Run competitive benchmarks 3x with median | Trivial | Matches self-benchmark rigor |

**Validation:** Re-run existing benchmarks before/after. Results should be reproducible (median variance < 10% across 5 runs). If not, the measurement infrastructure has a problem.

### Phase 2: Methodology (Measure What Matters)

**Goal:** Add open-loop mode and workloads that reflect queue-broker reality.

| Change | Effort | Impact |
|---|---|---|
| Add open-loop load generation mode | Medium | Correct latency measurement under load |
| Switch competitive latency to concurrent produce/consume | Medium | Reveals real latency differences between brokers |
| Add consumer processing time simulation (0/1/10/100ms) | Small | Exposes prefetch and buffering effects |
| Add backpressure ramp test (10%→150% capacity) | Medium | Discovers saturation point and degradation curve |
| Add queue depth effect on latency | Small | Critical operational dimension |

**Validation:** Open-loop p99 should be significantly higher than closed-loop p99 under load. If not, either the system is genuinely fast or the open-loop implementation has a bug.

### Phase 3: Completeness (Cover Edge Cases)

**Goal:** Benchmark failure paths, fairness, and operational scenarios.

| Change | Effort | Impact |
|---|---|---|
| Add nack storm / DLQ routing benchmarks | Medium | Failure-path performance visibility |
| Add Jain's Fairness Index to fairness tests | Small | Standard metric for fair scheduling |
| Add many-client workload (500 connections × low rate) | Small | Connection management stress |
| Add disk I/O measurement | Small | Primary bottleneck for persistent brokers |
| Declarative workload profiles (TOML catalog) | Medium | New workloads without code changes |

### Phase 4: Infrastructure (CI and Trending)

**Goal:** Automated tracking and visualization across commits.

| Change | Effort | Impact |
|---|---|---|
| Emit github-action-benchmark JSON format | Small | Plugs into free CI visualization |
| Add gh-pages chart visualization | Small | Performance trending over time |
| Adaptive warmup (CV-based steady-state detection) | Medium | More reliable measurement start |
| Orchestrator/worker restructure | Large | Clean separation of concerns, enables distributed benchmarking later |

### Dependency Order

```
Phase 1 (Foundation)
   └─→ Phase 2 (Methodology)   ─→ Phase 3 (Completeness)
   └─→ Phase 4 (Infrastructure)
```

Phases 2 and 4 can run in parallel after Phase 1. Phase 3 depends on Phase 2 (needs open-loop mode for meaningful failure benchmarks).

---

## 10. Key Decisions to Make

Before implementing, the following decisions need Lucas's input:

1. **Open-loop vs CO-corrected closed-loop:** Should we implement full open-loop (interval-based sending) or use HdrHistogram's `record_correct()` on closed-loop measurements? Open-loop is more accurate but more complex. CO-corrected closed-loop is a pragmatic middle ground.

2. **Measurement duration tradeoff:** 30s default is a 10x increase from current 3s. CI time impact is significant (10 benchmarks × 30s = 5 min vs current 40s). Should CI run short (10s) with nightly long (60s)?

3. **Declarative workload files:** Worth the investment now, or keep workloads in code until the catalog is larger?

4. **github-action-benchmark vs Bencher:** Simple threshold alerting (github-action-benchmark) vs statistical regression detection (Bencher)? Bencher is more sophisticated but adds a dependency.

5. **`fila bench` CLI command:** Ship a user-facing self-benchmark command (Redpanda model), or keep benchmarks developer-only?

---

## Research Synthesis

### What We Learned

This research started with a simple question: can we trust our benchmark numbers enough to optimize against them? The answer is **no, not yet** — but the path to fixing that is clear and well-scoped.

The most important insight is that **coordinated omission** is not an edge case or academic concern. It's a systematic measurement error baked into our latency benchmarks (and into most vendor tools). Our closed-loop, 100-sample latency measurement is the methodological equivalent of measuring a building's height by looking at it from across the street — directionally useful, but not something to make engineering decisions on.

The second key insight is that **queue brokers need fundamentally different benchmarks than log brokers**. The industry's dominant benchmark frameworks and published methodologies are designed around Kafka-style throughput optimization. For Fila, per-message ack throughput, redelivery cost, fairness scheduling overhead, and behavior under consumer processing delay are all more important than raw produce throughput — and none of them are benchmarked by our current competitive suite.

The third insight is that **our foundation is strong**. The existing `fila-bench` architecture (custom harness, native clients for all brokers, reproducible Docker setup, CI regression detection) is the right approach. The gaps are in statistical methodology and workload coverage, not in infrastructure.

### What's Different About Fila

Unlike Kafka (log-based, batch-optimized) or NATS (fire-and-forget with optional persistence), Fila is a **queue-first broker with fair scheduling**. This means:

- **Ack throughput under contention** is the critical metric, not produce throughput
- **Fairness across tenants** (DRR) is a first-class concern that needs benchmarking with Jain's Fairness Index
- **Tail latency (p99.9+)** matters more than average throughput because p99 spikes trigger redelivery cascades
- **Consumer processing time** dramatically changes the performance profile — instant-ack benchmarks hide the dominant cost
- **Poison pill isolation** is a differentiator that should be benchmarked competitively

### Confidence Assessment

| Finding | Confidence | Basis |
|---|---|---|
| Coordinated omission in our latency measurements | **High** | Code review confirms closed-loop model; CO theory is well-established |
| 100 samples insufficient for p99 | **High** | Mathematical certainty: p99 from 100 samples = 1 data point |
| HdrHistogram Rust crate fits our needs | **High** | API review confirms CO correction, merging, arbitrary percentiles |
| Open-loop load generation pattern for gRPC | **Medium-High** | Pattern is proven (wrk2, k6); our Rust/tokio adaptation is untested |
| Queue-specific workload profiles needed | **High** | Multiple sources (RabbitMQ, SoftwareMill, academic papers) confirm |
| 3s measurement window too short | **Medium** | True for sustained-state effects, but may be sufficient for regression detection |
| github-action-benchmark as CI integration | **High** | Well-maintained, widely used, free, simple integration |

### What This Research Does NOT Cover

- **Specific performance optimization targets** — this research is about measurement methodology, not what to optimize
- **Distributed/cluster benchmark methodology** — all analysis assumes single-node; cluster benchmarking adds network partitioning, leader election, and replication overhead as additional dimensions
- **Cost-per-message economics** — important for competitive positioning but orthogonal to measurement methodology
- **Flame graph / profiling integration** — complementary to benchmarking but a separate tooling concern

---

**Research Completion Date:** 2026-03-23
**Research Period:** Comprehensive technical analysis with current web sources
**Source Verification:** All technical claims cited with current sources (30+ unique sources)
**Confidence Level:** High — based on multiple authoritative technical sources, cross-validated findings, and direct code review of existing benchmark suite

_This research document serves as the foundation for improving Fila's benchmark infrastructure to produce numbers we can trust and optimize against._
