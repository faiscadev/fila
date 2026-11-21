# Fila Binary Protocol Specification

Version: 1 (draft)

## Overview

Fila uses a custom binary protocol over TCP for all client-server communication.
The protocol is designed for:

- **Minimal overhead**: about 18 bytes amortized per message beyond payload in batch operations
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
  - Bits 1-7: Reserved, must be 0. A reader **rejects** a frame that sets one rather
    than ignoring it: a flag changes how the frame itself is read, so misparsing a body
    is worse than refusing it. A reserved bit is therefore also where an extended header
    would be announced — gated by a capability, so neither side ever sets it against a
    peer that would refuse the frame.
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
| `string8` | `[u8 length][UTF-8 bytes]` | 1 + length |
| `string32` | `[u32 length][UTF-8 bytes]` | 4 + length |
| `bytes` | `[u32 length][raw bytes]` | 4 + length |
| `map<string8,string8>` | `[u16 count][repeated: string8 key, string8 value]` | 2 + entries |
| `string8[]` | `[u16 count][repeated: string8]` | 2 + strings |
| `optional<T>` | `[u8 present][T if present]` | 1, or 1 + sizeof(T) |
| `key` | `[u16 part_count][repeated: u8 kind, string8 name]` | 2 + parts |

A **`key`** names message properties that together identify a group — an ordering group,
a throttle bucket, or a set of duplicates. Each part's `kind` is:

| Kind | Part | `name` |
|------|------|--------|
| `0` | A header | The header name |
| `1` | The fairness key | Empty |
| `2` | An attribute returned by `on_enqueue` | The attribute name |
| `3` | The producer's idempotency key | Empty |
| `4` | A hash of the payload | Empty |
| `5` | A hash of every header | Empty |

Kinds `3`–`5` are valid only in deduplication keys. A `part_count` of 0 means no key.

### On identifiers

Message IDs are UUIDv7 and travel as **16 raw bytes**, never as text. A UUID
rendered as a 36-character string costs 37 bytes on the wire — 21 wasted bytes on
every `Delivery`, `EnqueueResult`, `Ack`, `Nack` and `ExtendLease` item. Batch-acking
1,000 messages costs 23 KB rather than 44 KB.

Queue IDs and API key IDs remain `string8`: they are operator-facing, appear only on
cold paths, and are not necessarily UUIDs.

### On string length

There are two string types, named for the width of their length prefix so that nobody
has to remember which is larger.

**`string8` carries identifiers and labels** — queue names, fairness keys, API keys,
config and header keys and values, patterns, addresses. Every one of them is a name or
a short value, so a 255-byte ceiling is ample and the 1-byte prefix keeps it off the
hot path's budget. Two fields are user-supplied and therefore the only places the
ceiling can realistically be met: **header values** and **runtime config values**. A
header carries routing metadata — a W3C `traceparent` is 55 bytes, a tenant id a
dozen — and anything larger belongs in the payload.

**`string32` carries UTF-8 text that is not an identifier**, which today means Lua
scripts. The width is not a capability: the 4 GiB a `u32` could describe is never the
operative limit, because a frame is capped long before that and a reassembled body is
capped by the receiver. It is simply the type for text that needs more than 255 bytes,
on a cold path where four prefix bytes cost nothing.

Bounding `string8` at 255 bytes also bounds the handshake. Version 1's longest legal
`Handshake` is:

```
4 prefix + 6 header + 2 version + (1 present + 1 length + 255 api_key) + 4 capabilities = 273 bytes
```

