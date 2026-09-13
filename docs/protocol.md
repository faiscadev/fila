# Fila Binary Protocol Specification

Version: 1 (draft)

## Overview

Fila uses a custom binary protocol over TCP for all client-server communication.
The protocol is designed for:

- **Minimal overhead**: about 17 bytes amortized per message beyond payload in batch operations
- **Zero-copy parsing**: Length-prefixed frames — no delimiter scanning
- **Batch-native**: Every operation accepts multiple items; a single message is a batch of 1
- **Multiplexed**: Multiple concurrent requests on one connection via request IDs
- **Flow-controlled**: Consumers grant delivery credit; the server never outruns them
- **Streaming**: Server-push delivery for consume operations

The protocol replaces gRPC/HTTP2/protobuf with a purpose-built binary format
optimized for message broker workloads.

## Transport Layer

### TCP Connection

Clients connect on a configurable TCP port (default: 5555).

### TLS

TLS is optional and wraps the TCP connection using standard TLS 1.2+. When enabled:

- The server presents its certificate during the TLS handshake
- The client optionally presents a certificate for mTLS
- All subsequent protocol bytes flow over the encrypted channel

TLS negotiation completes before any protocol bytes are exchanged. The protocol
itself is TLS-agnostic.

## Frame Format

All communication uses length-prefixed frames:

```
+----------------+-------------------+
| Frame Length   | Frame Body        |
| (4 bytes, BE)  | (variable)        |
+----------------+-------------------+
```

- **Frame Length**: Big-endian `u32`. Byte count of the Frame Body, excluding the
  length prefix itself. Maximum frame size: 16 MiB (16,777,216 bytes) by default.

### Frame Header

Every Frame Body starts with a fixed 6-byte header:

```
+----------+----------+------------------+
| Opcode   | Flags    | Request ID       |
| (1 byte) | (1 byte) | (4 bytes, BE)    |
+----------+----------+------------------+
```

