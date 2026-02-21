# Configuration Reference

Fila reads configuration from a TOML file. It searches for:

1. `fila.toml` in the current working directory
2. `/etc/fila/fila.toml`

If no file is found, all defaults are used. The broker runs with zero configuration.

## Environment variables

| Variable | Default | Description |
|----------|---------|-------------|
| `FILA_DATA_DIR` | `data` | Path to the RocksDB data directory |

## Full configuration

```toml
[server]
listen_addr = "0.0.0.0:5555"    # gRPC listen address

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
otlp_endpoint = "http://localhost:4317"  # OTLP gRPC endpoint (omit to disable)
service_name = "fila"                     # OTel service name
metrics_interval_ms = 10000              # metrics export interval (ms)
```

## Section reference

### `[server]`

| Key | Type | Default | Description |
|-----|------|---------|-------------|
| `listen_addr` | string | `"0.0.0.0:5555"` | Address and port for the gRPC server |

### `[scheduler]`

| Key | Type | Default | Description |
|-----|------|---------|-------------|
| `command_channel_capacity` | integer | `10000` | Size of the bounded channel between gRPC handlers and the scheduler loop. Increase if you see backpressure under high load. |
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
| `otlp_endpoint` | string | (none) | OTLP gRPC endpoint for exporting traces and metrics. Example: `"http://localhost:4317"`. |
| `service_name` | string | `"fila"` | Service name reported in OTel traces and metrics. |
| `metrics_interval_ms` | integer | `10000` | How often metrics are exported to the OTLP endpoint. |

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