The limit a server enforces before a handshake is **1 KiB**, not 273 — see
[Maximum Frame Size](#maximum-frame-size). What matters is the distance from 16 MiB;
the headroom above version 1 keeps a server from refusing a layout it could otherwise
have negotiated down.

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
| `0xE3` | GetThrottleStats | Client → Server | Statistics for one throttle |
| `0xE2` | GetThrottleStatsResult | Server → Client | Throttle statistics |
| `0xE1` | GetThrottleBucket | Client → Server | Statistics for one throttle bucket |
| `0xE0` | GetThrottleBucketResult | Server → Client | Bucket statistics |
| `0xDF` | UpdateQueue | Client → Server | Change a queue's mutable settings |
| `0xDE` | UpdateQueueResult | Server → Client | Result |
| `0xDD` | GetRoutes | Client → Server | Which node leads each queue |
| `0xDC` | GetRoutesResult | Server → Client | Leaders, and shards when sharded |
| `0xDB` | DrainNode | Client → Server | Move every leader off a node, or cancel |
| `0xDA` | DrainNodeResult | Server → Client | Result |
| `0xD9` | MoveQueueLeader | Client → Server | Move one queue's leadership |
| `0xD8` | MoveQueueLeaderResult | Server → Client | Result |
| `0xD7` | SetRebalancing | Client → Server | Enable, pause or disable rebalancing |
| `0xD6` | SetRebalancingResult | Server → Client | Result |
| `0xD5` | GetClusterStatus | Client → Server | Nodes, meta leader, rebalancing state |
| `0xD4` | GetClusterStatusResult | Server → Client | Cluster status |

Opcodes `0x1D`–`0xD3` are reserved. Nodes talk to each other over a separate protocol
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
| `0x12` | ThrottleConflict | A throttle with this name is already declared by other subscribers with a different key |
| `0x13` | ScriptError | The queue's `on_enqueue` script failed on this message; retrying the same message will not help |
| `0x14` | ScriptTimeout | The queue's `on_enqueue` script timed out on this message; retrying may help |
| `0x15` | OrderingKeyMissing | The message has no value for the queue's ordering key, and the queue rejects such messages |
| `0x16` | ReservedQueueName | Queue names ending in `.dlq` are reserved for dead-letter queues |
| `0x17` | InvalidThrottle | A throttle declaration is malformed: no limits, a zero limit or window, an empty name, a name repeated within one subscription, or a limit that can never apply |
| `0x18` | ThrottleNotFound | No current subscriber declares a throttle with this name |
| `0x19` | AuthStateStale | This node cannot confirm its API keys and ACLs are current, and refuses to answer on stale state |
| `0x1A` | ImmutableSetting | An `UpdateQueue` tried to change a setting that is fixed at creation |
| `0x1B` | NodeNotFound | No node in the cluster has this ID |
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
[optional<string8>: api_key]
[u32: client_capabilities]           -- bitmap; see Capabilities
```

Trailing bytes are tolerated here as everywhere else, which is how this frame gains a
field: a newer client appends one, an older server skips it. The sender learns whether
it was actually read from the capability bit it set to announce it — an ignored field
and an understood one are otherwise indistinguishable. What bounds an unauthenticated
peer is the size limit in [Maximum Frame Size](#maximum-frame-size), not a rule about
extra bytes; refusing them would buy nothing and close the cheapest way this frame has
to change.

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
| 1-31 | Unassigned | Ignored |

**An unassigned bit is ignored, never rejected.** A reader that refuses a bitmap it
does not fully recognise makes every later feature unshippable: a newer client could
not handshake with an older server, which is the one case the AND exists to handle. A
bit is a peer's claim about itself, and disregarding a claim costs the reader nothing.

This is the opposite of the rule for reserved [frame flags](#frame-header), which are
rejected. The two are distinguished in [Flags and capabilities](#flags-and-capabilities).

A retired capability's bit is never reused; it stays spent, as a deprecated field does.
When the 31 are gone, a bit announcing an extended capabilities frame buys more.

#### Flags and capabilities

Both are bitfields, and they answer different questions. A flag describes **this
frame**; a capability describes **this peer**.

| | Flags | Capabilities |
|---|---|---|
| Where | the 6-byte frame header | the handshake body |
| Width | `u8`, on every frame | `u32`, once per connection |
| Scope | that one frame | the whole connection |
| Set by | the sender, unilaterally | both sides; the active set is the AND |
| An unknown bit | is rejected | is ignored |
| Cost | 1 byte per frame | 4 bytes per connection |

`CONTINUATION` is a flag because whether a frame is a fragment differs frame to frame,
and a reader that misreads it stitches the body wrong. `CREDIT_FLOW_CONTROL` is a
capability because it is a standing fact about a peer, and learning it means hearing
from the other side.

Neither substitutes for the other. A flag cannot be negotiated: the sender asserts it
and learns nothing about whether the receiver copes. A capability cannot vary per
frame: it is agreed once, so it can say "I am able to fragment" but never "this frame
is a fragment".

A feature may need both, and continuation will. The flag marks each fragment; a
capability is how a peer that has not implemented reassembly declines to receive any.

#### A note on the handshake's own evolution

`protocol_version` is the first field of the body, and **its offset never changes in
any version of this protocol**. That is what lets a reader decide whether it knows a
handshake before parsing one: two bytes in, it either recognises the version or
answers with the range it supports.

So this frame evolves like any other, with one extra cost. Appending a field is free —
an older peer skips the trailing bytes, and a capability bit announcing the field tells
the sender whether it was read. Removing, retyping or reordering a field needs a new
version, and for this frame alone that costs a reconnect on every skew: a reader
without version *N*'s layout cannot parse a version *N* body, so it can only reply with
its range and close. Every other frame is negotiated before it is sent and pays
nothing.

Prefer a capability bit regardless. This is the frame every connection pays for and
every SDK writes first.

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
  [string8: queue]
  [map<string8,string8>: headers]
  [bytes: payload]
  [optional<string8>: fairness_key]   -- absent = queue default
  [optional<u32>: weight]            -- absent = queue default (1)
  [optional<u64>: delay_ms]          -- absent or 0 = deliverable immediately
  [optional<string8>: idempotency_key]
```

**EnqueueResult (0x11):**

```
[frame header: opcode=0x11]
[u32: result_count]
For each result:
  [u8: error_code]
  [uuid: message_id]                 -- all-zero if error; the original's ID for a duplicate
  [bool: duplicate]                  -- matched a remembered deduplication key; nothing was enqueued
[u16: route_hint_count]
For each hint:
  [string8: queue]
  [u64: leader_node_id]
  [string8: leader_addr]
```

A **route hint** is sent for each queue in the batch that this node does not lead and
therefore forwarded. Clients cache hints and send later messages for those queues straight
to their leaders; forwarding stays available for when a hint is stale. See
[clustering.md](clustering.md#routing).

Results are in request order.

A message whose deduplication key matches one the queue still remembers is not enqueued
again: its result is `Ok` with the original message's ID and `duplicate` set. See
[deduplication.md](deduplication.md).

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

`delay_ms` makes a message ineligible for delivery before `enqueued_at + delay_ms`. That
not-before time survives a redrive; retry delays do not.
Delayed messages count toward queue depth and are visible to `GetStats`, but the
scheduler will not select them. They do not consume delivery credit while waiting.

On an ordered queue, a delayed message holds its ordering group: nothing behind it in
the group is delivered first. See [ordering.md](ordering.md#delayed-messages).

### Consume (0x12)

**Request:**

```
[frame header: opcode=0x12]
[string8: queue]
[u32: credit]                        -- initial delivery credit; 0 = unlimited
[u16: throttle_count]
For each throttle:
  [string8: name]
  [u16: limit_count]                 -- at least 1
  For each limit:
    [u64: max_deliveries]            -- N
    [u64: window_ms]                 -- W
  [key: key]                         -- part_count 0 = one bucket for all messages
  [u8: when_key_missing]             -- 0 = shared bucket, 1 = unthrottled
```

If this node is not the leader for the queue, the server replies `Error` with
`NotLeader` (`0x0C`) and a `leader_addr` metadata entry.

#### Throttle declarations

A subscription may declare throttles: named limits of at most `max_deliveries` in any
sliding window of `window_ms`, optionally keyed per message. See
[throttling.md](throttling.md) for the model.

The whole `Consume` fails with an `Error` frame if a declaration is malformed
(`InvalidThrottle`, `0x17`) or uses a name already declared by other subscribers with a
different key (`ThrottleConflict`, `0x12`). Declarations cannot be changed on a live
subscription; changing them means subscribing again.

`max_deliveries` is a `u64` because it is a limit rather than a collection count, and the
cost is paid once per subscription.

### ConsumeOk (0x13)

Sent before any `Delivery` frame.

```
[frame header: opcode=0x13]
[string8: consumer_id]
```

### Delivery (0x14)

Pushed to a consuming client, using the request ID of the `Consume` subscription.

```
[frame header: opcode=0x14, request_id=<consume_request_id>]
[u32: message_count]
For each message:
  [uuid: message_id]
  [string8: queue]
  [map<string8,string8>: headers]
  [bytes: payload]
  [string8: fairness_key]
  [u32: weight]
  [u32: attempt_count]               -- 1 on first delivery; reset by redrive
  [u32: redrive_count]               -- times redriven from the dead-letter queue
  [u64: enqueued_at]                 -- Unix ms
  [u64: leased_at]                   -- Unix ms
  [u64: lease_expires_at]            -- Unix ms
```

`lease_expires_at` is sent because the client otherwise cannot know when to call
`ExtendLease` — the visibility timeout is queue configuration the consumer has no
reason to have fetched, and it may be changed by an operator mid-stream.

It is a lower bound: a leader change can restart a lease at its full duration, and never
shortens one. See [clustering.md](clustering.md#delivery-durability).

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
  [string8: queue]
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
  [string8: queue]
  [uuid: message_id]
  [string8: error]                    -- reaches the on_failure hook as msg.error
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
  [string8: queue]
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

On a queue with `Fast` delivery durability, an `ExtendLease` for a lease the leader does
not know, received during the reclaim grace period after a crash, re-establishes the
lease for that consumer instead of failing.

## Admin Operation Frames

### CreateQueue (0xFD)

```
[frame header: opcode=0xFD]
[string8: name]
[optional<string32>: on_enqueue_script]
[optional<string32>: on_failure_script]
[u64: visibility_timeout_ms]         -- 0 = server default

-- ordering ([ordering.md](ordering.md))
[u8: ordering]                       -- 0 = none, 1 = the whole queue is one group,
                                     --   2 = grouped by ordering_key
[key: ordering_key]                  -- empty unless ordering = 2
[u8: when_ordering_key_missing]      -- 0 = reject, 1 = unordered; ordering = 2 only

-- deduplication ([deduplication.md](deduplication.md))
[key: dedup_key]                     -- part_count 0 = no queue-defined key
[u64: dedup_window_ms]               -- 0 = server default

-- script failures ([concepts.md](concepts.md#when-on_enqueue-fails))
[u8: script_failure]                 -- 0 = reject, 1 = park
[u32: park_dead_letter_after]        -- parked classification attempts before dead-lettering;
                                     --   0 = never

-- retries ([concepts.md](concepts.md#retry-policy))
[u32: max_attempts]                  -- 0 = server default
[u8: backoff]                        -- 0 = server default, 1 = none, 2 = fixed, 3 = exponential
[u64: backoff_initial_ms]            -- 0 = server default
[u64: backoff_max_ms]                -- 0 = server default; exponential only

-- delivery durability ([clustering.md](clustering.md#delivery-durability))
[u8: delivery_durability]            -- 0 = committed, 1 = fast
[u64: reclaim_grace_ms]              -- 0 = server default; fast queues only
```

Scripts use `string32` rather than `string8`: 255 bytes is nowhere near enough for
user-authored Lua, and the four prefix bytes cost nothing on a cold path.

Creating a queue also creates its dead-letter queue, `<name>.dlq`. A name ending in `.dlq`
is rejected with `ReservedQueueName` (`0x16`).

Zero means "use the broker's configured default" for every duration and count here, so a
client that wants defaults sends zeros rather than having to know them.

**CreateQueueResult (0xFC):**

```
[frame header: opcode=0xFC]
[u8: error_code]
[string8: queue_id]                   -- empty if error
```

### UpdateQueue (0xDF)

Changes a queue's mutable settings. Every field is optional; those left absent are
unchanged.

```
[frame header: opcode=0xDF]
[string8: queue]
[u8: on_enqueue_change]              -- 0 = unchanged, 1 = set, 2 = remove
[string32: on_enqueue_script]            -- empty unless on_enqueue_change = 1
[u8: on_failure_change]              -- 0 = unchanged, 1 = set, 2 = remove
[string32: on_failure_script]            -- empty unless on_failure_change = 1
[optional<u64>: visibility_timeout_ms]
[optional<u64>: dedup_window_ms]
[optional<u32>: max_attempts]
[optional<u8>: backoff]
[optional<u64>: backoff_initial_ms]
[optional<u64>: backoff_max_ms]
[optional<u8>: delivery_durability]
[optional<u64>: reclaim_grace_ms]
[optional<u32>: park_dead_letter_after]
```

**UpdateQueueResult (0xDE):**

```
[frame header: opcode=0xDE]
[u8: error_code]                     -- 0x01 QueueNotFound, 0x04 LuaCompilationError,
                                     --   0x1A ImmutableSetting
```

**Fixed at creation**, and therefore not in this frame: the ordering key with its
missing-key policy, the dedup key, and the script failure policy. Redefining ordering
groups or duplicate identity over a live backlog cannot preserve either guarantee. A
client that tries anyway gets `ImmutableSetting` (`0x1A`).

Changing a script affects messages enqueued afterwards; messages already queued keep the
classification they were given ([concepts.md](concepts.md#lua-hooks)). Parked messages are
reclassified with the new script ([concepts.md](concepts.md#when-on_enqueue-fails)).

### DeleteQueue (0xFB)

```
[frame header: opcode=0xFB]
[string8: queue]
```

**DeleteQueueResult (0xFA):**

```
[frame header: opcode=0xFA]
[u8: error_code]
```

### GetStats (0xF9)

```
[frame header: opcode=0xF9]
[string8: queue]
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
[u64: throttled]                     -- pending because a throttle bucket has no room
[u64: active_fairness_keys]
[u32: active_consumers]
[u32: quantum]
[u64: leader_node_id]                -- 0 if single-node
[u32: replication_count]             -- 0 if single-node
[u32: per_key_stats_count]
For each fairness key stat:
  [string8: key]
  [u64: pending_count]
  [i64: current_deficit]
  [u32: weight]
[u32: active_throttle_count]
For each throttle active on the queue:
  [throttle_summary]
```

A `throttle_summary`, shared with `GetThrottleStatsResult`:

```
[string8: name]
[u16: limit_count]
For each effective limit:
  [u64: max_deliveries]
  [u64: window_ms]
[key: key]
[u8: when_key_missing]               -- 0 = shared bucket, 1 = unthrottled
[u32: declaring_subscribers]
```

Individual buckets are never listed here; see `GetThrottleStats`.

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
  [string8: name]
  [u64: depth]
  [u64: in_flight]
  [u32: active_consumers]
  [u64: leader_node_id]              -- 0 if single-node
```

### SetConfig (0xF5)

```
[frame header: opcode=0xF5]
[string8: key]
[string8: value]
```

**SetConfigResult (0xF4):**

```
[frame header: opcode=0xF4]
[u8: error_code]
```

### GetConfig (0xF3)

```
[frame header: opcode=0xF3]
[string8: key]
```

**GetConfigResult (0xF2):**

```
[frame header: opcode=0xF2]
[u8: error_code]                     -- 0x08 InvalidConfigValue if key is unset
[string8: value]                      -- empty if error
```

### ListConfig (0xF1)

```
[frame header: opcode=0xF1]
[string8: prefix]                     -- empty = all entries
```

**ListConfigResult (0xF0):**

```
[frame header: opcode=0xF0]
[u8: error_code]
[u32: entry_count]
For each entry:
  [string8: key]
  [string8: value]
```

### Redrive (0xEF)

```
[frame header: opcode=0xEF]
[string8: dlq_queue]
[u64: count]                         -- 0 = all
[u8: reclassify]                     -- 0 = all, 1 = unclassified only, 2 = none
```

**RedriveResult (0xEE):**

```
[frame header: opcode=0xEE]
[u8: error_code]
[u64: redriven]
[u64: left_unclassified]             -- stayed in the DLQ: unclassified, reclassify = none
[u64: rejected_by_script]            -- stayed in the DLQ: script failed under Reject
```

Redriven messages keep their message ID, reset their attempt count, and keep their enqueue
not-before time. See [concepts.md](concepts.md#inspecting-and-redriving).

### CreateApiKey (0xED)

```
[frame header: opcode=0xED]
[string8: name]
[u64: expires_at_ms]                 -- Unix ms, 0 = never
[bool: is_superadmin]
```

**CreateApiKeyResult (0xEC):**

```
[frame header: opcode=0xEC]
[u8: error_code]
[string8: key_id]
[string8: key]                        -- plaintext secret, returned once
[bool: is_superadmin]
```

### RevokeApiKey (0xEB)

```
[frame header: opcode=0xEB]
[string8: key_id]
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
  [string8: key_id]
  [string8: name]
  [u64: created_at_ms]
  [u64: expires_at_ms]               -- 0 = never
  [bool: is_superadmin]
```

### SetAcl (0xE7)

```
[frame header: opcode=0xE7]
[string8: key_id]
[u32: permission_count]
For each permission:
  [string8: kind]                     -- "produce" | "consume" | "admin"
  [string8: pattern]                  -- glob over queue names
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
[string8: key_id]
```

**GetAclResult (0xE4):**

```
[frame header: opcode=0xE4]
[u8: error_code]                     -- 0x0F = ApiKeyNotFound
[string8: key_id]
[bool: is_superadmin]
[u32: permission_count]
For each permission:
  [string8: kind]
  [string8: pattern]
```

### GetThrottleStats (0xE3)

```
[frame header: opcode=0xE3]
[string8: name]
[u32: top_buckets]                   -- how many of the most-waiting buckets to include
```

**GetThrottleStatsResult (0xE2):**

```
[frame header: opcode=0xE2]
[u8: error_code]                     -- 0x18 = ThrottleNotFound
[throttle_summary]                   -- declaring_subscribers counts every queue
[u32: queue_count]
For each queue where the throttle is active:
  [string8: queue]
[u64: active_buckets]                -- buckets with deliveries inside the window
[u64: buckets_at_limit]              -- buckets with no room now
[u64: waiting]                       -- messages held by this throttle, across queues
[u32: bucket_count]
For each of the most-waiting buckets, most waiting first:
  [bucket_stats]
```

A `bucket_stats`:

```
[u8: bucket_kind]                    -- 0 = keyed or unkeyed, 1 = shared bucket for messages missing the key
[u16: value_count]                   -- one value per key part; 0 for an unkeyed or shared bucket
For each value:
  [string8: value]
[u16: limit_count]
For each effective limit, in the order of throttle_summary:
  [u64: counted]                     -- deliveries and unexpired grants inside the window
[u64: waiting]
[u64: next_room_at]                  -- Unix ms when a delivery next fits every limit; 0 = now
```

`counted` includes tokens granted but not yet spent or returned, because that is what the
limit is enforced against.

### GetThrottleBucket (0xE1)

```
[frame header: opcode=0xE1]
[string8: name]
[u8: bucket_kind]                    -- as in bucket_stats
[u16: value_count]
For each value:
  [string8: value]
```

**GetThrottleBucketResult (0xE0):**

```
[frame header: opcode=0xE0]
[u8: error_code]                     -- 0x18 = ThrottleNotFound
[bucket_stats]
```

A bucket with no deliveries inside its window is reported with every `counted` at 0 and
`next_room_at` 0, whether or not it has been evicted.

### GetRoutes (0xDD)

```
[frame header: opcode=0xDD]
[u32: queue_count]                   -- 0 = every queue
For each:
  [string8: queue]
```

**GetRoutesResult (0xDC):**

```
[frame header: opcode=0xDC]
[u8: error_code]
[u32: entry_count]
For each queue:
  [string8: queue]
  [u64: leader_node_id]              -- 0 if this node does not know
  [string8: leader_addr]              -- empty if this node does not know
  [u32: shard_count]                 -- 0 = not sharded
  For each shard:
    [u32: shard_index]
    [u64: leader_node_id]
    [string8: leader_addr]
```

Any node answers from its own copy of the meta group, so an answer may be slightly stale.
A stale answer costs a `NotLeader` redirect, never a wrong result.

### DrainNode (0xDB)

```
[frame header: opcode=0xDB]
[u64: node_id]
[bool: drain]                        -- false cancels an ongoing drain
```

**DrainNodeResult (0xDA):**

```
[frame header: opcode=0xDA]
[u8: error_code]                     -- 0x1B = NodeNotFound
```

Draining moves every leadership off the node through controlled handovers, so that the
node can be stopped without a queue losing in-flight leases.

### MoveQueueLeader (0xD9)

```
[frame header: opcode=0xD9]
[string8: queue]
[u64: target_node_id]                -- must be a member of the queue's group
```

**MoveQueueLeaderResult (0xD8):**

```
[frame header: opcode=0xD8]
[u8: error_code]                     -- 0x01 QueueNotFound,
                                     --   0x1B NodeNotFound: unknown, or not a member
```

A move pins nothing. Automatic rebalancing, when enabled, may move the queue again later.

### SetRebalancing (0xD7)

```
[frame header: opcode=0xD7]
[u8: mode]                           -- 0 = enabled, 1 = paused, 2 = disabled
[u64: pause_ms]                      -- mode 1 only; how long the pause lasts
```

**SetRebalancingResult (0xD6):**

```
[frame header: opcode=0xD6]
[u8: error_code]
```

A pause always has a duration and resumes on its own; disabling lasts until rebalancing is
enabled again. Both are cluster-wide state in the meta group. See
[clustering.md](clustering.md#operator-control).

### GetClusterStatus (0xD5)

```
[frame header: opcode=0xD5]
```

**GetClusterStatusResult (0xD4):**

```
[frame header: opcode=0xD4]
[u8: error_code]
[u64: meta_leader_node_id]
[u8: rebalancing]                    -- 0 = enabled, 1 = paused, 2 = disabled
[u64: rebalancing_pause_ends_at]     -- Unix ms; 0 unless paused
[u32: node_count]
For each node:
  [u64: node_id]
  [string8: addr]
  [u8: state]                        -- 0 = up, 1 = unreachable, 2 = draining
  [u32: queue_leaderships]
```

## Error Frame (0xFE)

```
[frame header: opcode=0xFE, request_id=<failed request's ID>]
[u8: error_code]
[string8: message]                    -- human-readable
[map<string8,string8>: metadata]       -- machine-readable context
```

Standard metadata keys:

| Error Code | Key | Value | Description |
|------------|-----|-------|-------------|
| `0x0C` NotLeader | `leader_addr` | `"host:port"` | Current leader |
| `0x09` ChannelFull | `retry_after_ms` | `"100"` | Suggested backoff |
| `0x0D` UnsupportedVersion | `min_version` | `"1"` | Lowest version still accepted |
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
- Fragmenting to a peer needs its agreement, so continuation is a capability as well as
  a flag. The flag marks each fragment; the capability is how a peer that has not
  implemented reassembly declines to receive any. A flag cannot carry that, being set
  on a frame already in flight. See [Flags and capabilities](#flags-and-capabilities).

## Overhead Analysis

### Single Enqueue (batch of 1)

1 KB payload to queue `orders`, no headers, no scheduling metadata:

| Component | Bytes |
|-----------|-------|
| Frame length prefix | 4 |
| Opcode + Flags + Request ID | 6 |
| Message count (u32) | 4 |
| Queue string8 (1 + 6) | 7 |
| Headers map (count=0) | 2 |
| Payload (4 + 1024) | 1028 |
| fairness_key (absent) | 1 |
| weight (absent) | 1 |
| delay_ms (absent) | 1 |
| idempotency_key (absent) | 1 |
| **Total** | **1055** |
| **Overhead beyond payload** | **31 bytes** |

The four optional fields cost 4 bytes when unused — the price of making
fairness settable without Lua.

### Batch Enqueue (100 messages, same queue)

| Component | Bytes |
|-----------|-------|
| Frame length + header + count | 14 |
| 100x queue string8 | 700 |
| 100x headers map (empty) | 200 |
| 100x payload (4 + 1024) | 102,800 |
| 100x optional fields (unused) | 400 |
| **Total** | **104,114** |
| **Per-message overhead** | **17.14 bytes** |

### Batch Ack (1,000 messages)

An ack item is a queue name and an ID — there is no payload to amortize the encoding
against, which is why the ID is 16 raw bytes.

| Component | Bytes |
|-----------|------:|
| Queue string8 (1 + 6) | 7 |
| Message ID (`uuid`) | 16 |
| **Per item** | **23** |
| **1,000 items** | **23,000** |

Encoding the same ID as a 36-character string would cost 44 bytes per item, and
44,000 for the batch.

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

### What needs a new version

Two rules, by scope:

| Scope | Allowed |
|-------|---------|
| Within a version | Append fields at the end. Never remove, retype or reorder. |
| Across a version bump | Any layout change, including removal and retyping. |

Appending is safe because a reader tolerates trailing bytes it does not understand: an
older peer skips the new field, while a writer must not omit a field present in the
negotiated version. Removing or retyping changes how **existing** bytes are read, so an
older peer would misparse them.

That is the whole test — not whether a feature is optional, but whether **an older
reader parsing older bytes still gets the right answer**. Appending always passes it.
Nor does a sender that cannot proceed without a feature need a version: it advertises a
capability, finds the bit absent in `HandshakeOk`, and closes the connection itself.

Deprecated fields are never removed within a version; they carry zero or empty values.

### Prefer a capability

Versions are for layout changes; capabilities are for behaviour either side may not
implement. Versions are **ordered** — version 3 implies 2 implies 1 — so an optional
feature placed in a version forces every implementation to build it just to claim the
number. Capabilities are **orthogonal**: 31 independent switches, any combination
legal, and client and server upgrade in either order with no coordination.

| Change | Path | New version? |
|--------|------|-------------|
| Add optional behaviour | a capability bit | no |
| Add a field | a capability bit announcing it, and the field appended | no |
| Redefine an existing field | a capability bit that changes its reading | no |
| Remove, retype or reorder a field | the new version's layout | yes |

### Protocol Versioning

A client sends the highest version it speaks. The server picks the highest version it
supports that is both **≤ the client's** and **≥ its own minimum**. `HandshakeOk`
carries that `negotiated_version`, and it governs every later frame on the connection.

If no such version exists the server replies `UnsupportedVersion` carrying **both**
`min_version` and `max_version`, then closes. Both are needed. A server that has
dropped version 1 and speaks 2–3 answers a version-1 client with `max_version=3`, which
on its own does not say that 1 is no longer accepted — the client would retry at 1
indefinitely rather than report that it is too old to connect.

A client checks `negotiated_version` against **its own** minimum and closes if it is
lower. A client may drop old versions too, and the server has no way to know its floor.

A client told to use a lower version retries on a new connection, and should remember
the answer per endpoint, so skew costs one reconnect per client process rather than one
per connection.

Version 1 is the initial version.

### Dropping a version

1. The server adds the new version beside the old and advertises the higher maximum.
   Existing clients keep negotiating the old one and notice nothing.
2. Updated clients offer the new version. A server that lacks it answers with its range
   and those clients fall back.
3. When nothing needs the old version, the server raises its minimum. A client too old
   to connect then receives `UnsupportedVersion` naming the range, and that refusal is
   the deprecation notice.

### What no version can change

Three things are read before any version is known, and are therefore fixed for the life
of the protocol:

- the **`u32` length prefix**, read on every frame including the first
- the **6-byte frame header of both handshake frames**
- the **offset of `protocol_version`** within the handshake body

A version may redefine the frame header for frames sent *after* negotiation, provided
the handshake exchange itself keeps version 1's header — the client reads
`negotiated_version` out of `HandshakeOk`, so that frame cannot already depend on it.
New opcodes need none of this: an unknown opcode is answered `InvalidFrame` with the
connection intact.

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
SDKs should default to one connection per node, with multiplexed requests and multiple
subscriptions on it.

### Clusters

Which node handles each request is described in
[clustering.md](clustering.md#routing). Clients reach any node and are steered from there:
`NotLeader` (`0x0C`) redirects a consumer to a queue's leader, `EnqueueResult` carries
route hints, and `GetRoutes` (`0xDD`) answers where queues live.

A node that cannot confirm its API keys and ACLs are current refuses authenticated requests
with `AuthStateStale` (`0x19`) rather than answering from state that may have missed a
revocation. Clients should retry against another node.

### Consume and Ack on the Same Connection

Subscribing and acking on one connection is the expected pattern. Ack, Nack and
ExtendLease request IDs are independent of the Consume request ID.

### Maximum Frame Size

Default 16 MiB, configurable, advertised in `HandshakeOk`. The length field is `u32`
(~4 GiB), but a lower enforced limit bounds per-frame allocation. This does not limit
payload size — larger bodies use continuation frames. A server receiving an oversized
frame responds `InvalidFrame` (`0x0E`).

**Before a connection has handshaked the limit is 1 KiB**, not the configured maximum.
Everything about the handshake is processed before the peer is authenticated, so what
an unauthenticated peer can make a server buffer is bounded by this specification
rather than by configuration. Version 1's longest legal `Handshake` is 273 bytes; the
rest is headroom for later layouts.

A first frame whose length prefix exceeds that limit is still **answered, not dropped**.
The server reads the eight bytes it needs — length prefix, frame header,
`protocol_version` — replies `UnsupportedVersion` with the range it supports, and
closes, without ever buffering the frame. A client newer than the server it reached
then learns what to retry with, instead of seeing a socket close for no stated reason.

### Byte Order

All multi-byte integers are big-endian (network byte order) — consistent, and it
matches Wireshark's default display.
