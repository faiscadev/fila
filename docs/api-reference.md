# API Reference

Fila exposes two gRPC services on the same port (default `5555`). Proto definitions are in [`proto/fila/v1/`](../proto/fila/v1/).

## Hot-path service (`fila.v1.FilaService`)

Used by producers and consumers for message operations.

### Enqueue

Add a message to a queue.

```protobuf
rpc Enqueue(EnqueueRequest) returns (EnqueueResponse)
```

**Request:**
| Field | Type | Description |
|-------|------|-------------|
| `queue` | string | Queue name |
| `headers` | map&lt;string, string&gt; | Arbitrary key-value headers (accessible in Lua hooks) |
| `payload` | bytes | Message body |

**Response:**
| Field | Type | Description |
|-------|------|-------------|
| `message_id` | string | UUID assigned to the message |

**Errors:**
| gRPC Status | Condition |
|-------------|-----------|
| `NOT_FOUND` | Queue does not exist |

### Consume

Open a server-streaming connection to receive messages. The broker delivers messages according to the DRR scheduler, respecting fairness groups and throttle limits.

```protobuf
rpc Consume(ConsumeRequest) returns (stream ConsumeResponse)
```

**Request:**
| Field | Type | Description |
|-------|------|-------------|
| `queue` | string | Queue name to consume from |

