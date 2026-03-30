# Fila Binary Protocol Specification

Version: 1 (draft)

## Overview

Fila uses a custom binary protocol over TCP for all client-server communication. The protocol is designed for:

- **Minimal overhead**: < 16 bytes amortized per message beyond payload in batch operations
- **Zero-copy parsing**: Length-prefixed frames — no delimiter scanning
- **Batch-native**: Every operation accepts multiple items; single message = batch of 1
- **Multiplexed**: Multiple concurrent requests on a single connection via request IDs
- **Streaming**: Server-push delivery for consume operations

The protocol replaces gRPC/HTTP2/protobuf with a purpose-built binary format optimized for message broker workloads.

## Transport Layer

### TCP Connection

Clients connect to the Fila server on a configurable TCP port (default: 5555).

### TLS

TLS is optional and wraps the TCP connection using standard TLS 1.2+. When enabled:

- Server presents its certificate during TLS handshake
- Client optionally presents a certificate for mTLS mutual authentication
- All subsequent protocol bytes flow over the encrypted TLS channel
- Same certificate/key configuration as existing Fila TLS config (`tls_cert_file`, `tls_key_file`, `tls_ca_cert_file`)

TLS negotiation happens at the transport layer before any protocol bytes are exchanged. The protocol itself is TLS-agnostic.

## Frame Format

All communication uses length-prefixed frames. Every frame has the same outer structure:

```
+----------------+-------------------+
| Frame Length   | Frame Body        |
| (4 bytes, BE)  | (variable)        |
+----------------+-------------------+
```

- **Frame Length**: Big-endian `u32`. The byte count of the Frame Body (not including the 4-byte length prefix itself). Maximum frame size: 16 MiB (16,777,216 bytes).

### Frame Header

Every Frame Body starts with a fixed 6-byte header:

```
+----------+----------+------------------+
| Opcode   | Flags    | Request ID       |
| (1 byte) | (1 byte) | (4 bytes, BE)    |
+----------+----------+------------------+
```

