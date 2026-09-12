# Configuration Reference

Fila has three configuration layers, each with a different lifetime and audience.

| Layer | Set by | Takes effect | Visible to Lua |
|-------|--------|--------------|----------------|
| **File** — `fila.toml` | operator, at deploy | restart | no |
| **Environment** | orchestrator, per deployment | restart | no |
| **Runtime store** | admin API or CLI, live | immediately | yes, `fila.get(key)` |

Environment variables override the file. The runtime store is a separate namespace:
it holds operational policy, not boot parameters, and it is the only layer Lua reads.

## Conventions

- **Durations are strings with units** — `"30s"`, `"10ms"`, `"5m"`. Not bare
  integers with the unit buried in the field name; `visibility_timeout_ms = 30000`
  cannot change units without renaming the key.
- **Sizes are strings with units** — `"8MB"`, `"1MB"`.
- **Every key is overridable by environment variable**, upper-cased and prefixed
  with `FILA_`, sections joined by `_`. `[scheduler] quantum` becomes
  `FILA_SCHEDULER_QUANTUM`.

## File lookup

1. `fila.toml` in the current working directory
2. `/etc/fila/fila.toml`

If no file is found, all defaults apply. The broker runs with zero configuration.

## Full configuration

```toml
[server]
listen_addr = "0.0.0.0:5555"

[storage]
data_dir = "data"

[scheduler]
quantum = 1000                    # DRR deficit granted per weight unit, per round
command_channel_capacity = 10000  # bounded channel: protocol handlers → scheduler
idle_timeout = "100ms"            # wait before re-checking for work when idle

[queue]
visibility_timeout = "30s"        # default lease; overridable per queue at creation

[lua]
default_timeout = "10ms"
memory_limit = "1MB"
circuit_breaker_threshold = 3
circuit_breaker_cooldown = "10s"

[auth]
enabled = false
bootstrap_apikey = ""             # first credential; mint real keys, then remove

[tls]
cert_file = ""
key_file = ""
client_ca_file = ""               # set to require mTLS

[telemetry]
otlp_endpoint = ""                # OTLP/gRPC endpoint; empty disables export
service_name = "fila"
metrics_interval = "10s"
```

## Section reference

### `[server]`

| Key | Type | Default | Description |
|-----|------|---------|-------------|
| `listen_addr` | string | `"0.0.0.0:5555"` | Address and port for the binary protocol listener. |

### `[storage]`

| Key | Type | Default | Description |
|-----|------|---------|-------------|
| `data_dir` | string | `"data"` | Directory for the embedded storage engine. Also settable as `FILA_DATA_DIR`. |

### `[scheduler]`

| Key | Type | Default | Description |
|-----|------|---------|-------------|
| `quantum` | integer | `1000` | DRR quantum. Each fairness key receives `weight * quantum` deficit per round, and delivering a message costs one unit. Higher values deliver more per key per round, so interleaving is coarser. |

Deficit is counted in **messages**, not bytes. Classic DRR counts bytes, which is
fairer when payload sizes vary by orders of magnitude — a key sending 1 MB messages
and a key sending 100 B messages get equal message counts here, not equal bandwidth.
Message-counting is the right default for a work queue, where a message is a unit of
consumer effort rather than a unit of transfer. Revisit if payload sizes turn out to
be the thing that varies.

Do not confuse this with the delivery credit a consumer grants in `Consume`: DRR
deficit decides *which* key is served next, delivery credit decides *whether* the
consumer can take more at all.
| `command_channel_capacity` | integer | `10000` | Size of the bounded channel between protocol handlers and the scheduler loop. Raise if you observe backpressure under load. |
| `idle_timeout` | duration | `"100ms"` | How long the scheduler waits when there is no work. Lower values cut latency and cost CPU. |

### `[queue]`

| Key | Type | Default | Description |
|-----|------|---------|-------------|
| `visibility_timeout` | duration | `"30s"` | Default lease duration for delivered messages. A queue may override this at creation; a consumer may extend an individual lease. |

### `[lua]`

