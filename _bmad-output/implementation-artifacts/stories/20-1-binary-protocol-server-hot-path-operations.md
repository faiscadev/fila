# Story 20.1: Binary Protocol Server — Hot-Path Operations

Status: ready-for-dev

## Story

As a producer/consumer,
I want to connect to Fila over a custom binary protocol for hot-path operations,
so that enqueue/consume/ack/nack have minimal transport overhead.

## Acceptance Criteria

1. **Given** the wire format spec from docs/protocol.md, **when** a TCP listener is added to fila-server on a configurable port (default 5555), **then** clients can connect over TCP and perform: batch enqueue, streaming consume, batch ack, batch nack using the binary wire format.

2. **Given** a TCP connection, **when** a client sends binary frames, **then** the server decodes frames, translates to `SchedulerCommand` batch variants, and encodes responses back to binary frames.

3. **Given** TLS is configured (`[tls]` section in config), **when** a client connects, **then** TLS wraps the TCP connection using the same cert/key/ca_file config as current gRPC TLS.

4. **Given** a new TCP connection, **when** the client sends a Handshake frame, **then** the server responds with HandshakeOk containing negotiated protocol version, node_id, and max_frame_size per the spec.

5. **Given** the binary protocol server is running, **when** gRPC is still needed for cluster comms, **then** the gRPC listener remains temporarily on a secondary port (will be removed in Story 20.5).

6. **Given** the binary protocol implementation, **when** frame encoding/decoding is implemented, **then** the codec is extracted into `crates/fila-fibp/` crate so the Rust SDK can reuse it in Story 20.3.

7. **Given** the binary protocol server, **when** integration tests run, **then** all hot-path operations (enqueue, consume, ack, nack) are verified over the binary protocol.

8. **Given** the binary protocol server, **when** `cargo bench` runs, **then** binary protocol vs gRPC throughput numbers are compared and pasted in the PR.

## Tasks / Subtasks

