# Story 20.3: Rust SDK Binary Protocol Client

Status: done

## Story

As a Rust developer using fila-sdk,
I want the SDK to communicate over the binary protocol,
so that my application benefits from the lower-overhead transport.

## Acceptance Criteria

1. **Given** the binary protocol server from Stories 20.1-20.2, **when** fila-sdk is updated to use the binary protocol instead of gRPC, **then** all existing SDK operations work: enqueue (single and batch), consume (streaming), ack (single and batch), nack (single and batch), and all admin operations.

2. **Given** fila-sdk, **when** TLS and API key auth are configured via ConnectOptions, **then** they work through the binary protocol SDK.

3. **Given** fila-sdk, **when** the public API is evaluated, **then** it is unchanged or improved (batch operations are first-class, single = batch of 1).

4. **Given** fila-sdk, **when** connection errors or disconnects occur, **then** the SDK handles reconnection and leader hints the same as before.

5. **Given** fila-sdk, **when** the fila-fibp crate is used for frame encoding/decoding, **then** there is no duplicate codec implementation.

6. **Given** all existing e2e tests in crates/fila-e2e/, **when** they run using the binary protocol SDK, **then** they pass.

## Tasks / Subtasks

- [x] Task 1: Replace gRPC internals with binary protocol connection (AC: #1, #5)
  - [x] 1.1: Replace FilaServiceClient<Channel> with TCP connection + fila-fibp codec
  - [x] 1.2: Connection management — handshake on connect, request ID tracking
  - [x] 1.3: Request-response correlation using request IDs
  - [x] 1.4: Remove tonic/prost dependencies, add fila-fibp + tokio-rustls

- [x] Task 2: Implement all operations over binary protocol (AC: #1)
  - [x] 2.1: enqueue — single message (batch of 1) and batch variant
  - [x] 2.2: consume — streaming delivery via background read task
  - [x] 2.3: ack — single and batch
  - [x] 2.4: nack — single and batch

- [x] Task 3: TLS and auth (AC: #2)
  - [x] 3.1: TLS via tokio-rustls (system roots, custom CA, mTLS identity)
  - [x] 3.2: API key sent in handshake frame

- [x] Task 4: Leader hint reconnection (AC: #4)
  - [x] 4.1: Parse NotLeader error with leader_addr metadata
  - [x] 4.2: Reconnect to leader transparently in consume

- [x] Task 5: Update e2e tests to use binary protocol (AC: #6)
  - [x] 5.1: Update fila-e2e test helpers to start server with binary_addr
  - [x] 5.2: Update SDK connection in tests to use binary protocol address
  - [x] 5.3: All existing e2e tests pass

## Dev Notes

### Key Changes

The SDK currently uses tonic gRPC internally. Replace with:
- TCP connection via `tokio::net::TcpStream`
- Frame encode/decode via `fila-fibp` crate
- TLS via `tokio-rustls` (matching server's implementation)
- Streaming consume via background read task + mpsc channel

### Public API Preservation

The public API surface (ConnectOptions, FilaClient, ConsumeMessage, error types) must remain unchanged. The binary protocol is an internal implementation detail.

The address format changes: users provide `host:port` instead of `http://host:port`. Support both formats for backward compatibility.

### References

- [Source: crates/fila-sdk/src/client.rs] — Current gRPC SDK implementation
- [Source: crates/fila-sdk/src/error.rs] — Error types
- [Source: crates/fila-fibp/src/types.rs] — Binary protocol types
- [Source: crates/fila-server/src/binary_server.rs] — Server-side reference

## Dev Agent Record

### Agent Model Used

### Completion Notes List

### File List
