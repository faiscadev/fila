# Story 9.1: Go Client SDK

Status: review

## Story

As a Go developer,
I want an idiomatic Go client SDK for Fila,
so that I can integrate Fila into my Go services with minimal effort.

## Acceptance Criteria

1. **Given** the Fila proto definitions are available, **when** the Go SDK is built, **then** the SDK lives in a separate repository (`fila-go`)
2. **Given** the repo is created, **when** proto files are set up, **then** proto files are copied into the repo and generated Go code is committed (no submodules, no Buf)
3. **Given** proto files are in the repo, **when** gRPC stubs are generated, **then** `protoc-gen-go` and `protoc-gen-go-grpc` produce Go types and client stubs
4. **Given** the Go stubs exist, **when** the ergonomic wrapper is built, **then** `client.Enqueue(ctx, queue, headers, payload) (string, error)` is available
5. **Given** the wrapper is built, **when** streaming consume is implemented, **then** `client.Consume(ctx, queue) (<-chan *ConsumeMessage, error)` returns a channel for streaming consumption
6. **Given** the ConsumeMessage type, **when** inspected, **then** it includes: `ID`, `Headers`, `Payload`, `FairnessKey`, `AttemptCount`, `Queue`
7. **Given** the wrapper is built, **when** ack/nack are implemented, **then** `client.Ack(ctx, queue, msgID) error` and `client.Nack(ctx, queue, msgID, errMsg) error` are available
8. **Given** the client API, **when** all methods are called, **then** all methods accept `context.Context` for cancellation and deadlines
9. **Given** the error types, **when** operations fail, **then** per-operation error types are defined (e.g., `ErrQueueNotFound`, `ErrMessageNotFound`) checkable via `errors.Is`
10. **Given** the SDK is built, **when** it follows Go conventions, **then** exported types, error returns, and godoc comments are idiomatic
11. **Given** a running `fila-server` binary, **when** integration tests execute, **then** all four operations (enqueue, consume, ack, nack) are verified end-to-end
12. **Given** the repo is created, **when** CI is configured, **then** GitHub Actions runs lint (`golangci-lint`), test, and build on every PR
13. **Given** the repo is created, **when** documentation is added, **then** a README with usage examples is included

## Tasks / Subtasks

