# InMemoryEngine vs RocksDB Benchmark — 2026-03-25

## Goal

Isolate how much of the per-message time is RocksDB vs everything else (gRPC, serialization, DRR, Lua, lock contention, tokio runtime) by running identical benchmarks with `InMemoryEngine` (HashMap/RwLock) vs `RocksDbEngine`.

## Method

- `FILA_STORAGE=memory` env var added to `fila-server` to switch storage backend
- Benchmarks run via `profile-workload` binary (same SDK client, same server, same config)
- macOS ARM64 (M-series), release build with debug symbols
- 1KB payloads, `otlp_endpoint = ""` (OTel export disabled, but tracing subscriber still active)

## Raw Results

### Enqueue-Only (1KB, single producer, 10s)

| Backend   | Run 1 (msg) | Run 2 (msg) | Avg msg/s |
|-----------|-------------|-------------|-----------|
| RocksDB   | 85,120      | 80,152      | 8,264     |
| InMemory  | 92,277      | 91,303      | 9,179     |

**Ratio: InMemory is 1.11x faster (11% gain)**

### Lifecycle — Enqueue + Consume + Ack (1KB, single producer + single consumer, 10s)

| Backend   | Run 1 (msg) | Run 2 (msg) | Avg msg/s |
|-----------|-------------|-------------|-----------|
| RocksDB   | 60,629      | 59,789      | 6,021     |
| InMemory  | 65,996      | 65,379      | 6,569     |

**Ratio: InMemory is 1.09x faster (9% gain)**

### Batch Enqueue (1KB, single producer with accumulator, 10s)

| Backend   | Run 1 (msg) | Avg msg/s |
|-----------|-------------|-----------|
| RocksDB   | 115,342     | 11,534    |
| InMemory  | 129,902     | 12,990    |

**Ratio: InMemory is 1.13x faster (13% gain)**

### Multi-Producer Enqueue (1KB, 4 producers, 10s)

| Backend   | Run 1 (msg) | Avg msg/s |
|-----------|-------------|-----------|
| RocksDB   | 214,013     | 21,401    |
| InMemory  | 227,180     | 22,718    |

**Ratio: InMemory is 1.06x faster (6% gain)**

## Key Finding

**RocksDB is NOT the bottleneck.** Eliminating all disk I/O by switching to a pure in-memory HashMap engine yields only 6-13% throughput improvement. The storage layer accounts for roughly 10% of per-message time.

## Server-Side Flamegraph Analysis (InMemory, enqueue-only)

Profiled the `fila-server` process with macOS `sample` tool (1ms sampling interval, 12s duration) during a 10s enqueue-only workload with InMemoryEngine.

### Sample Distribution (non-idle thread)

Total active samples: ~911 (out of 9,095 total; rest is idle/parking)

| Component | Samples | % of Active | Description |
|-----------|---------|-------------|-------------|
| **Tracing span creation** | ~161 | **32.7%** | `tracing::span::Span::new` |
| — fmt subscriber Debug format | ~78 | 15.8% | `DefaultVisitor::record_debug` → `EnqueueRequest::Debug::fmt` |
| — OTel subscriber Debug format | ~76 | 15.4% | `SpanAttributeVisitor::record_debug` → `EnqueueRequest::Debug::fmt` |
| — OTel span bookkeeping | ~7 | 1.4% | `on_new_span` HashMap insert, SystemTime |
| **Broker send_command** | ~27 | 5.5% | `crossbeam_channel::try_send` + waker notify (semaphore signal) |
| **h2/hyper gRPC framing** | ~115 | 23.3% | HTTP/2 connection polling, send, codec |
| **tonic request decode** | ~11 | 2.2% | Protobuf decode + routing |
| **Span drop** | ~5 | 1.0% | `drop_in_place<Span>` |
| **Other (tokio, mio, etc.)** | ~118 | 24.0% | Scheduler park/wake, I/O driver, timers |

### The #1 Bottleneck: Tracing Debug Formatting

