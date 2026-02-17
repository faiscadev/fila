# Story 7.1: Rust Client SDK

Status: done

## Story

As a Rust developer,
I want an idiomatic Rust client SDK for Fila,
so that I can integrate message enqueue, lease, ack, and nack into my Rust application.

## Acceptance Criteria

1. **Given** the Fila proto definitions are available, **when** the Rust SDK crate is built, **then** a `fila-sdk` crate exists in the workspace as a thin wrapper over tonic gRPC clients from `fila-proto`
2. **Given** a running Fila broker, **when** a client connects, **then** `FilaClient::connect(addr)` returns a connected client instance
3. **Given** a connected client and an existing queue, **when** `client.enqueue(queue, headers, payload)` is called, **then** a `Result<String>` is returned containing the assigned message ID
4. **Given** a connected client and a queue with messages, **when** `client.lease(queue)` is called, **then** a `Result<impl Stream<Item = Result<LeaseMessage>>>` is returned for streaming consumption
5. **Given** a connected client and a leased message, **when** `client.ack(queue, msg_id)` is called, **then** a `Result<()>` is returned confirming acknowledgment
6. **Given** a connected client and a leased message, **when** `client.nack(queue, msg_id, error)` is called, **then** a `Result<()>` is returned confirming negative acknowledgment
7. **Given** the client API, **when** connection is configured, **then** it accepts address, optional timeout duration, and optional TLS config (future-proofing field, not wired)
8. **Given** the client types, **when** inspected, **then** all public types implement `Send + Sync` and use async/await with `Result<T, E>` return types
9. **Given** a running broker, **when** integration tests execute, **then** all four operations (enqueue, lease, ack, nack) are verified end-to-end

## Tasks / Subtasks