- **Opcode**: Identifies the operation.
- **Flags**: Bitfield.
  - Bit 0: **CONTINUATION** — this frame continues the previous frame with the same
    request ID and opcode. See [Continuation Frames](#continuation-frames).
  - Bits 1-7: Reserved, must be 0.
- **Request ID**: Big-endian `u32`, used to correlate responses.

### Request ID Space

The high bit partitions the ID space so both peers can originate requests without
colliding:

| Bit 31 | Originator | Range |
|--------|-----------|-------|
| `0` | Client-initiated | `0x00000001` – `0x7FFFFFFF` |
| `1` | Server-initiated | `0x80000001` – `0xFFFFFFFF` |

Request ID `0` is reserved for the handshake.

A response echoes the request ID of the request it answers, so a server response to
a client request keeps bit 31 clear. Server-*initiated* frames — an unsolicited
`Ping`, for example — set bit 31 and the client's `Pong` echoes it.

`Delivery` frames are neither: they carry the request ID of the `Consume`
subscription that asked for them, which is client-initiated and therefore has bit 31
clear.

### Total Frame Overhead

4 (length) + 1 (opcode) + 1 (flags) + 4 (request ID) = **10 bytes per frame**.

## Encoding Primitives

All multi-byte integers are big-endian (network byte order).

| Type | Encoding | Size |
|------|----------|------|
| `u8` | Raw byte | 1 |
| `u16` | Big-endian unsigned 16-bit | 2 |
| `u32` | Big-endian unsigned 32-bit | 4 |
| `u64` | Big-endian unsigned 64-bit | 8 |
| `i64` | Big-endian signed 64-bit | 8 |
| `f64` | Big-endian IEEE 754 double | 8 |
| `bool` | `0x00` = false, `0x01` = true | 1 |
| `uuid` | 16 raw bytes, big-endian (RFC 9562 byte order) | 16 |
| `string` | `[u16 length][UTF-8 bytes]` | 2 + length |
| `text` | `[u32 length][UTF-8 bytes]` | 4 + length |
| `bytes` | `[u32 length][raw bytes]` | 4 + length |
| `map<string,string>` | `[u16 count][repeated: string key, string value]` | 2 + entries |
| `string[]` | `[u16 count][repeated: string]` | 2 + strings |
| `optional<T>` | `[u8 present][T if present]` | 1, or 1 + sizeof(T) |

### On identifiers

Message IDs are UUIDv7 and travel as **16 raw bytes**, never as text. A UUID
rendered as a 36-character string costs 38 bytes on the wire — 22 wasted bytes on
every `Delivery`, `EnqueueResult`, `Ack`, `Nack` and `ExtendLease` item. Batch-acking
1,000 messages costs 24 KB rather than 46 KB.

Queue IDs and API key IDs remain `string`: they are operator-facing, appear only on
cold paths, and are not necessarily UUIDs.

### On string length

`string` is capped at 65,535 bytes. This is deliberate — queue names, fairness keys
and header values are short, and a `u32` prefix would add 2 bytes to every one of
them on the hot path.

Lua scripts use `text` (`u32`-prefixed) because they are cold-path and legitimately
large. Any field that can exceed 64 KB uses `text` or `bytes`, never `string`.

### On collection counts

Counts are sized to what the collection can actually hold, not uniformly:

| Collection | Count type | Why |
|------------|-----------|-----|
| Batch items (messages, acks, nacks) | `u32` | Bounded only by frame size |
| Admin list results (queues, keys, config, stats) | `u32` | Unbounded — a broker may hold millions of fairness keys |
| Per-message headers | `u16` | Bounded by practicality; hot path, and 2 saved bytes per message matters |

Sizing an admin list count at `u16` would cap the per-key breakdown at 65,535 while
`active_fairness_keys` reports a `u64` — a queue able to report more fairness keys
than it can enumerate. Fairness keys are per-tenant, so that cap would land squarely
on the feature the broker exists for.

## Opcode Table

### Control Opcodes (0x00-0x0F)

| Opcode | Name | Direction | Description |
|--------|------|-----------|-------------|
| `0x01` | Handshake | Client → Server | Connection initialization |
| `0x02` | HandshakeOk | Server → Client | Handshake accepted |
| `0x03` | Ping | Either → Either | Keepalive probe |
| `0x04` | Pong | Either → Either | Keepalive response |
| `0x05` | Disconnect | Either → Either | Graceful close |

### Hot-Path Opcodes (0x10-0x1F)

| Opcode | Name | Direction | Description |
|--------|------|-----------|-------------|
| `0x10` | Enqueue | Client → Server | Enqueue a batch of messages |
| `0x11` | EnqueueResult | Server → Client | Per-message enqueue results |
| `0x12` | Consume | Client → Server | Subscribe to delivery |
| `0x13` | ConsumeOk | Server → Client | Subscription accepted |
| `0x14` | Delivery | Server → Client | Batch of messages pushed to a consumer |
| `0x15` | Credit | Client → Server | Grant additional delivery credit |
| `0x16` | CancelConsume | Client → Server | Unsubscribe |
| `0x17` | Ack | Client → Server | Acknowledge a batch |
| `0x18` | AckResult | Server → Client | Per-message ack results |
| `0x19` | Nack | Client → Server | Negative-acknowledge a batch |
| `0x1A` | NackResult | Server → Client | Per-message nack results |
| `0x1B` | ExtendLease | Client → Server | Extend leases on in-flight messages |
| `0x1C` | ExtendLeaseResult | Server → Client | Per-message new expiry |

### Error Opcode (0xFE)

| Opcode | Name | Direction | Description |
|--------|------|-----------|-------------|
| `0xFE` | Error | Server → Client | Request-level error |

### Admin Opcodes (0xFD downward)

Admin opcodes grow downward from `0xFD` so hot-path and admin ranges expand
independently without colliding.

| Opcode | Name | Direction | Description |
|--------|------|-----------|-------------|
| `0xFD` | CreateQueue | Client → Server | Create a queue |
| `0xFC` | CreateQueueResult | Server → Client | Creation result |
| `0xFB` | DeleteQueue | Client → Server | Delete a queue |
| `0xFA` | DeleteQueueResult | Server → Client | Deletion result |
| `0xF9` | GetStats | Client → Server | Queue statistics |
| `0xF8` | GetStatsResult | Server → Client | Statistics |
| `0xF7` | ListQueues | Client → Server | List queues |
| `0xF6` | ListQueuesResult | Server → Client | Queue list |
| `0xF5` | SetConfig | Client → Server | Set a runtime config key |
| `0xF4` | SetConfigResult | Server → Client | Result |
| `0xF3` | GetConfig | Client → Server | Read a runtime config key |
| `0xF2` | GetConfigResult | Server → Client | Value |
| `0xF1` | ListConfig | Client → Server | List config by prefix |
| `0xF0` | ListConfigResult | Server → Client | Entries |
| `0xEF` | Redrive | Client → Server | Redrive DLQ messages |
| `0xEE` | RedriveResult | Server → Client | Redriven count |
| `0xED` | CreateApiKey | Client → Server | Create an API key |
| `0xEC` | CreateApiKeyResult | Server → Client | Result |
| `0xEB` | RevokeApiKey | Client → Server | Revoke an API key |
| `0xEA` | RevokeApiKeyResult | Server → Client | Result |
| `0xE9` | ListApiKeys | Client → Server | List API keys |
| `0xE8` | ListApiKeysResult | Server → Client | Key list |
| `0xE7` | SetAcl | Client → Server | Replace a key's permissions |
| `0xE6` | SetAclResult | Server → Client | Result |
| `0xE5` | GetAcl | Client → Server | Read a key's permissions |
| `0xE4` | GetAclResult | Server → Client | Permissions |

Opcodes `0x1D`–`0xE3` are reserved. Nodes talk to each other over a separate protocol
with its own opcode space; see [Inter-node Communication](#inter-node-communication).

### Handling Unknown Opcodes

Symmetric "ignore what you don't know" is wrong for responses — silently dropping an
unrecognized reply leaves the request hanging forever. The rule depends on whether
the frame answers something:

| Situation | Behavior |
|-----------|----------|
| Server receives an unknown request opcode | Respond `Error` with `InvalidFrame`, keep the connection |
| Client receives an unknown opcode whose request ID matches a **pending request** | Fail that request with `InvalidFrame`. Do not ignore it. |
| Client receives an unknown opcode with no matching pending request | Ignore the frame. This is how server-initiated extensions stay forward-compatible. |

## Error Codes

Errors arrive either as an `Error` frame (request-level failure) or inline in a
per-item result array (batch item failure).

| Code | Name | Description |
|------|------|-------------|
| `0x00` | Ok | Success (per-item results only) |
| `0x01` | QueueNotFound | Queue does not exist |
| `0x02` | MessageNotFound | Message ID not found, or its lease already ended |
| `0x03` | QueueAlreadyExists | Queue name is taken |
| `0x04` | LuaCompilationError | Script failed to compile |
| `0x05` | StorageError | Storage engine failure |
| `0x06` | NotADLQ | Queue is not a dead-letter queue |
| `0x07` | ParentQueueNotFound | DLQ's parent queue is missing |
| `0x08` | InvalidConfigValue | Config value rejected |
| `0x09` | ChannelFull | Server overloaded; back off |
| `0x0A` | Unauthorized | Missing, invalid, expired or revoked credential |
| `0x0B` | Forbidden | Authenticated, but the ACL denies this |
| `0x0C` | NotLeader | Not the leader for this queue; see `leader_addr` metadata |
| `0x0D` | UnsupportedVersion | Protocol version not supported |
| `0x0E` | InvalidFrame | Malformed, oversized or unparseable frame |
| `0x0F` | ApiKeyNotFound | API key ID does not exist |
| `0x10` | NodeNotReady | No leader elected yet |
| `0x11` | CreditExhausted | Delivery credit is zero; grant more |
| `0x12` | ThrottleConflict | A throttle with this name is already declared with a different partition |
| `0x13` | ScriptError | The queue's `on_enqueue` script failed on this message; retrying the same message will not help |
| `0x14` | ScriptTimeout | The queue's `on_enqueue` script timed out on this message; retrying may help |
| `0x15` | OrderingKeyMissing | The message has no value for the queue's ordering key, and the queue rejects such messages |
| `0x16` | ReservedQueueName | Queue names ending in `.dlq` are reserved for dead-letter queues |
| `0xFF` | InternalError | Unexpected server error |

## Connection Lifecycle

### 1. TCP Connect (+ Optional TLS)

### 2. Handshake

The client's first frame after connecting must be a `Handshake`. No other frame may
precede it.

**Handshake (0x01)** — Client → Server:

```
[frame header: opcode=0x01, flags=0, request_id=0]
[u16: protocol_version]              -- highest version the client speaks
[optional<string>: api_key]
[u32: client_capabilities]           -- bitmap; see Capabilities
```

**HandshakeOk (0x02)** — Server → Client:

```
[frame header: opcode=0x02, flags=0, request_id=0]
[u16: negotiated_version]
[u64: node_id]                       -- 0 if single-node
[u32: max_frame_size]                -- 0 = default 16 MiB
[u32: server_capabilities]           -- bitmap
```

On rejection the server sends an `Error` frame and closes the connection.

#### Capabilities

The **active** capability set is the bitwise AND of both bitmaps. Either side may
advertise a bit the other lacks; the feature is simply off. This lets optional
features ship without a version bump.

| Bit | Name | Meaning |
|-----|------|---------|
| 0 | `CREDIT_FLOW_CONTROL` | Peer honours `Credit`. When inactive, `Consume` credit is ignored and the server pushes freely. |
| 1-31 | Reserved | Must be 0 |

#### A note on the handshake's own evolution

The `Handshake` frame is sent before a version is agreed, so it can never safely
gain a field: the server does not yet know which layout to expect. The capability
bitmap is the escape hatch — extensions go in capability-gated frames after the
handshake, not in the handshake itself. Keep it frozen.

### 3. Request/Response

The client sends requests; the server replies with the matching result frame,
correlated by request ID. Multiple requests may be in flight.

### 4. Consume Streaming

After a `Consume` subscription the server pushes `Delivery` frames as messages
become ready **and as credit permits**. The client acks, nacks or extends leases on
the same connection. `CancelConsume` stops delivery.

### 5. Keepalive

Either side may send `Ping` at any time; the receiver responds `Pong` echoing the
request ID. Server-initiated pings use the server ID range (bit 31 set). If no
`Pong` arrives within 30 seconds, close the connection.

### 6. Disconnect

Either side sends `Disconnect`, then closes. The peer finishes in-flight responses
and closes.

## Flow Control

A server that pushes whenever messages are ready needs a brake the consumer
controls. Pausing TCP reads at an internal high-water mark is not it: that applies
backpressure at the wrong layer and is invisible to the sender, which keeps
producing work that has nowhere to go.

Delivery is therefore governed by **credit**, granted by the consumer and spent by
the server.

- `Consume` carries an initial credit in messages. `0` means unlimited, which
  the right choice for a consumer that acks immediately.
- The server decrements credit by one per message placed in a `Delivery` frame.
- At zero credit the server stops delivering and holds the messages. It does **not**
  error; the subscription stays open.
- The client sends `Credit` to grant more.

Credit is per subscription, not per connection. A client with two subscriptions
manages two independent credit balances.

Servers must not deliver on zero credit even if messages are ready. A client that
wants a bounded number of unacked messages sets credit to that bound and grants one
more per ack.

`CreditExhausted` (`0x11`) is never sent for a normal zero balance — it exists for
the case where a client's own accounting has diverged from the server's, so the
disagreement surfaces instead of hanging.

## Hot-Path Operation Frames

### Enqueue (0x10)

Each message names its own queue, so one frame may target several queues. The server
applies per-queue ACL checks and routes each message independently, including to the
correct Raft group in cluster mode. Partial success is normal.

**Request:**

```
[frame header: opcode=0x10]
[u32: message_count]
For each message:
  [string: queue]
  [map<string,string>: headers]
  [bytes: payload]
  [optional<string>: fairness_key]   -- absent = queue default
  [optional<u32>: weight]            -- absent = queue default (1)
  [optional<u64>: delay_ms]          -- absent or 0 = deliverable immediately
```

**EnqueueResult (0x11):**

```
[frame header: opcode=0x11]
[u32: result_count]
For each result:
  [u8: error_code]
  [uuid: message_id]                 -- all-zero if error
```

Results are in request order.

A message can be rejected individually: `ScriptError` or `ScriptTimeout` when the queue
rejects messages its script fails on, `OrderingKeyMissing` when it is an ordered queue
that rejects messages without a key. On a queue that parks script failures, the message
is accepted and a message ID is returned. See
[concepts.md](concepts.md#when-on_enqueue-fails).

#### Scheduling metadata precedence

`fairness_key` and `weight` may be set directly, so the scheduler's defining feature
does not require writing a Lua script. When a queue **also** has an
`on_enqueue` hook, the hook wins for every field it returns.

This ordering is a security property, not a preference. The hook is operator-authored
server-side policy; the client-supplied value is a claim. If clients could override
the hook, any tenant could set `fairness_key` to another tenant's key and take their
share of delivery bandwidth. Client-supplied values are therefore **defaults for
queues without a hook**, and suggestions for queues with one.

#### Delayed delivery

`delay_ms` makes a message ineligible for delivery until that interval has elapsed.
Delayed messages count toward queue depth and are visible to `GetStats`, but the
scheduler will not select them. They do not consume delivery credit while waiting.

On an ordered queue, a delayed message holds its ordering group: nothing behind it in
the group is delivered first. See [ordering.md](ordering.md#delayed-messages).

### Consume (0x12)

**Request:**

```
[frame header: opcode=0x12]
[string: queue]
[u32: credit]                        -- initial delivery credit; 0 = unlimited
```

If this node is not the leader for the queue, the server replies `Error` with
`NotLeader` (`0x0C`) and a `leader_addr` metadata entry.

#### Throttle declarations

A subscription may declare throttles — named rate limits, optionally partitioned per
message — that pace delivery for the queue. See [throttling.md](throttling.md) for the
model.

**Their encoding is not yet specified.** A declaration reusing a throttle name with a
different partition is rejected with `ThrottleConflict` (`0x12`).

### ConsumeOk (0x13)

Sent before any `Delivery` frame.

```
[frame header: opcode=0x13]
[string: consumer_id]
```

### Delivery (0x14)

Pushed to a consuming client, using the request ID of the `Consume` subscription.

```
[frame header: opcode=0x14, request_id=<consume_request_id>]
[u32: message_count]
For each message:
  [uuid: message_id]
  [string: queue]
  [map<string,string>: headers]
  [bytes: payload]
  [string: fairness_key]
  [u32: weight]
  [u32: attempt_count]               -- 1 on first delivery
  [u64: enqueued_at]                 -- Unix ms
  [u64: leased_at]                   -- Unix ms
  [u64: lease_expires_at]            -- Unix ms
```

`lease_expires_at` is sent because the client otherwise cannot know when to call
`ExtendLease` — the visibility timeout is queue configuration the consumer has no
reason to have fetched, and it may be changed by an operator mid-stream.

### Credit (0x15)

Grant additional delivery credit to an existing subscription.

**Request:**

```
[frame header: opcode=0x15, request_id=<consume_request_id>]
[u32: additional_credit]
```

Credit is additive and saturates at `u32::MAX`. No response frame is sent. Sending
`Credit` for an unknown subscription is ignored.

### CancelConsume (0x16)

```
[frame header: opcode=0x16, request_id=<consume_request_id>]
```

The server stops delivering and releases the subscription. No response frame.

### Ack (0x17)

```
[frame header: opcode=0x17]
[u32: item_count]
For each item:
  [string: queue]
  [uuid: message_id]
```

**AckResult (0x18):**

```
[frame header: opcode=0x18]
[u32: result_count]
For each result:
  [u8: error_code]                   -- 0x00 Ok, 0x02 MessageNotFound
```

`queue` is carried per item even though `message_id` is globally unique. In cluster
mode it routes the ack to the owning Raft group without a lookup, and it lets one
frame ack across several queues — the same property that makes cross-queue batching
work on `Enqueue`.

### Nack (0x19)

```
[frame header: opcode=0x19]
[u32: item_count]
For each item:
  [string: queue]
  [uuid: message_id]
  [string: error]                    -- reaches the on_failure hook as msg.error
  [optional<u64>: retry_after_ms]    -- absent = let on_failure decide
```

**NackResult (0x1A):**

```
[frame header: opcode=0x1A]
[u32: result_count]
For each result:
  [u8: error_code]                   -- 0x00 Ok, 0x02 MessageNotFound
```

When `retry_after_ms` is present the message is retried no sooner than that
interval, overriding any delay the `on_failure` hook or the retry policy's backoff would
apply. The retry still counts as an attempt. The hook still decides
**whether** to retry or dead-letter; the client only overrides **when**. A client
holding a `Retry-After` from a rate-limited upstream knows the correct delay in a
way the broker cannot.

On an ordered queue, the retrying message holds its ordering group for the delay.

Without a backoff mechanism every failure retries immediately, and one failing
dependency becomes a hot loop.

### ExtendLease (0x1B)

```
[frame header: opcode=0x1B]
[u32: item_count]
For each item:
  [string: queue]
  [uuid: message_id]
  [u64: extend_by_ms]                -- from now, not from current expiry
```

**ExtendLeaseResult (0x1C):**

```
[frame header: opcode=0x1C]
[u32: result_count]
For each result:
  [u8: error_code]                   -- 0x00 Ok, 0x02 MessageNotFound
  [u64: lease_expires_at]            -- new expiry, Unix ms; 0 if error
```

Extension is measured from receipt, not from the current expiry, so a client that
heartbeats on a fixed interval cannot accumulate unbounded lease time by racing.

`MessageNotFound` here means the lease already expired and the message was
redelivered. The client should stop work: another consumer may hold it now.

## Admin Operation Frames

### CreateQueue (0xFD)

```
[frame header: opcode=0xFD]
[string: name]
[optional<text>: on_enqueue_script]
[optional<text>: on_failure_script]
[u64: visibility_timeout_ms]         -- 0 = server default
```

Scripts use `text` (`u32`-prefixed) rather than `string`; a 64 KB ceiling on
user-authored Lua is an arbitrary limit with no reason behind it.

Creating a queue also creates its dead-letter queue, `<name>.dlq`. A name ending in `.dlq`
is rejected with `ReservedQueueName` (`0x16`).

**Not yet specified:** the encoding of the ordering key and its missing-key policy
([ordering.md](ordering.md)), of the script failure policy with its optional dead-letter
threshold ([concepts.md](concepts.md#when-on_enqueue-fails)), and of the retry policy
([concepts.md](concepts.md#retry-policy)). The ordering key and the script failure policy
are fixed at creation.

**CreateQueueResult (0xFC):**

```
[frame header: opcode=0xFC]
[u8: error_code]
[string: queue_id]                   -- empty if error
```

### DeleteQueue (0xFB)

```
[frame header: opcode=0xFB]
[string: queue]
```

**DeleteQueueResult (0xFA):**

```
[frame header: opcode=0xFA]
[u8: error_code]
```

### GetStats (0xF9)

```
[frame header: opcode=0xF9]
[string: queue]
```

**GetStatsResult (0xF8):**

```
[frame header: opcode=0xF8]
[u8: error_code]
[u64: depth]
[u64: in_flight]
[u64: delayed]                       -- enqueued but not yet eligible
[u64: unclassified]                  -- parked after a script failure
[u64: oldest_unclassified_at]        -- Unix ms; 0 if none
[u64: active_fairness_keys]
[u32: active_consumers]
[u32: quantum]
[u64: leader_node_id]                -- 0 if single-node
[u32: replication_count]             -- 0 if single-node
[u32: per_key_stats_count]
For each fairness key stat:
  [string: key]
  [u64: pending_count]
  [i64: current_deficit]
  [u32: weight]
```

Throttle statistics are not yet specified. A partitioned throttle can have too many
buckets to list, so what a queue reports about its throttles is an open question in
[throttling.md](throttling.md#open-questions).

### ListQueues (0xF7)

```
[frame header: opcode=0xF7]
```

**ListQueuesResult (0xF6):**

```
[frame header: opcode=0xF6]
[u8: error_code]
[u32: cluster_node_count]            -- 0 if single-node
[u32: queue_count]
For each queue:
  [string: name]
  [u64: depth]
  [u64: in_flight]
  [u32: active_consumers]
  [u64: leader_node_id]              -- 0 if single-node
```

### SetConfig (0xF5)

```
[frame header: opcode=0xF5]
[string: key]
[string: value]
```

**SetConfigResult (0xF4):**

```
[frame header: opcode=0xF4]
[u8: error_code]
```

### GetConfig (0xF3)

```
[frame header: opcode=0xF3]
[string: key]
```

**GetConfigResult (0xF2):**

```
[frame header: opcode=0xF2]
[u8: error_code]                     -- 0x08 InvalidConfigValue if key is unset
[string: value]                      -- empty if error
```

### ListConfig (0xF1)

```
[frame header: opcode=0xF1]
[string: prefix]                     -- empty = all entries
```

**ListConfigResult (0xF0):**

```
[frame header: opcode=0xF0]
[u8: error_code]
[u32: entry_count]
For each entry:
  [string: key]
  [string: value]
```

### Redrive (0xEF)

```
[frame header: opcode=0xEF]
[string: dlq_queue]
[u64: count]                         -- 0 = all
```

**RedriveResult (0xEE):**

```
[frame header: opcode=0xEE]
[u8: error_code]
[u64: redriven]
```

### CreateApiKey (0xED)

```
[frame header: opcode=0xED]
[string: name]
[u64: expires_at_ms]                 -- Unix ms, 0 = never
[bool: is_superadmin]
```

**CreateApiKeyResult (0xEC):**

```
[frame header: opcode=0xEC]
[u8: error_code]
[string: key_id]
[string: key]                        -- plaintext secret, returned once
[bool: is_superadmin]
```

### RevokeApiKey (0xEB)

```
[frame header: opcode=0xEB]
[string: key_id]
```

**RevokeApiKeyResult (0xEA):**

```
[frame header: opcode=0xEA]
[u8: error_code]                     -- 0x0F = ApiKeyNotFound
```

### ListApiKeys (0xE9)

```
[frame header: opcode=0xE9]
```

**ListApiKeysResult (0xE8):**

```
[frame header: opcode=0xE8]
[u8: error_code]
[u32: key_count]
For each key:
  [string: key_id]
  [string: name]
  [u64: created_at_ms]
  [u64: expires_at_ms]               -- 0 = never
  [bool: is_superadmin]
```

### SetAcl (0xE7)

```
[frame header: opcode=0xE7]
[string: key_id]
[u32: permission_count]
For each permission:
  [string: kind]                     -- "produce" | "consume" | "admin"
  [string: pattern]                  -- glob over queue names
```

Replaces the permission set; it does not merge.

**SetAclResult (0xE6):**

```
[frame header: opcode=0xE6]
[u8: error_code]                     -- 0x0F ApiKeyNotFound, 0x08 invalid kind
```

### GetAcl (0xE5)

```
[frame header: opcode=0xE5]
[string: key_id]
```

**GetAclResult (0xE4):**

```
[frame header: opcode=0xE4]
[u8: error_code]                     -- 0x0F = ApiKeyNotFound
[string: key_id]
[bool: is_superadmin]
[u32: permission_count]
For each permission:
  [string: kind]
  [string: pattern]
```

## Error Frame (0xFE)

```
[frame header: opcode=0xFE, request_id=<failed request's ID>]
[u8: error_code]
[string: message]                    -- human-readable
[map<string,string>: metadata]       -- machine-readable context
```

Standard metadata keys:

| Error Code | Key | Value | Description |
|------------|-----|-------|-------------|
| `0x0C` NotLeader | `leader_addr` | `"host:port"` | Current leader |
| `0x09` ChannelFull | `retry_after_ms` | `"100"` | Suggested backoff |
| `0x0D` UnsupportedVersion | `max_version` | `"1"` | Highest version supported |
| `0x11` CreditExhausted | `server_credit` | `"0"` | Server's view of the balance |

SDKs should expose the metadata map to callers. Unknown keys must be preserved, not
discarded.

## Continuation Frames

Payloads and bodies larger than the maximum frame size are split with the
CONTINUATION flag.

### How It Works

1. The sender serializes the **entire operation body** — everything after the 6-byte
   frame header — into one buffer.
2. If it fits in a frame, send it with CONTINUATION=0.
3. Otherwise, send each chunk with CONTINUATION=1, same opcode and request ID, and
   the final chunk with CONTINUATION=0.

### Receiver Behavior

On CONTINUATION=1 the receiver buffers the body (excluding the header) and keeps
buffering until a frame with the same request ID and opcode arrives with
CONTINUATION=0. It concatenates the buffers in order and parses the result as one
body.

The split is at **raw byte boundaries** — any field may be cut mid-value. This is
deliberate: the sender needs no knowledge of field structure to chunk, and the
receiver needs none to reassemble.

### Rules

- All continuation frames for a request share one opcode and request ID
- Receivers may enforce a maximum reassembled size; there is no protocol-level cap
- Frames from different request IDs may interleave; continuation state is per request ID
- A connection closing mid-continuation discards the partial data

## Overhead Analysis

### Single Enqueue (batch of 1)

1 KB payload to queue `orders`, no headers, no scheduling metadata:

| Component | Bytes |
|-----------|-------|
| Frame length prefix | 4 |
| Opcode + Flags + Request ID | 6 |
| Message count (u32) | 4 |
| Queue string (2 + 6) | 8 |
| Headers map (count=0) | 2 |
| Payload (4 + 1024) | 1028 |
| fairness_key (absent) | 1 |
| weight (absent) | 1 |
| delay_ms (absent) | 1 |
| **Total** | **1055** |
| **Overhead beyond payload** | **31 bytes** |

The three optional scheduling fields cost 3 bytes when unused — the price of making
fairness settable without Lua.

### Batch Enqueue (100 messages, same queue)

| Component | Bytes |
|-----------|-------|
| Frame length + header + count | 14 |
| 100x queue string | 800 |
| 100x headers map (empty) | 200 |
| 100x payload (4 + 1024) | 102,800 |
| 100x scheduling fields (unused) | 300 |
| **Total** | **104,114** |
| **Per-message overhead** | **17.14 bytes** |

### Batch Ack (1,000 messages)

An ack item is a queue name and an ID — there is no payload to amortize the encoding
against, which is why the ID is 16 raw bytes.

| Component | Bytes |
|-----------|------:|
| Queue string (2 + 6) | 8 |
| Message ID (`uuid`) | 16 |
| **Per item** | **24** |
| **1,000 items** | **24,000** |

Encoding the same ID as a 36-character string would cost 46 bytes per item, and
46,000 for the batch.

### Comparison with gRPC/Protobuf

| Protocol | Single 1 KB Enqueue | Notes |
|----------|--------------------:|-------|
| Fila binary | ~31 bytes overhead | Length-prefixed, no HTTP/2 |
| gRPC/HTTP2/protobuf | ~100-200 bytes | HTTP/2 HEADERS + DATA, protobuf tags, HPACK |

## Serialization Format Decision

### Options Evaluated

| Format | Per-field overhead | Zero-copy | Cross-language | Schema evolution |
|--------|--------------------|-----------|----------------|------------------|
| **Hand-rolled binary** | 0 (fixed layout) | Yes | Manual per SDK | Append at end |
| msgpack | 1-5 B/field | No | Excellent | Via key-value maps |
| bincode | 0 (fixed layout) | Limited | Rust only | Poor |
| postcard | 0-2 B (varint) | Limited | Rust only | Poor |
| FlatBuffers | vtable overhead | Yes | Good | Via vtable |
| Cap'n Proto | pointer overhead | Yes | Limited | Via pointer evolution |

### Decision: Hand-Rolled Binary with Fixed Layouts

1. **Minimal overhead.** Fixed layouts mean no per-field encoding cost. Kafka, Redis
   RESP3 and the NATS client protocol all made this choice for the same reason.
2. **Zero-copy payloads.** `[u32 length][raw bytes]` lets a reader reference the
   payload without copying it.
3. **Implementable from this document alone.** Big-endian integers and
   length-prefixed strings need no library, no schema file, no code generation.
4. **Schema evolution by appending.** New fields go at the end of a body; readers
   ignore trailing bytes they do not recognize.

**Trade-off:** changing field order or type within an opcode requires a version bump.
Layout is worth settling before an implementation exists to be compatible with.

## Schema Evolution

### Adding Fields

Append to the end of an opcode's body. Readers must tolerate trailing bytes they do
not understand. Writers must not omit fields present in the negotiated version.

### Removing Fields

Fields are never removed. Deprecated fields carry zero or empty values.

### Optional Features

Prefer a capability bit over a version bump. Versions are for layout changes;
capabilities are for behaviour that either side may not implement.

### Protocol Versioning

The handshake negotiates a version: the server picks the highest it supports that is
≤ the client's. If none exists it rejects with `UnsupportedVersion` and a
`max_version` metadata entry.

Version 1 is the initial version.

## Inter-node Communication

Nodes in a cluster talk to each other over a separate protocol, with its own opcode
space and its own version. It shares this document's frame format and encoding
primitives and nothing else, and it is not part of this specification.

The client protocol is a public contract that is never broken; the inter-node protocol
connects nodes deployed together and may change between releases. See
[clustering.md](clustering.md#inter-node-protocol).

## Implementation Notes

### Backpressure

Two mechanisms, at different layers, for different problems:

- **Credit** bounds how much the *server* pushes to a consumer. It is the consumer's
  brake, and it is explicit in the protocol.
- **`ChannelFull` (`0x09`)** signals that the server's internal command channel is
  saturated by *inbound* work. Clients should back off exponentially, honouring
  `retry_after_ms` when present.

Neither replaces the other. Credit is the consumer's; `ChannelFull` is the producer's.

### Connection Pooling

Clients may open several connections. Request IDs are per connection, not global.
SDKs should default to one connection with multiplexed requests, and multiple
subscriptions on it.

### Consume and Ack on the Same Connection

Subscribing and acking on one connection is the expected pattern. Ack, Nack and
ExtendLease request IDs are independent of the Consume request ID.

### Maximum Frame Size

Default 16 MiB, configurable, advertised in `HandshakeOk`. The length field is `u32`
(~4 GiB), but a lower enforced limit bounds per-frame allocation. This does not limit
payload size — larger bodies use continuation frames. A server receiving an oversized
frame responds `InvalidFrame` (`0x0E`).

### Byte Order

All multi-byte integers are big-endian (network byte order) — consistent, and it
matches Wireshark's default display.