- [x] Task 1: Create `fila-go` repository structure (AC: #1, #2, #3)
  - [x] Subtask 1.1: Initialize Go module (`github.com/faisca/fila-go`)
  - [x] Subtask 1.2: Copy proto files from `fila` repo: `proto/fila/v1/messages.proto`, `proto/fila/v1/service.proto`, `proto/fila/v1/admin.proto` (admin for test helpers)
  - [x] Subtask 1.3: Set up proto generation with `protoc-gen-go` and `protoc-gen-go-grpc`, generate Go code, commit output
  - [x] Subtask 1.4: Verify generated stubs compile and produce correct types

- [x] Task 2: Implement `Client` struct and connection (AC: #4, #8, #10)
  - [x] Subtask 2.1: Create `client.go` with `Client` struct wrapping `FilaServiceClient`
  - [x] Subtask 2.2: `Dial(addr string, opts ...DialOption) (*Client, error)` — wraps `grpc.NewClient`
  - [x] Subtask 2.3: `DialOption` with `WithGRPCDialOption` for extensibility
  - [x] Subtask 2.4: `Close() error` to close the underlying gRPC connection

- [x] Task 3: Implement `Enqueue` method (AC: #4, #8, #9)
  - [x] Subtask 3.1: `Enqueue(ctx, queue, headers, payload) (string, error)` — returns message ID
  - [x] Subtask 3.2: Map gRPC NOT_FOUND to `ErrQueueNotFound`

- [x] Task 4: Implement `Consume` method (AC: #5, #6, #8, #9)
  - [x] Subtask 4.1: `Consume(ctx, queue) (<-chan *ConsumeMessage, error)` — returns receive-only channel
  - [x] Subtask 4.2: Background goroutine reads server stream, sends to channel, closes channel on stream end/error
  - [x] Subtask 4.3: `ConsumeMessage` struct with `ID`, `Headers`, `Payload`, `FairnessKey`, `AttemptCount`, `Queue`
  - [x] Subtask 4.4: Skip nil message frames (keepalive signals)
  - [x] Subtask 4.5: Respect context cancellation — stop goroutine when ctx is cancelled

- [x] Task 5: Implement `Ack` and `Nack` methods (AC: #7, #8, #9)
  - [x] Subtask 5.1: `Ack(ctx, queue, msgID) error` — map NOT_FOUND to `ErrMessageNotFound`
  - [x] Subtask 5.2: `Nack(ctx, queue, msgID, errMsg) error` — map NOT_FOUND to `ErrMessageNotFound`

- [x] Task 6: Per-operation error types (AC: #9)
  - [x] Subtask 6.1: Define sentinel errors: `ErrQueueNotFound`, `ErrMessageNotFound`
  - [x] Subtask 6.2: Wrap with operation context so `errors.Is(err, ErrQueueNotFound)` works
  - [x] Subtask 6.3: General `RPCError` for unexpected gRPC failures preserving code + message

- [x] Task 7: Integration tests (AC: #11)
  - [x] Subtask 7.1: Test helper: start `fila-server` binary on random port with temp data dir, wait for ready, cleanup on test end
  - [x] Subtask 7.2: Test helper: create queue via direct gRPC admin call (SDK doesn't wrap admin ops)
  - [x] Subtask 7.3: Test `TestEnqueueConsumeAck` lifecycle: enqueue → consume → verify message fields → ack
  - [x] Subtask 7.4: Test `TestEnqueueConsumeNackRedeliver`: enqueue → consume → nack → redelivery on same stream → verify AttemptCount incremented
  - [x] Subtask 7.5: Test `TestEnqueueNonexistentQueue`: enqueue to missing queue → verify `ErrQueueNotFound`

- [x] Task 8: CI pipeline (AC: #12)
  - [x] Subtask 8.1: `.github/workflows/ci.yml` — trigger on PR and push to main
  - [x] Subtask 8.2: Steps: checkout, setup Go, golangci-lint, vet, build, tests
  - [x] Subtask 8.3: Integration tests skip gracefully when fila-server binary not found

- [x] Task 9: README and documentation (AC: #13)
  - [x] Subtask 9.1: README with: installation (`go get`), quickstart example (enqueue/consume/ack), error handling, API reference overview
  - [x] Subtask 9.2: Godoc comments on all exported types and methods

## Dev Notes

### Repository Structure

This SDK lives in a **separate repository** (`fila-go`), not inside the fila Cargo workspace. Create it at a sibling path or in a suitable location.

```
fila-go/
├── go.mod
├── go.sum
├── README.md
├── LICENSE                          # AGPLv3
├── client.go                       # Client struct, Dial, Close
├── enqueue.go                      # Enqueue method
├── consume.go                      # Consume method + ConsumeMessage
├── ack.go                          # Ack method
├── nack.go                         # Nack method
├── errors.go                       # Per-operation error types + sentinels
├── client_test.go                  # Integration tests
├── proto/
│   └── fila/
│       └── v1/
│           ├── messages.proto
│           └── service.proto
├── filav1/                         # Generated Go code (committed)
│   ├── messages.pb.go
│   ├── service.pb.go
│   └── service_grpc.pb.go
├── .github/
│   └── workflows/
│       └── ci.yml
└── .golangci.yml                   # Linter config
```

### Proto Generation

Copy only the proto files needed for the hot-path SDK:
- `messages.proto` — Message envelope, metadata types
- `service.proto` — FilaService RPCs (Enqueue, Consume, Ack, Nack)

Do **NOT** copy `admin.proto` — the SDK covers hot-path only.

Generation command:
```bash
protoc --go_out=. --go_opt=paths=source_relative \
       --go-grpc_out=. --go-grpc_opt=paths=source_relative \
       proto/fila/v1/messages.proto proto/fila/v1/service.proto
```

The `go_package` option must be added to proto files for Go codegen:
```protobuf
option go_package = "github.com/faisca/fila-go/filav1";
```

Commit the generated `filav1/*.pb.go` files — consumers should not need protoc installed.

### API Design — Mirror Rust SDK Pattern

The Rust SDK (`fila-sdk`) established the API shape. Mirror it in Go idioms:

| Rust SDK | Go SDK |
|----------|--------|
| `FilaClient::connect(addr)` | `fila.Dial(addr, opts...)` |
| `client.enqueue(queue, headers, payload)` | `client.Enqueue(ctx, queue, headers, payload)` |
| `client.consume(queue)` → `Stream<ConsumeMessage>` | `client.Consume(ctx, queue)` → `<-chan *ConsumeMessage` |
| `client.ack(queue, msg_id)` | `client.Ack(ctx, queue, msgID)` |
| `client.nack(queue, msg_id, error)` | `client.Nack(ctx, queue, msgID, errMsg)` |

### Error Pattern

Per-operation errors following Go conventions. Use sentinel errors checkable via `errors.Is`:

```go
var (
    ErrQueueNotFound   = errors.New("queue not found")
    ErrMessageNotFound = errors.New("message not found")
)

// Wrap with context:
fmt.Errorf("enqueue: %w", ErrQueueNotFound)
```

For unexpected gRPC errors, preserve the status code and message in a structured error type.

### Streaming Consume Design

`Consume` returns a receive-only channel. A background goroutine reads from the gRPC server stream and sends `ConsumeMessage` values. The channel is closed when:
- The server stream ends
- The context is cancelled
- A stream error occurs

Skip `nil` message frames (keepalive signals from the server).

```go
func (c *Client) Consume(ctx context.Context, queue string) (<-chan *ConsumeMessage, error) {
    stream, err := c.svc.Consume(ctx, &filav1.ConsumeRequest{Queue: queue})
    if err != nil {
        return nil, mapConsumeError(err)
    }
    ch := make(chan *ConsumeMessage)
    go func() {
        defer close(ch)
        for {
            resp, err := stream.Recv()
            if err != nil {
                return // stream ended or error
            }
            if resp.Message == nil {
                continue // keepalive
            }
            // convert and send...
        }
    }()
    return ch, nil
}
```

### Integration Test Setup

Tests need a running `fila-server`. Follow the same subprocess pattern as fila-sdk/fila-e2e:

1. Build `fila-server` binary (from the fila repo) or have it available on PATH
2. Start on a random available port with a temp data dir
3. Wait for the server to be ready (poll the gRPC endpoint)
4. Run tests using the Go SDK client
5. Kill server and cleanup temp dir on test completion

For queue creation in tests, use the generated admin gRPC client directly (not wrapped by SDK). You may need to also copy `admin.proto` into a separate internal test package, or use `service.proto` admin RPCs if available. Actually, admin RPCs are in `admin.proto` with a separate `FilaAdmin` service — for tests, generate admin stubs in a test-only package or just use the `google.golang.org/grpc` client directly.

**Simpler approach**: copy `admin.proto` into the repo for test-only use, generate its stubs, and use them in test helpers. Do not export the admin client in the public SDK API.

### Go Dependencies

```
google.golang.org/grpc       # gRPC client
google.golang.org/protobuf   # Protobuf runtime
```

### CI Notes

The CI pipeline **must** be a first-class deliverable (CLAUDE.md requirement). Integration tests require a `fila-server` binary. Options:
1. **Cross-compile fila-server in CI** — install Rust toolchain, `cargo build --release -p fila-server`, use the binary
2. **Download pre-built release** — use GitHub Releases if available
3. **Skip integration tests in CI initially** — mark them with build tags, run unit tests only

Option 1 is most reliable for ensuring tests run. Include Rust toolchain setup in the CI workflow.

### What NOT To Do

- Do NOT add admin operations (CreateQueue, DeleteQueue, etc.) to the public SDK API — hot-path only
- Do NOT implement retry/reconnection logic — keep SDK thin
- Do NOT use `buf` or proto submodules — copy and commit
- Do NOT make `ConsumeMessage` channel buffered with a large buffer — use unbuffered or small buffer (1-8) so backpressure propagates
- Do NOT expose raw gRPC types in the public API — wrap everything in SDK types

### References

- [Source: proto/fila/v1/service.proto — RPC definitions (Enqueue, Consume, Ack, Nack)]
- [Source: proto/fila/v1/messages.proto — Message, MessageMetadata types]
- [Source: crates/fila-sdk/src/client.rs — Rust SDK pattern to mirror]
- [Source: crates/fila-sdk/src/error.rs — Per-operation error type pattern]
- [Source: _bmad-output/planning-artifacts/epics.md#Story 9.1: Go Client SDK]
- [Source: _bmad-output/implementation-artifacts/epic-8-retro-2026-02-19.md — Proto distribution decisions]

## Dev Agent Record

### Agent Model Used

Claude Opus 4.6

### Debug Log References

### Completion Notes List

- Created `fila-go` repository at `/Users/lucas/code/faisca/fila-go/` as a separate Go module
- Proto files copied with `go_package` option added for Go codegen; generated stubs committed in `filav1/`
- Admin proto included for test helpers (queue creation) — not exported in SDK public API
- `Client` struct wraps `FilaServiceClient` via `grpc.NewClient` (lazy connection)
- `Consume` uses background goroutine with channel-based delivery; respects context cancellation; skips nil keepalive frames
- Per-operation sentinel errors (`ErrQueueNotFound`, `ErrMessageNotFound`) checkable via `errors.Is` with `fmt.Errorf("%w")` wrapping
- `RPCError` struct preserves gRPC status code and message for unexpected failures
- 3 integration tests all pass against real `fila-server` subprocess (each gets own server instance with random port + temp dir)
- fila-server config discovery: writes `fila.toml` to temp dir (not CLI flags) + `FILA_DATA_DIR` env var
- golangci-lint clean, go vet clean, all 278 existing fila tests pass (zero regressions)

### File List

All files in the separate `fila-go` repository:
- `go.mod` (new)
- `go.sum` (new)
- `.gitignore` (new)
- `.golangci.yml` (new)
- `.github/workflows/ci.yml` (new)
- `LICENSE` (new — AGPLv3)
- `README.md` (new)
- `client.go` (new)
- `enqueue.go` (new)
- `consume.go` (new)
- `ack.go` (new)
- `nack.go` (new)
- `errors.go` (new)
- `client_test.go` (new)
- `proto/fila/v1/messages.proto` (new — copied with go_package)
- `proto/fila/v1/service.proto` (new — copied with go_package)
- `proto/fila/v1/admin.proto` (new — copied with go_package, for test helpers)
- `filav1/messages.pb.go` (new — generated)
- `filav1/service.pb.go` (new — generated)
- `filav1/service_grpc.pb.go` (new — generated)
- `filav1/admin.pb.go` (new — generated)
- `filav1/admin_grpc.pb.go` (new — generated)

Tracking files in `fila` repository:
- `_bmad-output/implementation-artifacts/9-1-go-client-sdk.md` (new — story spec)
- `_bmad-output/implementation-artifacts/sprint-status.yaml` (modified)
- `_bmad-output/epic-execution-state.yaml` (modified)