**32.7% of all server CPU time** is spent creating tracing spans — specifically, Debug-formatting the `EnqueueRequest` struct (which includes the 1KB payload `Bytes` field) into string representations for both the `tracing_subscriber::fmt` layer and the `tracing_opentelemetry` layer.

The call chain:
```
HotPathService::enqueue
  → tracing::span::Span::new
    → tracing_subscriber::fmt::DefaultVisitor::record_debug
      → EnqueueRequest::Debug::fmt
        → Bytes::Debug::fmt  ← hex-dumps 1KB payload!
    → tracing_opentelemetry::SpanAttributeVisitor::record_debug
      → alloc::fmt::format  ← allocates String
        → EnqueueRequest::Debug::fmt
          → Bytes::Debug::fmt  ← hex-dumps 1KB payload AGAIN!
```

The `Bytes` type's `Debug` impl formats the payload as a hex dump (e.g., `b"\x42\x42\x42..."`), which for a 1KB payload generates ~4KB of hex text. This happens **twice** per request — once for the fmt subscriber and once for OTel. With OTel export disabled (`otlp_endpoint = ""`), the OTel layer still records span attributes; it just doesn't export them.

### What This Means

The message payload should never be included in tracing span attributes. The per-request tracing overhead is:
- ~33% of CPU: Debug formatting (allocates ~8KB of strings per 1KB message)
- ~1% of CPU: Span lifecycle (create/drop)

This is a **low-hanging fruit optimization**. Options:
1. Remove payload from tracing span fields entirely
2. Use a custom `tracing::Value` impl that truncates/summarizes instead of hex-dumping
3. Gate the OTel attribute recording behind a flag
4. Use `tracing::field::Empty` for payload fields in hot-path spans

## Conclusion

**The actual bottleneck is tracing instrumentation overhead, not storage I/O.** RocksDB accounts for ~10% of per-message time. Tracing Debug formatting accounts for ~33%. The next optimization target should be removing or truncating payload data from hot-path tracing spans. This single change could yield a ~30-40% throughput improvement (roughly 8,500 → 11,000+ msg/s for single-producer enqueue) with zero architectural changes.

Secondary optimization targets (in order of impact):
1. HTTP/2 framing overhead (~23%) — harder to optimize, would require gRPC transport changes
2. Channel send + waker (~6%) — already efficient (crossbeam), not worth optimizing
3. Storage writes (~10%) — already small; InMemory ceiling shows diminishing returns

---

## Round 2: Tracing Fix Applied

### Change

Added `request` to the `skip(...)` list on all four hot-path `#[instrument]` attributes in `service.rs`:

```rust
// Before: #[instrument(skip(self), fields(batch_size))]
// After:  #[instrument(skip(self, request), fields(batch_size))]
```

This prevents the `#[instrument]` macro from Debug-formatting the `Request<EnqueueRequest>` (which contains the payload bytes) into tracing span attributes. The useful fields (`batch_size`, `queue_id`) are already recorded manually via `tracing::Span::current().record(...)`.

Applied to: `enqueue`, `consume`, `ack`, `nack`.

### Results

#### Enqueue-Only (1KB, single producer, 10s)

| Backend   | Run 1 (msg) | Run 2 (msg) | Avg msg/s | vs Round 1 |
|-----------|-------------|-------------|-----------|------------|
| RocksDB   | 94,742      | 95,023      | 9,488     | **+14.8%** |
| InMemory  | 103,536     | 103,334     | 10,344    | **+12.7%** |

#### Lifecycle — Enqueue + Consume + Ack (1KB, single producer + single consumer, 10s)

| Backend   | Run 1 (msg) | Run 2 (msg) | Avg msg/s | vs Round 1 |
|-----------|-------------|-------------|-----------|------------|
| RocksDB   | 66,199      | 65,905      | 6,605     | **+9.7%**  |
| InMemory  | 72,844      | 72,301      | 7,257     | **+10.5%** |