- [ ] Task 1: Create `crates/fila-fibp/` crate (AC: #6)
  - [ ] 1.1: Cargo.toml with `bytes`, `thiserror`, `uuid` dependencies
  - [ ] 1.2: Frame types — `RawFrame { length: u32, opcode: u8, flags: u8, request_id: u32, payload: Bytes }`
  - [ ] 1.3: Opcode enum matching docs/protocol.md (0x01-0x05 control, 0x10-0x18 hot-path, 0x20+ admin, 0xFE error)
  - [ ] 1.4: ErrorCode enum (16 codes from spec)
  - [ ] 1.5: Typed request/response structs for hot-path ops (EnqueueRequest, EnqueueResult, Delivery, AckRequest, AckResult, NackRequest, NackResult)
  - [ ] 1.6: Handshake/HandshakeOk structs
  - [ ] 1.7: Encode/decode traits and implementations — length-prefixed framing, big-endian primitives
  - [ ] 1.8: Continuation frame support (Flags bit 0) for payloads exceeding max_frame_size
  - [ ] 1.9: Unit tests for encode/decode round-trips on all frame types

- [ ] Task 2: TCP listener and connection handler in fila-server (AC: #1, #2, #4)
  - [ ] 2.1: Add `tokio::net::TcpListener` bound to configurable address (reuse `config.server.listen_addr` or add `config.server.binary_addr`)
  - [ ] 2.2: Connection accept loop spawning per-connection tasks
  - [ ] 2.3: Frame reader/writer using `tokio::io::{AsyncReadExt, AsyncWriteExt}` with length-prefix framing
  - [ ] 2.4: Handshake handler — validate protocol version, extract optional API key, respond with HandshakeOk
  - [ ] 2.5: Request dispatch — decode opcode, route to hot-path handler functions
  - [ ] 2.6: Ping/Pong keepalive handling

- [ ] Task 3: Hot-path operation handlers (AC: #2)
  - [ ] 3.1: Enqueue handler — decode EnqueueRequest, build `Vec<Message>`, send `SchedulerCommand::Enqueue`, encode EnqueueResult with per-message results
  - [ ] 3.2: Consume handler — decode ConsumeRequest, send `RegisterConsumer`, stream Delivery frames via mpsc channel, handle CancelConsume
  - [ ] 3.3: Ack handler — decode AckRequest, build `Vec<AckItem>`, send `SchedulerCommand::Ack`, encode AckResult
  - [ ] 3.4: Nack handler — decode NackRequest, build `Vec<NackItem>`, send `SchedulerCommand::Nack`, encode NackResult
  - [ ] 3.5: Error frame encoding for operation failures

- [ ] Task 4: TLS support (AC: #3)
  - [ ] 4.1: Conditionally wrap TcpStream with `tokio-rustls` acceptor using existing TlsParams config
  - [ ] 4.2: mTLS client certificate validation when ca_file is set

- [ ] Task 5: Integration tests (AC: #7)
  - [ ] 5.1: Test binary protocol enqueue + consume round-trip
  - [ ] 5.2: Test batch enqueue (100 messages) and verify all results
  - [ ] 5.3: Test batch ack and batch nack
  - [ ] 5.4: Test streaming consume with multiple deliveries
  - [ ] 5.5: Test handshake version negotiation
  - [ ] 5.6: Test TLS connection
  - [ ] 5.7: Test connection error handling (bad handshake, unknown opcode)

- [ ] Task 6: Benchmark comparison (AC: #8)
  - [ ] 6.1: Add binary protocol client to fila-bench or create standalone bench
  - [ ] 6.2: Run `cargo bench` comparing binary vs gRPC throughput
  - [ ] 6.3: Paste numbers in PR description

## Dev Notes

### Architecture

The binary protocol replaces the gRPC transport layer while keeping the same internal architecture:
- IO threads (tokio) handle TCP connections and frame encode/decode
- Commands flow through the same `crossbeam_channel` to the single-threaded scheduler
- Reply channels use the same `tokio::sync::oneshot` pattern
- Consumer delivery uses the same `tokio::sync::mpsc` channel

### Key Files to Create/Modify

**New crate — `crates/fila-fibp/`:**
- `Cargo.toml` — deps: `bytes`, `thiserror`, `uuid`
- `src/lib.rs` — re-exports
- `src/frame.rs` — RawFrame, frame reader/writer
- `src/opcode.rs` — Opcode enum
- `src/error_code.rs` — ErrorCode enum
- `src/types.rs` — typed request/response structs
- `src/codec.rs` — encode/decode implementations

**Modified — `crates/fila-server/`:**
- `Cargo.toml` — add `fila-fibp`, `tokio-rustls` deps
- `src/main.rs` — start TCP listener alongside gRPC
- `src/binary_server.rs` (new) — TCP accept loop, connection handler
- `src/binary_handlers.rs` (new) — hot-path operation handlers mapping frames to SchedulerCommand

**Modified — workspace `Cargo.toml`:**
- Add `fila-fibp` to workspace members

### Protocol Spec Reference

All frame formats, opcodes, and encoding rules are in `docs/protocol.md`. Key sections:
- §2 Frame Format: 4-byte length prefix + 6-byte header (opcode + flags + request_id) + payload
- §3 Opcodes: see table in spec
- §4 Handshake: client sends version + optional API key, server responds with version + node_id + max_frame_size
- §5 Hot-Path Operations: batch enqueue/ack/nack encoding, streaming consume
- §8 Error Codes: 16 codes
- §9 Continuation Frames: Flags bit 0 for multi-frame operations

### Existing Batch Commands (from Story 19.2)

The scheduler already accepts batch commands — no changes needed to fila-core:
- `SchedulerCommand::Enqueue { messages: Vec<Message>, reply }`
- `SchedulerCommand::Ack { items: Vec<AckItem>, reply }`
- `SchedulerCommand::Nack { items: Vec<NackItem>, reply }`

### TLS Implementation

Use `tokio-rustls` (not `tonic`'s TLS) since we're doing raw TCP:
- Load certs/keys from same `TlsParams` config
- `TlsAcceptor` wraps the TCP listener
- mTLS via `rustls::server::WebPkiClientVerifier` when `ca_file` is set

### Consumer Streaming Pattern

The consume operation is long-lived (server-push):
1. Client sends ConsumeRequest frame with queue_id and consumer_id
2. Server registers consumer via `SchedulerCommand::RegisterConsumer` with an mpsc sender
3. Server forwards `ReadyMessage` from mpsc as Delivery frames
4. Client sends CancelConsume or disconnects to unregister

### gRPC Coexistence (Temporary)

During this story, both protocols run simultaneously:
- Binary protocol on primary port (default 5555)
- gRPC moves to secondary port (e.g., 5556 or configurable)
- Cluster inter-node comms still use gRPC until Story 20.4
- Story 20.5 removes gRPC entirely

### Error Handling Pattern

Follow CLAUDE.md: explicit error mapping with per-variant matches. The fila-fibp crate should have its own error types:
```rust
#[derive(Debug, thiserror::Error)]
pub enum FrameError {
    #[error("frame too large: {size} > {max}")]
    FrameTooLarge { size: u32, max: u32 },
    #[error("unknown opcode: 0x{0:02x}")]
    UnknownOpcode(u8),
    #[error("incomplete frame: need {need} bytes, have {have}")]
    IncompleteFrame { need: usize, have: usize },
    #[error("io error")]
    Io(#[from] std::io::Error),
}
```

### Project Structure Notes

- `crates/fila-fibp/` follows workspace conventions — added to root `Cargo.toml` members
- Binary server code lives in `fila-server`, not in `fila-fibp` (fibp is codec only)
- Integration tests can go in `crates/fila-server/tests/` or `crates/fila-e2e/`

### References

- [Source: docs/protocol.md] — Complete wire format specification
- [Source: crates/fila-core/src/broker/command.rs] — SchedulerCommand batch variants
- [Source: crates/fila-server/src/main.rs] — Current server startup and TLS config
- [Source: crates/fila-server/src/service.rs] — Current HotPathService (gRPC handlers to replicate)
- [Source: crates/fila-core/src/broker/config.rs] — TlsParams, BrokerConfig
- [Source: _bmad-output/implementation-artifacts/stories/19-1-wire-format-protocol-spec.md] — Protocol design decisions
- [Source: _bmad-output/implementation-artifacts/stories/19-2-batch-native-scheduler-internals.md] — Batch command patterns

## Dev Agent Record

### Agent Model Used

### Debug Log References

### Completion Notes List

### File List
