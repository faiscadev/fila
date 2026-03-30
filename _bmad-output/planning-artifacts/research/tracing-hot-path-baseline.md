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

### Server-Side Profile & Flamegraph

Captured using `profile-workload --flamegraph` which profiles fila-server with dtrace (macOS, requires sudo) or perf (Linux), collapses stacks and renders SVG via the inferno crate:

```bash
# Build first (never sudo cargo — it leaves root-owned artifacts in target/)
cargo build --release --bin profile-workload

# Generate flamegraph (macOS: sudo for dtrace, Linux: needs perf)
sudo ./target/release/profile-workload --flamegraph --duration 15

# Custom workload and output path
sudo ./target/release/profile-workload \
    --workload lifecycle --duration 30 --flamegraph target/flamegraphs/lifecycle.svg

# Just run workload without profiling (no sudo needed)
./target/release/profile-workload --duration 10
```

Output defaults to `target/flamegraphs/<workload>.svg`. See `--help` for all options.

The initial baseline capture below was taken with macOS `sample` + `inferno-collapse-sample` during a sustained 1KB enqueue workload (~6,131 msg/s).

**Flamegraph:** [`enqueue-flamegraph-baseline.svg`](enqueue-flamegraph-baseline.svg) — open in a browser for interactive zoom/search. Search for `Debug::fmt` or `record_debug` to highlight the tracing overhead.

#### tokio Worker Thread — Enqueue Handler (Thread_15452589)

| Call Site | Samples | % of Enqueue |
|-----------|---------|--------------|
| **Total active (run_task)** | **1,313** | — |
| → `EnqueueSvc::call` → `enqueue()` | 452 | 100% |
| → `tracing::span::Span::new` | 332 | **73.5%** |
|   → OTel `SpanAttributeVisitor::record_debug` | 157 | 34.7% |
|     → `EnqueueRequest::Debug::fmt` → byte formatting | 154 | 34.1% |
|   → fmt `DefaultVisitor::record_debug` | 165 | 36.5% |
|     → `EnqueueRequest::Debug::fmt` → byte formatting | 163 | 36.1% |
| → h2/hyper framing | ~120 | 26.5% |

**Key finding:** 73.5% of enqueue CPU time is spent in `tracing::span::Span::new`, almost entirely `Debug`-formatting the `Request<EnqueueRequest>` — specifically the `Vec<u8>` payload bytes formatted as decimal integers (`[116, 101, 115, ...]`). This happens **twice**: once for the OTel layer and once for the fmt layer.

The `core::fmt::num::Display for u8` → `Formatter::pad_integral` → `String::write_str` → `memmove` chain is clearly visible, confirming that byte-by-byte decimal formatting of the 1KB payload dominates CPU.

#### fila-scheduler Thread (Thread_15452617)

| Call Site | Samples | % of Active |
|-----------|---------|-------------|
| **Total active (handle_command)** | **897** | 100% |
| → `put_message` → `rocksdb_put_cf` | 585 | 65.2% |
|   → `WriteImpl` → WAL flush (`write()` syscall) | 423 | 47.2% |
|   → CRC32 checksumming | 9 | 1.0% |
| → crossbeam channel recv (idle spin/wait) | 8,429 | — |

RocksDB WAL write (specifically `PosixWritableFile::Append` → `write()` syscall) dominates the scheduler thread at 47.2% of active time. This confirms RocksDB I/O as the second major bottleneck after tracing overhead.

#### Summary: CPU Budget per Enqueue (Server-Side)

| Component | % of Total | Notes |
|-----------|-----------|-------|
| Tracing `Debug` formatting | ~35% | Formats request twice (OTel + fmt layers). **Fix: skip `request` param** |
| h2/hyper/tonic framing | ~13% | HTTP/2 frame encode/decode. Not actionable. |
| RocksDB WAL write | ~47% | Disk I/O on scheduler thread. Separate bottleneck. |
| Other (protobuf decode, DRR, etc.) | ~5% | Negligible |

## Optimization Targets for Story 18.2

### Fix: Use `skip(self, request)` Instead of `skip(self)` on Hot-Path Functions

```rust
// CURRENT (Debug-formats `request` including payload bytes):
#[instrument(skip(self), fields(queue_id, msg_id))]
async fn enqueue(&self, request: Request<EnqueueRequest>) -> ...

// FIX (skip both self and request, keep the existing empty fields filled via Span::current().record()):
#[instrument(skip(self, request), fields(queue_id, msg_id))]
async fn enqueue(&self, request: Request<EnqueueRequest>) -> ...
```

This change:
- Skips Debug-formatting of `request` (the expensive parameter containing payload bytes)
- Still captures any future parameters that might be added (unlike `skip_all` which silently drops new params)
- Keeps the existing `fields(queue_id, msg_id)` which are already filled via `Span::current().record()` in the function body
- Preserves observability — queue name and message ID are still recorded in spans
- Zero cost for the payload bytes that were being Debug-formatted
- **Only applies to hot-path functions** (enqueue, consume, ack, nack in `service.rs`). Admin functions should remain `skip(self)` since they are low-frequency and don't carry payload bytes.

### Measured Impact (Story 18.2)

Fix applied: `skip(self)` → `skip(self, request)` on 4 hot-path `#[instrument]` macros in `service.rs`. Admin functions unchanged.

| Metric | Baseline | After Fix | Delta |
|--------|----------|-----------|-------|
| enqueue_throughput_1kb | 6,697 msg/s | 7,861 msg/s | **+17.4%** |
| fairness_overhead_fifo | 2,196 msg/s | 2,178 msg/s | -0.8% |
| fairness_overhead_fair | 2,165 msg/s | 2,148 msg/s | -0.8% |
| fairness_overhead_pct | 1.41% | 1.37% | -0.04pp |
| key_cardinality_10 | 6,047 msg/s | 6,094 msg/s | +0.8% |
| key_cardinality_1k | 1,707 msg/s | 1,715 msg/s | +0.5% |
| consumer_concurrency_10 | 2,531 msg/s | 2,513 msg/s | -0.7% |

**Result: +17.4% enqueue throughput improvement**, exceeding the +15% hypothesis from the PRD. Latency numbers varied across runs (machine-local noise), so throughput is the reliable metric. Fairness and scaling metrics are unchanged within noise, confirming the fix only removes formatting overhead without affecting scheduling behavior.
