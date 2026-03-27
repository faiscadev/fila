# Configuration Reference

Fila reads configuration from a TOML file. It searches for:

1. `fila.toml` in the current working directory
2. `/etc/fila/fila.toml`

If no file is found, all defaults are used. The broker runs with zero configuration.

## Environment variables

| Variable | Default | Description |
|----------|---------|-------------|
| `FILA_DATA_DIR` | `data` | Path to the RocksDB data directory |
| `FILA_FIBP_PORT_FILE` | (none) | When set, the server writes the actual FIBP listen address to this file after binding (useful for test harnesses with port 0) |

## Full configuration

```toml
[server]
listen_addr = "0.0.0.0:5555"    # FIBP listen address

[scheduler]
command_channel_capacity = 10000 # internal command channel buffer size
idle_timeout_ms = 100            # scheduler idle timeout between rounds (ms)
quantum = 1000                   # DRR quantum per fairness key per round

[lua]
default_timeout_ms = 10              # max script execution time (ms)
default_memory_limit_bytes = 1048576 # max memory per script (1 MB)
circuit_breaker_threshold = 3        # consecutive failures before circuit break
circuit_breaker_cooldown_ms = 10000  # cooldown period after circuit break (ms)

[telemetry]
otlp_endpoint = "http://localhost:4317"  # OTLP endpoint (omit to disable)
service_name = "fila"                     # OTel service name
metrics_interval_ms = 10000              # metrics export interval (ms)

# Uncomment to enable the web management GUI
# [gui]
# listen_addr = "0.0.0.0:8080"  # HTTP port for the dashboard

# FIBP transport tuning (defaults are optimized — override only if needed)
# [fibp]
# listen_addr = "0.0.0.0:5555"       # TCP listen address
# max_frame_size = 16777216           # max frame size in bytes (16 MB)
# keepalive_interval_secs = 15        # heartbeat ping interval
# keepalive_timeout_secs = 10         # timeout waiting for pong
```

## Section reference

### `[server]`

| Key | Type | Default | Description |
|-----|------|---------|-------------|
| `listen_addr` | string | `"0.0.0.0:5555"` | Address and port for the FIBP server |

### `[scheduler]`

| Key | Type | Default | Description |
|-----|------|---------|-------------|
| `command_channel_capacity` | integer | `10000` | Size of the bounded channel between FIBP connection handlers and the scheduler loop. Increase if you see backpressure under high load. |
| `idle_timeout_ms` | integer | `100` | How long the scheduler waits when there's no work before checking again. Lower values reduce latency at the cost of CPU. |
| `quantum` | integer | `1000` | DRR quantum. Each fairness key gets `weight * quantum` deficit per scheduling round. Higher values mean more messages delivered per key per round (coarser interleaving). |

### `[lua]`

| Key | Type | Default | Description |
|-----|------|---------|-------------|
| `default_timeout_ms` | integer | `10` | Maximum execution time for Lua scripts. Enforced via instruction count hook (approximate). |
| `default_memory_limit_bytes` | integer | `1048576` (1 MB) | Maximum memory a Lua script can allocate. |
| `circuit_breaker_threshold` | integer | `3` | Number of consecutive Lua execution failures before the circuit breaker trips. When tripped, Lua hooks are bypassed and default scheduling is used. |
| `circuit_breaker_cooldown_ms` | integer | `10000` | How long to wait after circuit breaker trips before retrying Lua execution. |

### `[telemetry]`

Telemetry export is optional. When `otlp_endpoint` is omitted, the broker uses plain `tracing-subscriber` logging only.

| Key | Type | Default | Description |
|-----|------|---------|-------------|
| `otlp_endpoint` | string | (none) | OTLP endpoint for exporting traces and metrics. Example: `"http://localhost:4317"`. |
| `service_name` | string | `"fila"` | Service name reported in OTel traces and metrics. |
| `metrics_interval_ms` | integer | `10000` | How often metrics are exported to the OTLP endpoint. |

### `[gui]`

Web management GUI. Disabled by default. When enabled, serves a read-only dashboard on a separate HTTP port.

| Key | Type | Default | Description |
|-----|------|---------|-------------|
| `listen_addr` | string | `"0.0.0.0:8080"` | Address and port for the web dashboard HTTP server |

### `[fibp]`

FIBP (Fila Binary Protocol) transport configuration. FIBP is Fila's binary TCP protocol, using length-prefixed frames with a 6-byte header (flags, op code, correlation ID).

When `[tls]` is configured, FIBP connections are TLS-wrapped. When `[auth]` is configured, FIBP clients must send an `OP_AUTH` frame (containing the API key) as the first frame after handshake. Per-queue ACL checks apply to all data and admin operations.

FIBP supports all admin operations (create/delete queue, list queues, queue stats, redrive) using protobuf-encoded payloads for schema evolution.

| Key | Type | Default | Description |
|-----|------|---------|-------------|
| `listen_addr` | string | `"0.0.0.0:5557"` | TCP address for the FIBP transport |
| `max_frame_size` | integer | `16777216` (16 MB) | Maximum frame size in bytes. Frames exceeding this limit are rejected. |
| `keepalive_interval_secs` | integer | `15` | Interval between keepalive heartbeat pings. |
| `keepalive_timeout_secs` | integer | `10` | Timeout waiting for a keepalive pong before closing the connection. |

### SDK connection

The Rust SDK connects via FIBP:

```rust
let client = FilaClient::connect("127.0.0.1:5555").await?;

// With TLS and API key
let opts = ConnectOptions::new("127.0.0.1:5555")
    .with_tls()
    .with_api_key("my-key");
let client = FilaClient::connect_with_options(opts).await?;
```

Notes:
- The address is a raw TCP address (not a URL with `http://`).
- TLS and API key options are set on `ConnectOptions`.
- The FIBP wire format supports multi-message enqueue in a single frame.

## OpenTelemetry metrics

When telemetry is enabled, Fila exports the following metrics:

| Metric | Type | Description |
|--------|------|-------------|
| `fila.messages.enqueued` | Counter | Messages enqueued |
| `fila.messages.delivered` | Counter | Messages delivered to consumers |
| `fila.messages.acked` | Counter | Messages acknowledged |
| `fila.messages.nacked` | Counter | Messages rejected |
| `fila.messages.expired` | Counter | Messages expired (visibility timeout) |
| `fila.messages.dead_lettered` | Counter | Messages moved to DLQ |
| `fila.messages.redriven` | Counter | Messages redriven from DLQ |
| `fila.queue.depth` | Gauge | Pending messages per queue |
| `fila.queue.in_flight` | Gauge | Leased messages per queue |
| `fila.queue.consumers` | Gauge | Active consumers per queue |
| `fila.queue.fairness_keys` | Gauge | Active fairness keys per queue |
| `fila.delivery.latency` | Histogram | Time from enqueue to consumer delivery |
| `fila.lua.executions` | Counter | Lua script executions (by hook type and outcome) |
| `fila.throttle.limited` | Counter | Messages held due to throttle limits |

All queue-scoped metrics include a `queue` attribute.
