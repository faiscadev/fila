# Story 36.3: FIBP Admin & Security

## Status: Done

## Description
Add admin operations, TLS transport security, and API key authentication to the FIBP custom binary protocol. This makes FIBP feature-complete alongside gRPC for both data and admin operations, with the same security posture.

## Acceptance Criteria

1. Admin operations over FIBP using protobuf-encoded payloads:
   - CreateQueue (OP_CREATE_QUEUE = 0x10)
   - DeleteQueue (OP_DELETE_QUEUE = 0x11)
   - GetQueueStats (OP_QUEUE_STATS = 0x12)
   - ListQueues (OP_LIST_QUEUES = 0x13)
   - Redrive (OP_REDRIVE = 0x16)
   - PauseQueue/ResumeQueue return "not implemented" (no scheduler backend)

2. TLS support: when `[tls]` is configured, FIBP connections are TLS-wrapped using the same cert/key/CA as gRPC. mTLS when client CA is configured.

3. API key auth (OP_AUTH = 0x30): when `[auth]` is configured, the first frame after handshake must be OP_AUTH with the API key. Invalid key sends error and closes connection.

4. Per-queue ACL checks on all operations using the same broker auth infrastructure as gRPC.

5. Unit tests for auth required/success/failure, admin create/list queues.

6. E2E tests for auth success, auth failure, admin create+list queues over FIBP.

7. Documentation updated to note FIBP shares TLS and auth configuration with gRPC.

## Technical Design

### Admin Operations
Admin payloads use protobuf encoding (reusing existing proto message types from `fila_proto`) rather than custom binary encoding. This provides schema evolution for admin operations while keeping data operations optimized with custom binary encoding.

Dispatch functions in `dispatch.rs` decode protobuf, call `Broker` methods via `SchedulerCommand`, and encode protobuf responses.

### TLS
`FibpListener::start()` accepts optional `TlsParams`. When present, it builds a `tokio_rustls::TlsAcceptor` and wraps each TCP connection before the FIBP handshake. The TLS code path duplicates the frame dispatch logic since `FibpConnection` is typed on `TcpStream`.

### Auth
`FibpConnection` tracks `authenticated: bool` and `caller: Option<CallerKey>`. When auth is enabled, all operations except OP_AUTH, OP_HEARTBEAT, and OP_GOAWAY require authentication. The `Broker::validate_api_key()` method is reused from the gRPC auth layer.

### ACL
Data operations check `check_permission()` with the appropriate `Permission` variant (Produce for enqueue, Consume for consume/ack/nack). Admin operations check `check_permission(Admin, "*")`.

## Files Changed
- `crates/fila-core/src/fibp/dispatch.rs` — admin dispatch functions
- `crates/fila-core/src/fibp/connection.rs` — auth tracking, admin op routing, ACL checks
- `crates/fila-core/src/fibp/listener.rs` — TLS support, TLS frame dispatch
- `crates/fila-core/src/fibp/error.rs` — new error variants (AuthFailed, PermissionDenied, ProtobufDecode, TlsConfig, Storage)
- `crates/fila-core/src/fibp/mod.rs` — unchanged (op codes already defined)
- `crates/fila-server/src/main.rs` — pass TLS params to FIBP listener
- `crates/fila-core/Cargo.toml` — added tokio-rustls, rustls-pemfile
- `crates/fila-e2e/tests/fibp.rs` — 3 new e2e tests
- `crates/fila-e2e/Cargo.toml` — added fila-proto, prost deps
- `docs/configuration.md` — FIBP shares TLS/auth docs
- `Cargo.toml` — workspace deps for tokio-rustls, rustls-pemfile