| Key | Type | Default | Description |
|-----|------|---------|-------------|
| `default_timeout` | duration | `"10ms"` | Maximum script execution time, enforced by instruction-count hook (approximate). Overridable per queue. |
| `memory_limit` | size | `"1MB"` | Maximum memory a script may allocate. Overridable per queue. |
| `circuit_breaker_threshold` | integer | `3` | Consecutive Lua failures before the breaker trips. While tripped, hooks are bypassed and default scheduling applies. |
| `circuit_breaker_cooldown` | duration | `"10s"` | How long to wait after tripping before retrying Lua execution. |

### `[auth]`

Authentication is disabled by default. When disabled, every connection is
implicitly superadmin.

| Key | Type | Default | Description |
|-----|------|---------|-------------|
| `enabled` | bool | `false` | Require an API key or client certificate on every connection. |
| `bootstrap_apikey` | string | (none) | A single credential that acts as superadmin, for minting the first real keys. Remove it once real keys exist. |

### `[tls]`

| Key | Type | Default | Description |
|-----|------|---------|-------------|
| `cert_file` | string | (none) | PEM server certificate. Setting this enables TLS. |
| `key_file` | string | (none) | PEM private key for `cert_file`. |
| `client_ca_file` | string | (none) | PEM CA bundle used to verify client certificates. Setting this requires mTLS. |

### `[telemetry]`

Optional. When `otlp_endpoint` is empty, the broker logs locally and exports nothing.

Telemetry export is the one place gRPC still appears, and it is unrelated to Fila's
own wire protocol — it is how OpenTelemetry collectors expect to be talked to.

| Key | Type | Default | Description |
|-----|------|---------|-------------|
| `otlp_endpoint` | string | (none) | OTLP/gRPC endpoint for exporting traces and metrics, e.g. `"http://localhost:4317"`. Collectors serve OTLP over gRPC on 4317 and over HTTP on 4318; Fila exports over gRPC. |
| `service_name` | string | `"fila"` | Service name reported in traces and metrics. |
| `metrics_interval` | duration | `"10s"` | Metrics export interval. |

## Runtime configuration

A flat key-value store, mutable while the broker runs and readable from Lua. This
is where operational policy lives — the values you change without a deploy.

```rust
admin.set_config("routing.default_region", "eu").await?;
let region = admin.get_config("routing.default_region").await?;
let all    = admin.list_config("routing.").await?;
```

```lua
function on_enqueue(msg)
  local region = fila.get("routing.default_region") or "us"
  return { fairness_key = msg.headers["tenant"] .. ":" .. region }
end
```

Namespace keys by prefix: `list_config("routing.")` returns every key under it.

Runtime configuration is replicated through the cluster's meta group, so every node —
and every Lua hook — reads the same value. See [clustering.md](clustering.md#the-meta-group).

Rate limits are not runtime configuration. Consumers declare them when they subscribe;
see [throttling.md](throttling.md).

## OpenTelemetry metrics

When telemetry is enabled, Fila exports:

| Metric | Type | Description |
|--------|------|-------------|
| `fila.messages.enqueued` | Counter | Messages enqueued |
| `fila.messages.delivered` | Counter | Messages delivered to consumers |
| `fila.messages.acked` | Counter | Messages acknowledged |
| `fila.messages.nacked` | Counter | Messages rejected |
| `fila.messages.expired` | Counter | Leases expired without an ack |
| `fila.messages.dead_lettered` | Counter | Messages moved to DLQ |
| `fila.messages.redriven` | Counter | Messages redriven from DLQ |
| `fila.queue.depth` | Gauge | Pending messages per queue |
| `fila.queue.in_flight` | Gauge | Leased messages per queue |
| `fila.queue.consumers` | Gauge | Active consumers per queue |
| `fila.queue.fairness_keys` | Gauge | Active fairness keys per queue |
| `fila.delivery.latency` | Histogram | Time from enqueue to consumer delivery |
| `fila.lua.executions` | Counter | Lua executions, by hook type and outcome |
| `fila.throttle.limited` | Counter | Messages held by a throttle limit |

All queue-scoped metrics carry a `queue` attribute.
