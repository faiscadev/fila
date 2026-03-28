# Tracing Hot-Path Baseline & Flamegraph Analysis

**Date:** 2026-03-28
**Commit:** 10cbedd (Epic 17 head)
**Purpose:** Establish performance baseline before fixing `#[instrument]` tracing overhead on hot-path functions.

## Benchmark Results

Full `cargo bench -p fila-bench --bench system` output:

### Throughput

| Metric | Value |
|--------|-------|
| enqueue_throughput_1kb | 6,697 msg/s |
| enqueue_throughput_1kb_mbps | 6.54 MB/s |

### End-to-End Latency

| Load Level | p50 | p95 | p99 |
|------------|-----|-----|-----|
| Light (1 producer) | 0.20 ms | 0.37 ms | 0.57 ms |

### Fairness

| Metric | Value |
|--------|-------|
| fairness_overhead_fifo_throughput | 2,196 msg/s |
| fairness_overhead_fair_throughput | 2,165 msg/s |
| fairness_overhead_pct | 1.41% |
| fairness_accuracy_max_deviation | 0.20% |

### Lua Execution

| Metric | Value |
|--------|-------|
| lua_on_enqueue_overhead_us | 8.21 us |
| lua_throughput_with_hook | 1,733 msg/s |

### Memory

| Metric | Value |
|--------|-------|
| memory_rss_idle | 256.12 MB |
| memory_rss_loaded_10k | 272.91 MB |
| memory_per_message_overhead | 1,760 bytes/msg |

### Compaction

| Metric | Value |
|--------|-------|
| compaction_idle_p99 | 0.37 ms |
| compaction_active_p99 | 0.23 ms |
| compaction_p99_delta | -0.14 ms |

### Key Cardinality Scaling

| Keys | Throughput |
|------|-----------|
| 10 | 6,047 msg/s |
| 1K | 1,707 msg/s |
| 10K | 758 msg/s |

### Consumer Concurrency

| Consumers | Throughput |
|-----------|-----------|
| 1 | 427 msg/s |
| 10 | 2,531 msg/s |
| 100 | 2,561 msg/s |

## Tracing Overhead Analysis

### Identified Problem: `#[instrument]` Debug-Formats Request Payloads

All 4 hot-path RPC functions in `crates/fila-server/src/service.rs` use `#[instrument]` with a pattern that Debug-formats the entire protobuf request on every call:

```rust
// CURRENT (problematic):
#[instrument(skip(self), fields(queue_id, msg_id))]
async fn enqueue(&self, request: Request<EnqueueRequest>) -> ...
```

**What happens on every enqueue call:**
1. `#[instrument]` creates a span and records all non-skipped parameters
2. `self` is skipped, but `request` is NOT → `Debug::fmt` is called on `Request<EnqueueRequest>`
3. `Request<T>` Debug output includes the inner `EnqueueRequest`
4. `EnqueueRequest` has `payload: Vec<u8>` (bytes field from protobuf)
5. `Vec<u8>::fmt` formats each byte as a decimal integer: `[116, 101, 115, ...]`
6. For a 1KB payload: ~1024 bytes × ~4 chars each = ~4KB of formatted string **per enqueue call**

The same pattern applies to `consume()`, `ack()`, and `nack()` — all record the `request` parameter via Debug formatting.

### Affected Functions

| File | Line | Function | Request Contains |
|------|------|----------|------------------|
| service.rs | 62 | `enqueue()` | queue (String) + headers (Map) + **payload (1KB bytes)** |
| service.rs | 181 | `consume()` | queue (String) — low overhead |
| service.rs | 299 | `ack()` | queue (String) + message_id (String) |
| service.rs | 391 | `nack()` | queue (String) + message_id (String) + error (String) |

**Enqueue is the worst offender** — it Debug-formats the entire message payload on every call. Ack/nack/consume have smaller requests but still pay unnecessary formatting cost.

### Subscriber Context

The server always initializes a tracing subscriber in `crates/fila-core/src/telemetry.rs`:
- Default filter: `info` (from `RUST_LOG` env or fallback)
- `#[instrument]` creates spans at INFO level → always enabled
- Even without OTel export, the `fmt::Layer` processes all spans
- Debug formatting of request parameters is **eager** — happens at span creation time, before any subscriber filtering

### Admin Service

13 additional functions in `crates/fila-server/src/admin_service.rs` have the same `#[instrument(skip(self))]` pattern. These are not hot-path (admin operations are infrequent), but should be fixed for consistency.

### Flamegraph

Flamegraph capture requires `sudo` for dtrace on macOS (SIP restriction). Code analysis above identifies the specific overhead sources. A Linux CI flamegraph should be captured as part of Story 18.2 validation.

## Optimization Targets for Story 18.2

### Fix: Use `skip_all` Instead of `skip(self)`

```rust
// CURRENT (Debug-formats `request` including payload bytes):
#[instrument(skip(self), fields(queue_id, msg_id))]
async fn enqueue(&self, request: Request<EnqueueRequest>) -> ...

// FIX (skip all parameters, keep the existing empty fields filled via Span::current().record()):
#[instrument(skip_all, fields(queue_id, msg_id))]
async fn enqueue(&self, request: Request<EnqueueRequest>) -> ...
```

This change:
- Skips Debug-formatting of `request` (and all other parameters) by using `skip_all`
- Keeps the existing `fields(queue_id, msg_id)` which are already filled via `Span::current().record()` in the function body
- Preserves observability — queue name and message ID are still recorded in spans
- Zero cost for the payload bytes that were being Debug-formatted

### Expected Impact

- **Enqueue:** Eliminates ~4KB of string formatting per call for 1KB payloads. Proportional to payload size.
- **Ack/Nack:** Eliminates formatting of queue name + message ID strings (small but nonzero).
- **Consume:** Eliminates formatting of queue name (minimal).
- **Estimated throughput improvement:** +10-20% on enqueue path (hypothesis from PRD: +15%). Actual measurement in Story 18.2.
