# Story 36.4: Rust SDK FIBP Transport

**Epic:** 35-36 — gRPC Streaming Tuning + Custom Binary Protocol (FIBP)
**Status:** Done

## Goal

Add FIBP as an opt-in transport in the Rust SDK alongside gRPC, so SDK users can connect via the high-throughput binary protocol without changing their application code.

## Acceptance Criteria

- [x] `Transport` enum (`Grpc`, `Fibp`) added to SDK public API
- [x] `ConnectOptions` gains `transport` field (default: `Grpc`) and `with_fibp()` builder
- [x] `FilaClient` routes all operations through FIBP when configured
- [x] FIBP wire format codec reimplemented in SDK (no dependency on fila-core)
- [x] Background I/O loop with correlation-ID-based request/response multiplexing
- [x] Consume stream over FIBP with credit-based flow control
- [x] Admin operations (create queue, delete queue, list queues, queue stats) over FIBP
- [x] TLS support for FIBP client connections
- [x] API key authentication over FIBP (OP_AUTH frame)
- [x] 6 integration tests covering enqueue, consume, ack, nack, batch, admin ops
- [x] Existing gRPC tests unaffected (15/15 pass)
- [x] SDK examples and configuration docs updated

## Implementation

### New files
- `crates/fila-sdk/src/fibp_codec.rs` — FIBP wire format codec (frame encoding/decoding, wire payload encoding/decoding)
- `crates/fila-sdk/src/fibp_transport.rs` — FIBP client transport (TCP connection, handshake, multiplexed I/O, TLS support)
- `crates/fila-sdk/tests/fibp_integration.rs` — 6 integration tests

### Modified files
- `crates/fila-sdk/src/lib.rs` — export `Transport`, `FibpTransport`
- `crates/fila-sdk/src/client.rs` — `Transport` enum, FIBP routing in `FilaClient`
- `crates/fila-sdk/Cargo.toml` — added tokio-util, tokio-rustls, rustls-pemfile, rustls-native-certs, prost
- `docs/sdk-examples.md` — FIBP connection examples
- `docs/configuration.md` — SDK transport option documentation

## Design Decisions

1. **SDK does not depend on fila-core** — Wire format reimplemented in `fibp_codec.rs` to keep the SDK lightweight. Only `fila-proto` is shared.

2. **Single I/O task with select** — Background task uses `tokio::select!` to interleave reading server frames and writing client frames. No stream splitting needed.

3. **Correlation ID multiplexing** — Each request gets a unique `u32` ID via `AtomicU32`. Responses are dispatched to pending oneshot channels by matching correlation IDs.

4. **Consume via push frames** — Server sends `FLAG_STREAM` frames for consume delivery. The I/O loop routes these to a dedicated `mpsc::Sender` channel, which the consumer reads from.

5. **No accumulator for FIBP** — The FIBP wire format already supports multi-message enqueue in a single frame. The gRPC accumulator is skipped when FIBP is active.

6. **Admin operations use protobuf** — Same as server-side: admin payloads are protobuf-encoded for schema evolution, while data operations use the compact binary wire format.