**Response (stream):**
| Field | Type | Description |
|-------|------|-------------|
| `message` | Message | The delivered message (see [Message](#message) below) |

The stream stays open until the client disconnects. Messages are delivered as they become available — the stream blocks when no messages are ready.

**Errors:**
| gRPC Status | Condition |
|-------------|-----------|
| `NOT_FOUND` | Queue does not exist |

### Ack

Acknowledge successful processing of a message. Removes the message from the broker.

```protobuf
rpc Ack(AckRequest) returns (AckResponse)
```

**Request:**
| Field | Type | Description |
|-------|------|-------------|
| `queue` | string | Queue name |
| `message_id` | string | ID of the message to acknowledge |

**Errors:**
| gRPC Status | Condition |
|-------------|-----------|
| `NOT_FOUND` | Queue or message does not exist |

### Nack

Reject a message. Triggers the `on_failure` Lua hook (if configured) to decide retry vs. dead-letter.

```protobuf
rpc Nack(NackRequest) returns (NackResponse)
```

**Request:**
| Field | Type | Description |
|-------|------|-------------|
| `queue` | string | Queue name |
| `message_id` | string | ID of the message to reject |
| `error` | string | Error description (passed to `on_failure` hook as `msg.error`) |

**Errors:**
| gRPC Status | Condition |
|-------------|-----------|
| `NOT_FOUND` | Queue or message does not exist |

---

## Admin service (`fila.v1.FilaAdmin`)

Used by operators and the `fila` CLI for queue management, configuration, and diagnostics.

### CreateQueue

Create a new queue with optional Lua hooks and visibility timeout.

```protobuf
rpc CreateQueue(CreateQueueRequest) returns (CreateQueueResponse)
```

**Request:**
| Field | Type | Description |
|-------|------|-------------|
| `name` | string | Queue name |
| `config` | QueueConfig | Optional configuration (see below) |

**QueueConfig:**
| Field | Type | Default | Description |
|-------|------|---------|-------------|
| `on_enqueue_script` | string | (none) | Lua script run on every enqueue |
| `on_failure_script` | string | (none) | Lua script run on every nack |
| `visibility_timeout_ms` | uint64 | 30000 | Lease duration in milliseconds |

**Response:**
| Field | Type | Description |
|-------|------|-------------|
| `queue_id` | string | Queue identifier |

**Errors:**
| gRPC Status | Condition |
|-------------|-----------|
| `ALREADY_EXISTS` | Queue with that name already exists |

### DeleteQueue

Delete a queue and all its messages.

```protobuf
rpc DeleteQueue(DeleteQueueRequest) returns (DeleteQueueResponse)
```

**Request:**
| Field | Type | Description |
|-------|------|-------------|
| `queue` | string | Queue name |

**Errors:**
| gRPC Status | Condition |
|-------------|-----------|
| `NOT_FOUND` | Queue does not exist |

### ListQueues

List all queues with summary statistics.

```protobuf
rpc ListQueues(ListQueuesRequest) returns (ListQueuesResponse)
```

**Response:**
| Field | Type | Description |
|-------|------|-------------|
| `queues` | repeated QueueInfo | List of queues |

**QueueInfo:**
| Field | Type | Description |
|-------|------|-------------|
| `name` | string | Queue name |
| `depth` | uint64 | Number of pending messages |
| `in_flight` | uint64 | Number of leased (in-flight) messages |
| `active_consumers` | uint32 | Number of connected consumers |

### SetConfig

Set a runtime configuration key-value pair. Persisted across restarts.

```protobuf
rpc SetConfig(SetConfigRequest) returns (SetConfigResponse)
```

**Request:**
| Field | Type | Description |
|-------|------|-------------|
| `key` | string | Configuration key |
| `value` | string | Configuration value |

### GetConfig

Retrieve a configuration value by key.

```protobuf
rpc GetConfig(GetConfigRequest) returns (GetConfigResponse)
```

**Request:**
| Field | Type | Description |
|-------|------|-------------|
| `key` | string | Configuration key |

**Response:**
| Field | Type | Description |
|-------|------|-------------|
| `value` | string | Configuration value |

**Errors:**
| gRPC Status | Condition |
|-------------|-----------|
| `NOT_FOUND` | Key does not exist |

### ListConfig

List configuration entries, optionally filtered by prefix.

```protobuf
rpc ListConfig(ListConfigRequest) returns (ListConfigResponse)
```

**Request:**
| Field | Type | Description |
|-------|------|-------------|
| `prefix` | string | Filter entries by key prefix (empty = all) |

**Response:**
| Field | Type | Description |
|-------|------|-------------|
| `entries` | repeated ConfigEntry | Key-value pairs |
| `total_count` | uint32 | Total number of matching entries |

**ConfigEntry:**
| Field | Type | Description |
|-------|------|-------------|
| `key` | string | Configuration key |
| `value` | string | Configuration value |

### GetStats

Get detailed statistics for a queue, including per-fairness-key and per-throttle-key breakdowns.

```protobuf
rpc GetStats(GetStatsRequest) returns (GetStatsResponse)
```

**Request:**
| Field | Type | Description |
|-------|------|-------------|
| `queue` | string | Queue name |

**Response:**
| Field | Type | Description |
|-------|------|-------------|
| `depth` | uint64 | Total pending messages |
| `in_flight` | uint64 | Messages currently leased |
| `active_fairness_keys` | uint64 | Number of fairness keys with pending messages |
| `active_consumers` | uint32 | Connected consumers |
| `quantum` | uint32 | DRR quantum value |
| `per_key_stats` | repeated PerFairnessKeyStats | Per-fairness-key breakdown |
| `per_throttle_stats` | repeated PerThrottleKeyStats | Per-throttle-key breakdown |

**PerFairnessKeyStats:**
| Field | Type | Description |
|-------|------|-------------|
| `key` | string | Fairness key |
| `pending_count` | uint64 | Pending messages for this key |
| `current_deficit` | int64 | Current DRR deficit |
| `weight` | uint32 | DRR weight |

**PerThrottleKeyStats:**
| Field | Type | Description |
|-------|------|-------------|
| `key` | string | Throttle key |
| `tokens` | double | Current available tokens |
| `rate_per_second` | double | Token refill rate |
| `burst` | double | Maximum bucket capacity |

**Errors:**
| gRPC Status | Condition |
|-------------|-----------|
| `NOT_FOUND` | Queue does not exist |

### Redrive

Move pending messages from a dead letter queue back to the source queue.

```protobuf
rpc Redrive(RedriveRequest) returns (RedriveResponse)
```

**Request:**
| Field | Type | Description |
|-------|------|-------------|
| `dlq_queue` | string | DLQ name (e.g., `orders.dlq`) |
| `count` | uint64 | Maximum number of messages to redrive |

**Response:**
| Field | Type | Description |
|-------|------|-------------|
| `redriven` | uint64 | Number of messages actually moved |

Only pending (non-leased) messages are redriven. Leased messages in the DLQ are skipped to avoid interfering with active consumers.

**Errors:**
| gRPC Status | Condition |
|-------------|-----------|
| `NOT_FOUND` | DLQ does not exist |

---

## Message types

### Message

The core message envelope returned by `Consume`.

```protobuf
message Message {
  string id = 1;
  map<string, string> headers = 2;
  bytes payload = 3;
  MessageMetadata metadata = 4;
  MessageTimestamps timestamps = 5;
}
```

| Field | Type | Description |
|-------|------|-------------|
| `id` | string | UUID assigned at enqueue |
| `headers` | map&lt;string, string&gt; | Headers set by the producer |
| `payload` | bytes | Message body |
| `metadata` | MessageMetadata | Broker-assigned scheduling metadata |
| `timestamps` | MessageTimestamps | Lifecycle timestamps |

### MessageMetadata

Scheduling metadata assigned by the broker (via Lua `on_enqueue` or defaults).

```protobuf
message MessageMetadata {
  string fairness_key = 1;
  uint32 weight = 2;
  repeated string throttle_keys = 3;
  uint32 attempt_count = 4;
  string queue_id = 5;
}
```

| Field | Type | Description |
|-------|------|-------------|
| `fairness_key` | string | DRR fairness group key |
| `weight` | uint32 | DRR weight for this key |
| `throttle_keys` | repeated string | Token bucket keys checked before delivery |
| `attempt_count` | uint32 | Number of delivery attempts |
| `queue_id` | string | Queue this message belongs to |

### MessageTimestamps

```protobuf
message MessageTimestamps {
  google.protobuf.Timestamp enqueued_at = 1;
  google.protobuf.Timestamp leased_at = 2;
}
```

| Field | Type | Description |
|-------|------|-------------|
| `enqueued_at` | Timestamp | When the message was first enqueued |
| `leased_at` | Timestamp | When the message was last delivered to a consumer |