- [x] Task 1: Create `fila-sdk` crate in workspace (AC: #1)
  - [x] Subtask 1.1: Create `crates/fila-sdk/` with `Cargo.toml` and `src/lib.rs`
  - [x] Subtask 1.2: Add `fila-sdk` to workspace members in root `Cargo.toml`
  - [x] Subtask 1.3: Add `fila-sdk = { path = "crates/fila-sdk" }` to `[workspace.dependencies]`
  - [x] Subtask 1.4: Dependencies: `fila-proto` (workspace), `tonic` (workspace), `tokio-stream` (workspace), `thiserror` (workspace)

- [x] Task 2: Define SDK error type (AC: #3, #4, #5, #6)
  - [x] Subtask 2.1: Create `src/error.rs` with `ClientError` enum
  - [x] Subtask 2.2: Variants: `Connect(tonic::transport::Error)`, `QueueNotFound(String)`, `MessageNotFound(String)`, `InvalidArgument(String)`, `AlreadyExists(String)`, `Unavailable(String)`, `Internal(String)`, `Rpc { code, message }`
  - [x] Subtask 2.3: Context-aware status mapping via `queue_status_error()` and `message_status_error()` — matches on `status.code()` explicitly per CLAUDE.md rules

- [x] Task 3: Implement `FilaClient` struct (AC: #2, #7, #8)
  - [x] Subtask 3.1: Create `src/client.rs` with `FilaClient` struct holding `FilaServiceClient<Channel>`
  - [x] Subtask 3.2: `FilaClient::connect(addr: impl Into<String>) -> Result<Self, ClientError>` — wraps `FilaServiceClient::connect`
  - [x] Subtask 3.3: `ConnectOptions` struct with `addr: String`, `timeout: Option<Duration>` and `connect_with_options()` method
  - [x] Subtask 3.4: `FilaClient` derives `Clone` and is `Send + Sync` (tonic Channel is clone-safe)

- [x] Task 4: Implement hot-path methods (AC: #3, #4, #5, #6)
  - [x] Subtask 4.1: `enqueue(&self, queue, headers, payload) -> Result<String, ClientError>` — returns message ID
  - [x] Subtask 4.2: `lease(&self, queue) -> Result<impl Stream<Item = Result<LeaseMessage, ClientError>>, ClientError>` — wraps streaming response with filter_map for None messages
  - [x] Subtask 4.3: `ack(&self, queue, message_id) -> Result<(), ClientError>`
  - [x] Subtask 4.4: `nack(&self, queue, message_id, error) -> Result<(), ClientError>`
  - [x] Subtask 4.5: `LeaseMessage` struct with fields: `id`, `headers`, `payload`, `fairness_key`, `attempt_count`, `queue` — converted from proto `LeaseResponse`

- [x] Task 5: Public API surface in `lib.rs` (AC: #1, #8)
  - [x] Subtask 5.1: Re-export `FilaClient`, `ClientError`, `LeaseMessage`, `ConnectOptions`
  - [x] Subtask 5.2: Re-export `fila_proto` as `pub use fila_proto as proto` for advanced users

- [x] Task 6: Integration tests (AC: #9)
  - [x] Subtask 6.1: `TestServer` helper that spawns `fila-server` on random port with temp data dir, polls for readiness, kills on Drop
  - [x] Subtask 6.2: Test `enqueue_lease_ack_lifecycle`: enqueue → lease → verify message → ack → verify double-ack returns MessageNotFound
  - [x] Subtask 6.3: Test `enqueue_lease_nack_release`: enqueue → lease → nack → receive redelivery on same stream → verify attempt_count incremented
  - [x] Subtask 6.4: Test `enqueue_to_nonexistent_queue`: enqueue to missing queue → verify QueueNotFound error

## Dev Notes

### Architecture Compliance

- **Crate organization**: `fila-sdk` is a new workspace member at `crates/fila-sdk/`. It depends only on `fila-proto` and `tonic` — no dependency on `fila-core` or `fila-server`
- **Thin wrapper**: The SDK wraps tonic-generated `FilaServiceClient<Channel>` from `fila-proto::fila_service_client`. Do NOT reimplement gRPC plumbing
- **Error handling**: Per CLAUDE.md rules, match on `tonic::Status` code explicitly (not `.to_string()` catch-all). Map each gRPC status code to a specific `ClientError` variant

### Existing Patterns to Follow

- **CLI client pattern**: `crates/fila-cli/src/main.rs` already constructs `FilaAdminClient::connect(addr)` and calls RPCs. The SDK follows the same pattern but wraps `FilaServiceClient` for hot-path operations
- **Request construction**: Build proto request structs directly (`EnqueueRequest { queue, headers, payload }`) and call `client.method(request).await`
- **Streaming response**: `client.lease(request).await` returns `Result<Response<Streaming<LeaseResponse>>, Status>`. The SDK should map `Streaming<LeaseResponse>` into a `Stream<Item = Result<LeaseMessage, ClientError>>` using `StreamExt::map`

### Key Proto Types (from `fila-proto`)

- **Client**: `fila_service_client::FilaServiceClient<Channel>` — hot-path client
- **Requests**: `EnqueueRequest { queue, headers, payload }`, `LeaseRequest { queue }`, `AckRequest { queue, message_id }`, `NackRequest { queue, message_id, error }`
- **Responses**: `EnqueueResponse { message_id }`, `LeaseResponse { message: Option<Message> }`, `AckResponse {}`, `NackResponse {}`
- **Message**: `Message { id, headers, payload, metadata: Option<MessageMetadata>, timestamps }`
- **MessageMetadata**: `{ fairness_key, weight, throttle_keys, attempt_count, queue_id }`

### LeaseMessage Design

Create a `LeaseMessage` struct that provides an ergonomic view over the proto `Message`:

```rust
pub struct LeaseMessage {
    pub id: String,
    pub headers: HashMap<String, String>,
    pub payload: Vec<u8>,
    pub fairness_key: String,
    pub attempt_count: u32,
    pub queue: String,
}
```

Convert from `LeaseResponse` in the stream mapping — handle `None` message field by skipping (tonic may send empty keepalive frames).

### Integration Test Setup

For integration tests, the SDK crate needs to start a real `fila-server`. Pattern:
1. `fila-sdk/Cargo.toml` → `[dev-dependencies]` include `fila-server` (or build the binary and spawn as subprocess)
2. Prefer subprocess approach: build `fila-server` binary, spawn with random port and temp dir, wait for ready, run tests, kill on drop
3. For queue creation in tests, use `fila_proto::fila_admin_client::FilaAdminClient` directly (the SDK doesn't wrap admin operations)
4. Tests must be independent — each gets its own server instance (or at minimum its own queue names)

### Dependencies (Cargo.toml)

```toml
[dependencies]
fila-proto = { workspace = true }
tonic = { workspace = true }
tokio-stream = "0.1"
thiserror = { workspace = true }

[dev-dependencies]
tokio = { workspace = true }
```

Add `tokio-stream` to `[workspace.dependencies]` in root Cargo.toml if not already present.

### What NOT To Do

- Do NOT add admin operations (CreateQueue, DeleteQueue, etc.) to `FilaClient`. The SDK covers hot-path only. Admin ops are for `fila` CLI
- Do NOT depend on `fila-core` or `fila-server` as runtime dependencies
- Do NOT implement retry/reconnection logic (keep the SDK thin; consumers handle retries)
- Do NOT add TLS implementation — just accept the field in options for future use

### Project Structure Notes

```
crates/fila-sdk/
├── Cargo.toml
├── src/
│   ├── lib.rs          # Re-exports: FilaClient, ClientError, LeaseMessage, ConnectOptions
│   ├── client.rs       # FilaClient struct and method implementations
│   └── error.rs        # ClientError enum with From<tonic::Status>
└── tests/
    └── integration.rs  # E2E tests against running broker
```

### References

- [Source: _bmad-output/planning-artifacts/epics.md#Epic 7: Rust Client SDK]
- [Source: _bmad-output/planning-artifacts/architecture.md#Wire Protocol — gRPC]
- [Source: _bmad-output/planning-artifacts/architecture.md#Error Handling Strategy]
- [Source: crates/fila-cli/src/main.rs — existing gRPC client pattern]
- [Source: crates/fila-proto/src/lib.rs — generated type re-exports]

## Dev Agent Record

### Agent Model Used

Claude Opus 4.6

### Debug Log References

### Completion Notes List

- Implemented `fila-sdk` crate as thin wrapper over tonic-generated `FilaServiceClient<Channel>`
- Context-aware error mapping: `queue_status_error()` maps NOT_FOUND → QueueNotFound, `message_status_error()` maps NOT_FOUND → MessageNotFound
- `LeaseMessage` flattens proto `Message` + `MessageMetadata` into ergonomic struct
- Lease stream uses `filter_map` to skip `None` message fields (empty keepalive frames)
- `FilaClient` is `Clone + Send + Sync` — safe to share across tokio tasks
- Integration tests spawn real `fila-server` subprocess per test with random ports and temp dirs
- All 3 integration tests pass; all 267 existing tests pass (zero regressions)

### File List

- `Cargo.toml` (modified — added fila-sdk to workspace members, added tokio-stream and fila-sdk to workspace deps)
- `crates/fila-sdk/Cargo.toml` (new)
- `crates/fila-sdk/src/lib.rs` (new)
- `crates/fila-sdk/src/client.rs` (new)
- `crates/fila-sdk/src/error.rs` (new)
- `crates/fila-sdk/tests/integration.rs` (new)
- `_bmad-output/implementation-artifacts/7-1-rust-client-sdk.md` (new — story file)
- `_bmad-output/implementation-artifacts/sprint-status.yaml` (modified — epic-7 in-progress, story 7-1 in-progress)
- `_bmad-output/epic-execution-state.yaml` (modified — epic 7 execution state)
