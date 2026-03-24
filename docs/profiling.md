# Profiling Fila

This guide covers how to generate and interpret CPU flamegraphs for the Fila message broker.

## Prerequisites

### Install cargo-flamegraph

```bash
cargo install flamegraph
```

### Platform-specific setup

**macOS (DTrace)**

DTrace is built into macOS. No additional tools are needed, but System Integrity Protection (SIP) may limit profiling. If `cargo flamegraph` fails with permission errors:

1. Run with `sudo` (the script uses `--root` by default), or
2. Partially disable SIP for DTrace (see [Apple docs](https://developer.apple.com/documentation/security/disabling_and_enabling_system_integrity_protection))

**Linux (perf)**

Install the `perf` tool for your kernel:

```bash
# Ubuntu/Debian
sudo apt install linux-tools-common linux-tools-$(uname -r)

# Fedora
sudo dnf install perf
```

You may also need to adjust `perf_event_paranoid`:

```bash
sudo sysctl kernel.perf_event_paranoid=1
```

## Generating flamegraphs

### Quick start

```bash
# Default: enqueue-only workload, 1KB messages, 30 seconds
make flamegraph

# Or use the script directly with options
./scripts/flamegraph.sh --workload lifecycle --duration 60 --message-size 4096 --concurrency 4
```

### Available workloads

| Workload | Description |
|----------|-------------|
| `enqueue-only` | Pure enqueue throughput. Measures the write path: gRPC receive, storage write, DRR scheduling. |
| `consume-only` | Pre-fills the queue, then profiles consume + ack. Measures the read path: DRR delivery, gRPC streaming, ack processing. |
| `lifecycle` | Concurrent producers and consumers. Realistic mixed workload. |
| `batch-enqueue` | Enqueue with auto-batching enabled. Profiles the Nagle-style batcher and BatchEnqueue RPC path. |

### Script options

```
--workload NAME     enqueue-only|consume-only|lifecycle|batch-enqueue (default: enqueue-only)
--duration SECS     how long to run the workload (default: 30)
--message-size N    payload size in bytes (default: 1024)
--concurrency N     number of concurrent producers/consumers (default: 1)
--output PATH       output SVG path (default: target/flamegraphs/<workload>-<timestamp>.svg)
--help              show help
```

### Make targets

```bash
make flamegraph            # enqueue-only, 1KB, 30s
make flamegraph-lifecycle  # lifecycle, 1KB, 30s
make flamegraph-consume    # consume-only, 1KB, 30s
make flamegraph-batch      # batch-enqueue, 1KB, 30s
```

## Interpreting flamegraphs

Open the generated SVG in a browser. The flamegraph is interactive: click on a stack frame to zoom in, click "Reset Zoom" to return.

### Reading the graph

- **Width** = proportion of CPU time. Wider frames consumed more CPU.
- **Y-axis** = call stack depth. The bottom is the entry point, each layer up is a deeper function call.
- **Color** has no semantic meaning (it aids visual distinction).

### Common patterns to look for

**Wide RocksDB stacks**

If frames under `rocksdb::` dominate the flamegraph, the storage layer is the bottleneck. Look for:

- `rocksdb::DB::put` / `rocksdb::DB::write` — write path bound by disk I/O or WAL sync
- `rocksdb::DB::get` / `rocksdb::DB::multi_get` — read path bound by block cache misses
- `rocksdb::DB::iterator` — scan operations doing too much work

Possible actions: tune RocksDB block cache size, adjust write buffer settings, check if compaction is keeping up.

**Wide tonic/hyper stacks**

If frames under `tonic::` or `hyper::` are wide, gRPC serialization or HTTP/2 framing is significant. Look for:

- `prost::Message::encode` / `decode` — protobuf serialization overhead
- `h2::` — HTTP/2 frame processing
- `tonic::transport::` — connection management

Possible actions: enable batching to amortize per-message overhead, check if message payloads are unnecessarily large.

**Wide tokio stacks**

If frames under `tokio::runtime::` are wide, the async runtime is spending time on scheduling rather than application logic. Look for:

- `tokio::runtime::park` — idle time (good in low-load profiles, suspicious under high load)
- `tokio::sync::mpsc` — channel contention between tasks
- `tokio::task::waker` — excessive wakeups

Possible actions: reduce cross-task channel usage, batch work to reduce task switches.

**Wide mlua/Lua stacks**

If frames under `mlua::` are wide, Lua hook execution is a bottleneck. This is expected for queues with complex on_enqueue or on_failure hooks.

Possible actions: simplify Lua scripts, check if the circuit breaker is firing frequently.

**Allocation-heavy profiles**

If `alloc::` or `__rust_alloc` frames are wide, memory allocation is significant. Common sources:

- `Vec::push` with frequent reallocations — pre-allocate with `with_capacity`
- `String::from` / `to_string` on hot paths — consider `&str` or `Cow`
- `clone()` in tight loops — investigate zero-copy alternatives

## Comparing before/after

Generate flamegraphs before and after a change to visually compare:

```bash
# Before
./scripts/flamegraph.sh --workload enqueue-only --output target/flamegraphs/before.svg

# Make your changes...

# After
./scripts/flamegraph.sh --workload enqueue-only --output target/flamegraphs/after.svg
```

Open both SVGs side by side in browser tabs and look for frames that grew or shrank.

## Troubleshooting

### "dtrace: system integrity protection is on, some features will not be available"

This warning is normal on macOS. DTrace still works for basic profiling. If the resulting flamegraph is empty, you may need to run with `sudo`:

```bash
sudo ./scripts/flamegraph.sh --workload enqueue-only
```

### "perf_event_open: Permission denied"

On Linux, set:

```bash
sudo sysctl kernel.perf_event_paranoid=1
```

### Empty or very small flamegraph

The workload may have completed too quickly. Increase the duration:

```bash
./scripts/flamegraph.sh --duration 60
```

### "fila-server binary not found"

The script builds release binaries automatically, but if it fails, build manually:

```bash
cargo build --release --bin fila-server --bin profile-workload
```