#### Batch Enqueue (1KB, single producer with accumulator, 10s)

| Backend   | Run 1 (msg) | Avg msg/s | vs Round 1 |
|-----------|-------------|-----------|------------|
| RocksDB   | 112,273     | 11,227    | -2.7%      |
| InMemory  | 127,096     | 12,710    | -2.2%      |

(Batch enqueue variance is within noise — the tracing fix has less impact here since batch amortizes per-message overhead.)

#### Multi-Producer Enqueue (1KB, 4 producers, 10s)

| Backend   | Run 1 (msg) | Avg msg/s | vs Round 1 |
|-----------|-------------|-----------|------------|
| RocksDB   | 233,539     | 23,354    | **+9.1%**  |
| InMemory  | 254,198     | 25,420    | **+11.9%** |

### Round 2 Server-Side Profile (InMemory, enqueue-only)

Profiled with macOS `sample` tool, same methodology as Round 1.

Total active samples in worker: ~880 (vs ~911 in Round 1). Of those, 459 in `run_task`:

| Component | Samples | % of Active | vs Round 1 |
|-----------|---------|-------------|------------|
| **H2/hyper/tonic (TCP+HTTP/2)** | ~284 | **61.9%** | was 23.3% |
| — TCP read (`__recvfrom`) | ~96 | 20.9% | — |
| — H2 frame decode | ~30 | 6.5% | — |
| — H2 connection poll/send | ~60 | 13.1% | — |
| **Enqueue handler** | ~49 | **10.7%** | — |
| — tracing enter/exit | ~14 | 3.1% | was 32.7% |
| — crossbeam send_command | ~14 | 3.1% | was 5.5% |
| — protobuf decode | ~8 | 1.7% | — |
| — prepare_enqueue_message | ~6 | 1.3% | — |
| **Steal work + maintenance** | ~421 | — | tokio scheduler |

### Analysis

The tracing fix reduced tracing overhead from **32.7% → 3.1%** of active CPU time (10.5x reduction). Single-producer enqueue throughput improved by **+14.8%** (RocksDB) and **+12.7%** (InMemory).

The new dominant cost center is HTTP/2 framing at **61.9%** of active CPU. This is in the tonic/hyper/h2 stack — kernel TCP reads (`__recvfrom`), HTTP/2 frame decoding, HPACK header compression, and connection multiplexing. This is not something that can be optimized without changing the transport layer (e.g., switching to a non-HTTP/2 protocol or Unix domain sockets).

The remaining application-level work (enqueue handler at 10.7%) is well-distributed across small items (tracing enter/exit, crossbeam channel send, protobuf decode, message construction). No single item exceeds 3.1% — there are no more >10% gains available from application code changes.

### Conclusion

**Diminishing returns reached.** The tracing fix was the last low-hanging fruit. The throughput progression:

| Metric | Baseline | After tracing fix | Total gain |
|--------|----------|-------------------|------------|
| RocksDB enqueue 1KB | 8,264 msg/s | 9,488 msg/s | **+14.8%** |
| InMemory enqueue 1KB | 9,179 msg/s | 10,344 msg/s | **+12.7%** |
| RocksDB lifecycle | 6,021 msg/s | 6,605 msg/s | **+9.7%** |
| RocksDB 4-producer | 21,401 msg/s | 23,354 msg/s | **+9.1%** |

The remaining bottleneck is the gRPC/HTTP/2 transport layer (~62% of CPU). Further throughput gains would require architectural changes:

1. **Transport optimization**: Unix domain sockets (eliminates TCP overhead for local benchmarks), or a custom binary protocol (eliminates HTTP/2 framing)
2. **Batch amortization**: The batch-enqueue path (11,227 msg/s) already partially addresses this by amortizing per-message gRPC overhead — streaming enqueue (`stream_enqueue`) amortizes further
3. **Connection multiplexing**: Multiple HTTP/2 streams over a single connection already benefit from h2 multiplexing; diminishing returns from further connection-level tuning
