# Story 36.1: FIBP Protocol Foundation

Status: done

## Story

As a developer,
I want a custom binary TCP protocol (FIBP) running alongside gRPC,
so that Fila can serve high-throughput workloads with lower per-message overhead than HTTP/2.

## Acceptance Criteria

1. **Given** a `[fibp]` section in `fila.toml` **Then** `FibpConfig` is parsed with fields: `listen_addr` (default `"0.0.0.0:5557"`), `max_frame_size` (default 16MB), `keepalive_interval_secs` (default 15), `keepalive_timeout_secs` (default 10).
2. **Given** no `[fibp]` section **Then** `BrokerConfig.fibp` is `None` and FIBP is completely disabled (zero overhead).
3. **Given** a FIBP-enabled server **Then** a TCP listener binds on the configured address and accepts connections.
4. **Given** a client connects via TCP **Then** handshake completes: client sends `b"FIBP\x01\x00"` magic, server validates and echoes back.
5. **Given** a client sends magic with wrong major version **Then** server sends GoAway frame and closes the connection.
6. **Given** a connected client sends a heartbeat frame **Then** server echoes back a heartbeat with the same correlation ID.
7. **Given** a connected client sends a data operation frame (enqueue, consume, etc.) **Then** server responds with an error frame indicating "not implemented" (Story 36.2 scope).
8. **Given** a frame exceeding `max_frame_size` **Then** the codec rejects it with `FrameTooLarge` error.
9. **Given** FIBP is enabled in the server **Then** the FIBP listener starts alongside gRPC and shuts down gracefully.
10. **Given** the implementation **Then** unit tests cover: frame encode/decode round-trip, handshake success, version mismatch rejection, oversized frame rejection, heartbeat echo; and an integration test connects via raw TCP to a running server.

## Tasks / Subtasks

- [x] Task 1: Add `FibpConfig` to `BrokerConfig` in `crates/fila-core/src/broker/config.rs`
  - [x] 1.1: Define `FibpConfig` struct with serde defaults
  - [x] 1.2: Add `pub fibp: Option<FibpConfig>` to `BrokerConfig`
  - [x] 1.3: Export from `broker/mod.rs` and `lib.rs`
  - [x] 1.4: Add config tests (absent = None, defaults, TOML override)

- [x] Task 2: Create FIBP frame codec (`crates/fila-core/src/fibp/codec.rs`)
  - [x] 2.1: Define frame wire format: `[4B length][flags:u8|op:u8|corr_id:u32|payload]`
  - [x] 2.2: Implement `tokio_util::codec::Decoder` and `Encoder`
  - [x] 2.3: Define op code constants (OP_ENQUEUE through OP_GOAWAY)
  - [x] 2.4: Enforce `max_frame_size` on both encode and decode
  - [x] 2.5: Unit tests for round-trip, partial reads, oversized frames, multiple frames

- [x] Task 3: Define FIBP error types (`crates/fila-core/src/fibp/error.rs`)

- [x] Task 4: Create connection handler (`crates/fila-core/src/fibp/connection.rs`)
  - [x] 4.1: 6-byte handshake (magic + version validation)
  - [x] 4.2: Heartbeat echo dispatch
  - [x] 4.3: GoAway handling
  - [x] 4.4: "Not implemented" stubs for data/admin operations
  - [x] 4.5: Unit tests for handshake, version mismatch, heartbeat, not-implemented

- [x] Task 5: Create TCP listener (`crates/fila-core/src/fibp/listener.rs`)
  - [x] 5.1: Bind TCP, accept connections, spawn FibpConnection tasks
  - [x] 5.2: Graceful shutdown via watch channel

- [x] Task 6: Wire into server (`crates/fila-server/src/main.rs`)
  - [x] 6.1: Start FIBP listener when `config.fibp.is_some()`
  - [x] 6.2: Log FIBP address at startup
  - [x] 6.3: Shut down FIBP listener on graceful shutdown
  - [x] 6.4: Support `FILA_FIBP_PORT_FILE` env var for test harness discovery

- [x] Task 7: Integration test (`crates/fila-e2e/tests/fibp.rs`)
  - [x] 7.1: Start server with FIBP enabled, connect via raw TCP, complete handshake
  - [x] 7.2: Send heartbeat, verify echo response
  - [x] 7.3: Send data operation, verify "not implemented" error response

- [x] Task 8: Update `docs/configuration.md` with `[fibp]` section

- [x] Task 9: Add `tokio-util` (codec feature) and `tokio-stream` dependencies

## Dev Notes

- FIBP module is transport-only: does not depend on scheduler or storage. Command dispatch will be injected in Story 36.2.
- `Bytes` used for zero-copy payload handling throughout.
- The listener writes `FILA_FIBP_PORT_FILE` for test harness port discovery (same pattern as `FILA_PORT_FILE` for gRPC).
- 18 unit tests (10 codec, 3 config, 5 connection) + 2 integration tests = 20 total new tests.