- **Opcode**: Identifies the operation (see Opcode Table below).
- **Flags**: Bitfield for frame-level options.
  - Bit 0: **CONTINUATION** — When set, this frame is a continuation of the previous frame with the same request ID and opcode. The receiver concatenates frame bodies (excluding headers) until a frame with CONTINUATION=0 arrives. See [Continuation Frames](#continuation-frames) below.
  - Bits 1-7: Reserved (must be 0)
- **Request ID**: Big-endian `u32`. Client-assigned identifier for correlating responses. Responses echo the request ID from the corresponding request. Server-initiated frames (ConsumeDelivery) use the request ID from the Consume subscribe request.

### Total Frame Overhead

Per frame: 4 (length) + 1 (opcode) + 1 (flags) + 4 (request ID) = **10 bytes fixed overhead**.

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
| `string` | `[u16 length][UTF-8 bytes]` | 2 + length |
| `bytes` | `[u32 length][raw bytes]` | 4 + length |
| `map<string,string>` | `[u16 count][repeated: string key, string value]` | 2 + sum of entries |
| `string[]` | `[u16 count][repeated: string]` | 2 + sum of strings |
| `optional<T>` | `[u8 present (0 or 1)][T if present]` | 1 or 1 + sizeof(T) |

Strings are limited to 65,535 bytes. Byte arrays use a `u32` length prefix (up to ~4 GiB). When a single value exceeds the maximum frame size, the sender uses [continuation frames](#continuation-frames) to split the data across multiple frames.

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
| `0x10` | Enqueue | Client → Server | Enqueue batch of messages |
| `0x11` | EnqueueResult | Server → Client | Per-message enqueue results |
| `0x12` | Consume | Client → Server | Subscribe to queue delivery |
| `0x13` | Delivery | Server → Client | Batch of messages pushed to consumer |
| `0x14` | CancelConsume | Client → Server | Unsubscribe from delivery |
| `0x15` | Ack | Client → Server | Acknowledge batch of messages |
| `0x16` | AckResult | Server → Client | Per-message ack results |
| `0x17` | Nack | Client → Server | Negative-acknowledge batch of messages |
| `0x18` | NackResult | Server → Client | Per-message nack results |

### Admin Opcodes (0x20-0x3F)

| Opcode | Name | Direction | Description |
|--------|------|-----------|-------------|
| `0x20` | CreateQueue | Client → Server | Create a queue |
| `0x21` | CreateQueueResult | Server → Client | Queue creation result |
| `0x22` | DeleteQueue | Client → Server | Delete a queue |
| `0x23` | DeleteQueueResult | Server → Client | Deletion result |
| `0x24` | GetStats | Client → Server | Get queue statistics |
| `0x25` | GetStatsResult | Server → Client | Queue statistics |
| `0x26` | ListQueues | Client → Server | List all queues |
| `0x27` | ListQueuesResult | Server → Client | Queue list |
| `0x28` | SetConfig | Client → Server | Set runtime config key |
| `0x29` | SetConfigResult | Server → Client | Config set result |
| `0x2A` | GetConfig | Client → Server | Get runtime config key |
| `0x2B` | GetConfigResult | Server → Client | Config value |
| `0x2C` | ListConfig | Client → Server | List config keys by prefix |
| `0x2D` | ListConfigResult | Server → Client | Config entries |
| `0x2E` | Redrive | Client → Server | Redrive DLQ messages |
| `0x2F` | RedriveResult | Server → Client | Redrive count |
| `0x30` | CreateApiKey | Client → Server | Create an API key |
| `0x31` | CreateApiKeyResult | Server → Client | API key creation result |
| `0x32` | RevokeApiKey | Client → Server | Revoke an API key |
| `0x33` | RevokeApiKeyResult | Server → Client | Revocation result |
| `0x34` | ListApiKeys | Client → Server | List all API keys |
| `0x35` | ListApiKeysResult | Server → Client | API key list |
| `0x36` | SetAcl | Client → Server | Set ACL permissions for a key |
| `0x37` | SetAclResult | Server → Client | ACL set result |
| `0x38` | GetAcl | Client → Server | Get ACL permissions for a key |
| `0x39` | GetAclResult | Server → Client | ACL permissions |

### Error Opcode (0xFE)

| Opcode | Name | Direction | Description |
|--------|------|-----------|-------------|
| `0xFE` | Error | Server → Client | Operation error with error code and message |

Opcodes `0x3A-0x3F` and `0x40-0xFD` are reserved for future use. The `0x40-0x5F` range is reserved for cluster-specific opcodes. Clients must ignore frames with unknown opcodes. Servers must respond with Error (0xFE) for unknown request opcodes.

## Error Codes

Errors are returned either via the Error frame (for request-level failures) or inline in per-item result arrays (for batch item failures).

| Code | Name | Description |
|------|------|-------------|
| `0x00` | Ok | Success (used in per-item results) |
| `0x01` | QueueNotFound | Queue does not exist |
| `0x02` | MessageNotFound | Message ID not found or not leased |
| `0x03` | QueueAlreadyExists | Queue with this name already exists |
| `0x04` | LuaCompilationError | Lua script failed to compile |
| `0x05` | StorageError | Internal storage engine failure |
| `0x06` | NotADLQ | Queue is not a dead-letter queue |
| `0x07` | ParentQueueNotFound | DLQ's parent queue not found |
| `0x08` | InvalidConfigValue | Config value is invalid |
| `0x09` | ChannelFull | Server overloaded (backpressure) |
| `0x0A` | Unauthorized | Missing or invalid API key |
| `0x0B` | Forbidden | Insufficient permissions (ACL) |
| `0x0C` | NotLeader | This node is not the leader for the queue (includes leader hint) |
| `0x0D` | UnsupportedVersion | Protocol version not supported |
| `0x0E` | InvalidFrame | Malformed or unparseable frame |
| `0x0F` | ApiKeyNotFound | API key ID does not exist |
| `0x10` | NodeNotReady | Cluster node is not ready (no leader elected yet) |
| `0xFF` | InternalError | Unexpected server error |

## Connection Lifecycle

### 1. TCP Connect (+ Optional TLS)

Client opens a TCP connection (optionally wrapped in TLS).

### 2. Handshake

The client must send a Handshake frame as the first frame after connecting (or after TLS negotiation). No other frames may be sent before the handshake completes.

**Handshake (0x01)** — Client → Server:

```
[frame header: opcode=0x01, flags=0, request_id=0]
[u16: protocol_version]         -- currently 1
[optional<string>: api_key]     -- API key for authentication (absent if auth disabled)
```

**HandshakeOk (0x02)** — Server → Client:

```
[frame header: opcode=0x02, flags=0, request_id=0]
[u16: negotiated_version]       -- version the server will use
[u64: node_id]                  -- server's cluster node ID (0 if single-node)
```

If the server rejects the handshake (unsupported version, invalid API key), it sends an Error frame and closes the connection.

### 3. Request/Response

After handshake, the client sends request frames and the server responds with the corresponding result frame (matched by request ID). Multiple requests can be in-flight concurrently.

### 4. Consume Streaming

After a Consume subscribe request, the server pushes Delivery frames whenever messages are ready. The client acks/nacks messages using normal Ack/Nack frames on the same connection. The client sends CancelConsume to stop delivery, or disconnects.

### 5. Keepalive

Either side can send Ping at any time. The receiver must respond with Pong using the same request ID. If no Pong is received within 30 seconds, the sender should close the connection.

### 6. Disconnect

Either side sends Disconnect for graceful close, then closes the TCP connection. The other side should finish processing any in-flight responses and close.

## Hot-Path Operation Frames

### Enqueue (0x10)

Enqueue one or more messages. Each message specifies its target queue independently, allowing cross-queue batching in a single frame. The server applies per-queue ACL checks and routes each message to the appropriate queue (including Raft group in cluster mode). Partial success is possible — some messages may succeed while others fail.

**Request:**

```
[frame header: opcode=0x10]
[u32: message_count]
For each message:
  [string: queue]
  [map<string,string>: headers]
  [bytes: payload]
```

**EnqueueResult (0x11):**

```
[frame header: opcode=0x11]
[u32: result_count]
For each result:
  [u8: error_code]              -- 0x00 = success
  [string: message_id]          -- UUID string, empty if error
```

The order of results matches the order of messages in the request.

### Consume (0x12)

Subscribe to message delivery from a queue.

**Request:**

```
[frame header: opcode=0x12]
[string: queue]
```

If successful, the server begins pushing Delivery frames. If the server is not the leader for this queue (cluster mode), it responds with an Error frame containing error code `0x0C` (NotLeader) with the leader address in the error message.

### Delivery (0x13)

Server pushes a batch of ready messages to a consuming client. Uses the request ID from the original Consume subscribe request.

**Server → Client:**

```
[frame header: opcode=0x13, request_id=<consume_request_id>]
[u32: message_count]
For each message:
  [string: message_id]          -- UUID string
  [string: queue]
  [map<string,string>: headers]
  [bytes: payload]
  [string: fairness_key]
  [u32: weight]
  [string[]: throttle_keys]
  [u32: attempt_count]
  [u64: enqueued_at]            -- Unix timestamp milliseconds
  [u64: leased_at]              -- Unix timestamp milliseconds (0 if unavailable)
```

### CancelConsume (0x14)

Unsubscribe from delivery. Uses the same request ID as the original Consume request.

**Request:**

```
[frame header: opcode=0x14, request_id=<consume_request_id>]
```

The server stops pushing Delivery frames for this subscription. No response frame is sent.

### Ack (0x15)

Acknowledge one or more messages.

**Request:**

```
[frame header: opcode=0x15]
[u32: item_count]
For each item:
  [string: queue]
  [string: message_id]
```

**AckResult (0x16):**

```
[frame header: opcode=0x16]
[u32: result_count]
For each result:
  [u8: error_code]              -- 0x00 = success, 0x02 = MessageNotFound
```

### Nack (0x17)

Negative-acknowledge one or more messages.

**Request:**

```
[frame header: opcode=0x17]
[u32: item_count]
For each item:
  [string: queue]
  [string: message_id]
  [string: error]               -- error description
```

**NackResult (0x18):**

```
[frame header: opcode=0x18]
[u32: result_count]
For each result:
  [u8: error_code]              -- 0x00 = success, 0x02 = MessageNotFound
```

## Admin Operation Frames

### CreateQueue (0x20)

**Request:**

```
[frame header: opcode=0x20]
[string: name]
[optional<string>: on_enqueue_script]
[optional<string>: on_failure_script]
[u64: visibility_timeout_ms]    -- 0 = server default
```

**CreateQueueResult (0x21):**

```
[frame header: opcode=0x21]
[u8: error_code]
[string: queue_id]              -- empty if error
```

### DeleteQueue (0x22)

**Request:**

```
[frame header: opcode=0x22]
[string: queue]
```

**DeleteQueueResult (0x23):**

```
[frame header: opcode=0x23]
[u8: error_code]
```

### GetStats (0x24)

**Request:**

```
[frame header: opcode=0x24]
[string: queue]
```

**GetStatsResult (0x25):**

```
[frame header: opcode=0x25]
[u8: error_code]
[u64: depth]
[u64: in_flight]
[u64: active_fairness_keys]
[u32: active_consumers]
[u32: quantum]
[u64: leader_node_id]           -- 0 if single-node
[u32: replication_count]        -- 0 if single-node
[u16: per_key_stats_count]
For each fairness key stat:
  [string: key]
  [u64: pending_count]
  [i64: current_deficit]
  [u32: weight]
[u16: per_throttle_stats_count]
For each throttle key stat:
  [string: key]
  [f64: tokens]
  [f64: rate_per_second]
  [f64: burst]
```

### ListQueues (0x26)

**Request:**

```
[frame header: opcode=0x26]
```

**ListQueuesResult (0x27):**

```
[frame header: opcode=0x27]
[u8: error_code]
[u32: cluster_node_count]       -- 0 if single-node
[u16: queue_count]
For each queue:
  [string: name]
  [u64: depth]
  [u64: in_flight]
  [u32: active_consumers]
  [u64: leader_node_id]         -- 0 if single-node
```

### SetConfig (0x28)

**Request:**

```
[frame header: opcode=0x28]
[string: key]
[string: value]
```

**SetConfigResult (0x29):**

```
[frame header: opcode=0x29]
[u8: error_code]
```

### GetConfig (0x2A)

**Request:**

```
[frame header: opcode=0x2A]
[string: key]
```

**GetConfigResult (0x2B):**

```
[frame header: opcode=0x2B]
[u8: error_code]
[string: value]                 -- empty if error or key not found
```

### ListConfig (0x2C)

**Request:**

```
[frame header: opcode=0x2C]
[string: prefix]
```

**ListConfigResult (0x2D):**

```
[frame header: opcode=0x2D]
[u8: error_code]
[u16: entry_count]
For each entry:
  [string: key]
  [string: value]
```

### Redrive (0x2E)

**Request:**

```
[frame header: opcode=0x2E]
[string: dlq_queue]
[u64: count]
```

**RedriveResult (0x2F):**

```
[frame header: opcode=0x2F]
[u8: error_code]
[u64: redriven]
```

## Auth & ACL Operation Frames

### CreateApiKey (0x30)

**Request:**

```
[frame header: opcode=0x30]
[string: name]                  -- human-readable label
[u64: expires_at_ms]            -- Unix timestamp ms, 0 = no expiration
[bool: is_superadmin]
```

**CreateApiKeyResult (0x31):**

```
[frame header: opcode=0x31]
[u8: error_code]
[string: key_id]                -- opaque ID for management
[string: key]                   -- plaintext API key (returned once)
[bool: is_superadmin]
```

### RevokeApiKey (0x32)

**Request:**

```
[frame header: opcode=0x32]
[string: key_id]
```

**RevokeApiKeyResult (0x33):**

```
[frame header: opcode=0x33]
[u8: error_code]                -- 0x0F = ApiKeyNotFound
```

### ListApiKeys (0x34)

**Request:**

```
[frame header: opcode=0x34]
```

**ListApiKeysResult (0x35):**

```
[frame header: opcode=0x35]
[u8: error_code]
[u16: key_count]
For each key:
  [string: key_id]
  [string: name]
  [u64: created_at_ms]
  [u64: expires_at_ms]          -- 0 = no expiration
  [bool: is_superadmin]
```

### SetAcl (0x36)

**Request:**

```
[frame header: opcode=0x36]
[string: key_id]
[u16: permission_count]
For each permission:
  [string: kind]                -- "produce", "consume", or "admin"
  [string: pattern]             -- queue name or wildcard ("*", "orders.*")
```

**SetAclResult (0x37):**

```
[frame header: opcode=0x37]
[u8: error_code]                -- 0x0F = ApiKeyNotFound
```

### GetAcl (0x38)

**Request:**

```
[frame header: opcode=0x38]
[string: key_id]
```

**GetAclResult (0x39):**

```
[frame header: opcode=0x39]
[u8: error_code]                -- 0x0F = ApiKeyNotFound
[string: key_id]
[bool: is_superadmin]
[u16: permission_count]
For each permission:
  [string: kind]
  [string: pattern]
```

## Error Frame (0xFE)

Used for request-level errors (as opposed to per-item errors in batch results). The request ID matches the request that caused the error.

```
[frame header: opcode=0xFE]
[u8: error_code]
[string: message]               -- human-readable error description
[map<string,string>: metadata]  -- structured key-value pairs for programmatic error handling
```

The metadata map provides machine-readable context beyond the human-readable message. Standard metadata keys by error code:

| Error Code | Metadata Key | Value | Description |
|------------|-------------|-------|-------------|
| `0x0C` NotLeader | `leader_addr` | `"host:port"` | Address of the current leader node |
| `0x09` ChannelFull | `retry_after_ms` | `"100"` | Suggested backoff in milliseconds |
| `0x0D` UnsupportedVersion | `max_version` | `"1"` | Highest version the server supports |

SDKs should expose the metadata map to callers. Unknown metadata keys must be preserved (not discarded) for forward compatibility. The metadata map may be empty.

## Continuation Frames

The protocol supports arbitrarily large payloads and headers through frame continuation. When a message's encoded body exceeds the maximum frame size, the sender splits it across multiple frames using the CONTINUATION flag (Flags bit 0).

### How It Works

1. The sender serializes the full operation body (e.g., an Enqueue with all its messages) into a byte buffer
2. If the buffer fits in a single frame: send it normally with CONTINUATION=0
3. If the buffer exceeds the max frame size:
   - Send the first chunk as a normal frame with CONTINUATION=1 (same opcode, same request ID)
   - Send subsequent chunks as continuation frames: same opcode, same request ID, CONTINUATION=1
   - Send the final chunk with CONTINUATION=0 to signal completion

### Receiver Behavior

When a receiver gets a frame with CONTINUATION=1, it buffers the frame body (excluding the 6-byte header). It continues buffering until a frame arrives with the same request ID and opcode with CONTINUATION=0, then concatenates all buffered bodies (in order) with the final frame's body and parses the result as a single operation body.

### Rules

- All continuation frames for a request must use the same opcode and request ID
- The receiver may enforce a maximum total reassembled size (configurable, no protocol-level cap)
- Interleaving: frames from different request IDs may be interleaved on the wire. The receiver tracks continuation state per request ID.
- If a connection closes mid-continuation, the partial data is discarded

### Example: Large Payload Enqueue

Enqueuing a single 50 MiB message with a 16 MiB max frame size:

| Frame | Flags | Size | Contents |
|-------|-------|------|----------|
| 1 | CONTINUATION=1 | 16 MiB | First 16 MiB of serialized Enqueue body |
| 2 | CONTINUATION=1 | 16 MiB | Next 16 MiB |
| 3 | CONTINUATION=1 | 16 MiB | Next 16 MiB |
| 4 | CONTINUATION=0 | ~2 MiB | Final chunk (remainder) |

The receiver reassembles frames 1-4 into the complete Enqueue body, then parses it normally.

## Overhead Analysis

### Single Enqueue (Batch of 1)

For a 1KB message to queue "orders" with no headers:

| Component | Bytes |
|-----------|-------|
| Frame length prefix | 4 |
| Opcode + Flags + Request ID | 6 |
| Message count (u32) | 4 |
| Queue string (2 + 6) | 8 |
| Headers map (count=0) | 2 |
| Payload (4 + 1024) | 1028 |
| **Total** | **1052** |
| **Overhead beyond payload** | **28 bytes** |

### Batch Enqueue (100 Messages, Same Queue)

| Component | Bytes |
|-----------|-------|
| Frame length prefix | 4 |
| Opcode + Flags + Request ID | 6 |
| Message count (u32) | 4 |
| 100x Queue string (2 + 6) | 800 |
| 100x Headers map (count=0) | 200 |
| 100x Payload (4 + 1024) | 102,800 |
| **Total** | **103,814** |
| **Per-message overhead** | **(103,814 - 102,400) / 100 = 14.14 bytes** |

Per-message overhead in batch operations: **~14 bytes** (< 16 byte NFR-P3 target).

### Comparison with gRPC/Protobuf

| Protocol | Single 1KB Enqueue | Notes |
|----------|--------------------|-------|
| Fila binary | ~28 bytes overhead | Length-prefixed, no HTTP/2 |
| gRPC/HTTP2/protobuf | ~100-200 bytes overhead | HTTP/2 HEADERS + DATA frames, protobuf field tags, HPACK |

The binary protocol eliminates HTTP/2 framing (~13% of CPU per flamegraph) and protobuf encoding overhead.

## Serialization Format Decision

### Options Evaluated

| Format | Per-field overhead | Zero-copy | Cross-language | Schema evolution |
|--------|--------------------|-----------|----------------|-----------------|
| **Hand-rolled binary** | 0 (fixed layout) | Yes | Manual per SDK | Add fields at end |
| msgpack | 1-5 bytes/field | No (decode required) | Excellent (all 6 languages) | Via key-value maps |
| bincode | 0 (fixed layout) | Limited | Rust only | Poor |
| postcard | 0-2 bytes (varint) | Limited | Rust only | Poor |
| FlatBuffers | vtable overhead | Yes | Good (not all languages) | Via vtable |
| Cap'n Proto | pointer overhead | Yes | Limited (no Ruby) | Via pointer evolution |

### Decision: Hand-Rolled Binary with Fixed Layouts

**Rationale:**

1. **Minimal overhead**: Fixed field layouts mean zero per-field encoding overhead. The Kafka protocol, Redis RESP3, and NATS client protocol all use this approach for the same reason.

2. **Zero-copy payload**: Message payloads are `[u32 length][raw bytes]` — the reader knows exactly where the payload starts and can reference it without copying.

3. **Cross-language implementability**: The encoding primitives (big-endian integers, length-prefixed strings/bytes) are trivial to implement in all 6 SDK languages. No external serialization library dependency.

4. **Simplicity**: Each opcode has a fixed field order. No field tags, no schema files, no code generation. A developer can implement a client from this document alone.

5. **Schema evolution**: New fields are appended to the end of opcode bodies. Older clients ignore trailing bytes they don't understand. The protocol version in the handshake gates which fields are present.

**Trade-off**: Changes to field order or types within an opcode require a protocol version bump. This is acceptable because operations are well-defined and stable — the Fila API surface has been stable since Epic 5.

## Schema Evolution

### Adding Fields

New fields are appended to the end of an opcode's body. Readers must accept frames with trailing bytes beyond what they expect (ignore unknown trailing data). Writers must not omit fields that were present in the version they negotiated.

### Removing Fields

Fields are never removed. Deprecated fields are sent with zero/empty values.

### Protocol Versioning

The handshake negotiates a protocol version. The server accepts the highest version it supports that is <= the client's requested version. If no compatible version exists, the server rejects with UnsupportedVersion.

Version 1 is the initial version defined in this document.

## Cluster Communication

Cluster inter-node communication on port 5556 uses the same frame format and encoding primitives. Cluster-specific opcodes are reserved in the `0x40-0x5F` range (defined in a future cluster protocol extension). For the initial implementation, cluster nodes use the standard client opcodes for leader forwarding.

## Implementation Notes

### Backpressure

If the server's internal command channel is full, it returns error code `0x09` (ChannelFull). Clients should implement exponential backoff on this error.

### Connection Pooling

Clients may open multiple connections to the same server. Request IDs are scoped per connection (not globally unique). SDKs should default to a single connection with multiplexed requests.

### Consume and Ack on Same Connection

A client can subscribe to a queue (Consume) and send Ack/Nack frames on the same connection. This is the expected pattern — the consumer receives Delivery frames and acknowledges them inline. Request IDs for Ack/Nack are independent of the Consume request ID.

### Maximum Frame Size

Individual frames are limited to a configurable maximum size (default: 16 MiB). The frame length field is u32, supporting up to ~4 GiB per frame, but the server enforces a lower limit to bound memory allocation per frame. This does **not** limit payload or header sizes — messages larger than the max frame size are transparently split using [continuation frames](#continuation-frames). Clients must use continuation frames when the serialized operation body exceeds the negotiated max frame size.

### Byte Order

All multi-byte integers throughout the protocol are big-endian (network byte order). This is a deliberate choice for consistency and debuggability (matches Wireshark's default display).
