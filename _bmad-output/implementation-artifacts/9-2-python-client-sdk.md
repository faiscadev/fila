# Story 9.2: Python Client SDK

Status: ready-for-dev

## Story

As a Python developer,
I want an idiomatic Python client SDK for Fila,
so that I can integrate Fila into my Python applications with minimal effort.

## Acceptance Criteria

1. **Given** the Fila proto definitions are available, **when** the Python SDK is built, **then** the SDK lives in a separate repository (`fila-python`)
2. **Given** the repo is created, **when** proto files are set up, **then** proto files are copied into the repo and generated Python code is committed (no submodules, no Buf)
3. **Given** proto files are in the repo, **when** gRPC stubs are generated, **then** `grpcio-tools` produces Python types and client stubs
4. **Given** the Python stubs exist, **when** the sync wrapper is built, **then** `client.enqueue(queue, headers, payload) -> str` is available
5. **Given** the wrapper is built, **when** sync consume is implemented, **then** `client.consume(queue)` returns an iterator of `ConsumeMessage` objects usable with `for msg in client.consume(queue):`
6. **Given** the wrapper is built, **when** async consume is implemented, **then** `client.consume(queue)` on the async client returns an async iterator usable with `async for msg in client.consume(queue):`
7. **Given** the ConsumeMessage type, **when** inspected, **then** it includes: `id`, `headers`, `payload`, `fairness_key`, `attempt_count`, `queue`
8. **Given** the wrapper is built, **when** ack/nack are implemented, **then** `client.ack(queue, msg_id)` and `client.nack(queue, msg_id, error)` are available on both sync and async clients
9. **Given** the SDK, **when** both interfaces are used, **then** `Client` provides synchronous operations and `AsyncClient` provides async/await operations
10. **Given** the error types, **when** operations fail, **then** per-operation exception classes are defined (`QueueNotFoundError`, `MessageNotFoundError`) as subclasses of a base `FilaError`
11. **Given** the SDK is built, **when** it follows Python conventions, **then** snake_case methods, type hints on all public APIs, and docstrings are present
12. **Given** a running `fila-server` binary, **when** integration tests execute, **then** all four operations (enqueue, consume, ack, nack) are verified end-to-end for both sync and async clients
13. **Given** the repo is created, **when** CI is configured, **then** GitHub Actions runs lint (`ruff`), type check (`mypy`), and test (`pytest`) on every PR
14. **Given** the repo is created, **when** packaging is set up, **then** the package is installable via `pip install fila-python` using a `pyproject.toml`
15. **Given** the repo is created, **when** documentation is added, **then** a README with usage examples for both sync and async is included

## Tasks / Subtasks

- [ ] Task 1: Create `fila-python` repository structure (AC: #1, #2, #3, #14)
  - [ ] Subtask 1.1: Initialize repository with `pyproject.toml` (package name: `fila-python`, import name: `fila`)
  - [ ] Subtask 1.2: Copy proto files from `fila` repo: `proto/fila/v1/messages.proto`, `proto/fila/v1/service.proto`, `proto/fila/v1/admin.proto` (admin for test helpers)
  - [ ] Subtask 1.3: Generate Python stubs with `grpcio-tools` (`python -m grpc_tools.protoc`), commit output to `fila/_proto/`
  - [ ] Subtask 1.4: Verify generated stubs import and produce correct types

- [ ] Task 2: Implement `Client` (sync) and `AsyncClient` (async) (AC: #4, #8, #9, #11)
  - [ ] Subtask 2.1: Create `fila/client.py` with `Client` class wrapping sync `grpc.insecure_channel`
  - [ ] Subtask 2.2: Create `fila/async_client.py` with `AsyncClient` class wrapping `grpc.aio.insecure_channel`
  - [ ] Subtask 2.3: Both classes: `__init__(self, addr: str)`, `close()`, context manager support (`__enter__`/`__exit__`, `__aenter__`/`__aexit__`)
  - [ ] Subtask 2.4: Type hints on all public methods, docstrings on all public classes/methods

- [ ] Task 3: Implement `enqueue` method (AC: #4, #9)
  - [ ] Subtask 3.1: `Client.enqueue(queue, headers, payload) -> str` — returns message ID
  - [ ] Subtask 3.2: `AsyncClient.enqueue(queue, headers, payload) -> str` — async version
  - [ ] Subtask 3.3: Map gRPC NOT_FOUND to `QueueNotFoundError`

- [ ] Task 4: Implement `consume` method (AC: #5, #6, #7, #9)
  - [ ] Subtask 4.1: `Client.consume(queue)` — returns `Iterator[ConsumeMessage]` (sync iterator)
  - [ ] Subtask 4.2: `AsyncClient.consume(queue)` — returns `AsyncIterator[ConsumeMessage]` (async iterator)
  - [ ] Subtask 4.3: `ConsumeMessage` dataclass with `id`, `headers`, `payload`, `fairness_key`, `attempt_count`, `queue`
  - [ ] Subtask 4.4: Skip nil message frames (keepalive signals)

- [ ] Task 5: Implement `ack` and `nack` methods (AC: #8, #9, #10)
  - [ ] Subtask 5.1: `Client.ack(queue, msg_id)` and `AsyncClient.ack(queue, msg_id)` — map NOT_FOUND to `MessageNotFoundError`
  - [ ] Subtask 5.2: `Client.nack(queue, msg_id, error)` and `AsyncClient.nack(queue, msg_id, error)` — map NOT_FOUND to `MessageNotFoundError`

- [ ] Task 6: Exception hierarchy (AC: #10)
  - [ ] Subtask 6.1: Define `FilaError` base exception
  - [ ] Subtask 6.2: `QueueNotFoundError(FilaError)` and `MessageNotFoundError(FilaError)`
  - [ ] Subtask 6.3: `RPCError(FilaError)` for unexpected gRPC failures, preserving status code and message

- [ ] Task 7: Integration tests (AC: #12)
  - [ ] Subtask 7.1: Test helper: start `fila-server` binary on random port with temp data dir, wait for ready, cleanup on test end (pytest fixture)
  - [ ] Subtask 7.2: Test helper: create queue via direct gRPC admin call
  - [ ] Subtask 7.3: Test `test_enqueue_consume_ack` lifecycle for sync client
  - [ ] Subtask 7.4: Test `test_enqueue_consume_nack_redeliver` for sync client — verify AttemptCount incremented on same stream
  - [ ] Subtask 7.5: Test `test_enqueue_nonexistent_queue` for sync client — verify `QueueNotFoundError`
  - [ ] Subtask 7.6: Test `test_async_enqueue_consume_ack` lifecycle for async client
  - [ ] Subtask 7.7: Integration tests skip gracefully when fila-server binary not found

- [ ] Task 8: CI pipeline (AC: #13)
  - [ ] Subtask 8.1: `.github/workflows/ci.yml` — trigger on PR and push to main
  - [ ] Subtask 8.2: Steps: checkout, setup Python, install deps, ruff lint, mypy type check, pytest
  - [ ] Subtask 8.3: Integration tests skip gracefully when fila-server binary not found

- [ ] Task 9: README and documentation (AC: #15)
  - [ ] Subtask 9.1: README with: installation (`pip install`), sync quickstart, async quickstart, error handling, API reference overview
  - [ ] Subtask 9.2: Docstrings on all public types and methods

## Dev Notes

### Repository Structure

This SDK lives in a **separate repository** (`fila-python`), not inside the fila Cargo workspace. Create it at a sibling path.

```
fila-python/
├── pyproject.toml
├── README.md
├── LICENSE                          # AGPLv3
├── .github/
│   └── workflows/
│       └── ci.yml
├── proto/
│   └── fila/
│       └── v1/
│           ├── messages.proto
│           ├── service.proto
│           └── admin.proto          # For test helpers only
├── fila/
│   ├── __init__.py                  # Public API exports
│   ├── client.py                    # Sync Client class
│   ├── async_client.py              # AsyncClient class
│   ├── errors.py                    # Exception hierarchy
│   ├── types.py                     # ConsumeMessage dataclass
│   └── _proto/                      # Generated protobuf code (committed)
│       └── fila/
│           └── v1/
│               ├── __init__.py
│               ├── messages_pb2.py
│               ├── messages_pb2.pyi
│               ├── service_pb2.py
│               ├── service_pb2.pyi
│               ├── service_pb2_grpc.py
│               ├── admin_pb2.py
│               ├── admin_pb2.pyi
│               └── admin_pb2_grpc.py
└── tests/
    ├── conftest.py                  # Pytest fixtures (server management)
    └── test_client.py               # Integration tests
```

### Proto Generation

Copy proto files needed for the SDK + admin for tests:
- `messages.proto` — Message envelope, metadata types
- `service.proto` — FilaService RPCs (Enqueue, Consume, Ack, Nack)
- `admin.proto` — FilaAdmin (for test queue creation only)

Generation command:
```bash
python -m grpc_tools.protoc \
    -Iproto \
    --python_out=fila/_proto \
    --pyi_out=fila/_proto \
    --grpc_python_out=fila/_proto \
    proto/fila/v1/messages.proto \
    proto/fila/v1/service.proto \
    proto/fila/v1/admin.proto
```

Commit the generated `fila/_proto/` files — consumers should not need protoc or grpcio-tools installed.

### API Design — Mirror Rust/Go SDK Pattern

| Rust SDK | Go SDK | Python SDK (sync) | Python SDK (async) |
|----------|--------|-------------------|-------------------|
| `FilaClient::connect(addr)` | `fila.Dial(addr)` | `Client(addr)` | `AsyncClient(addr)` |
| `client.enqueue(q, h, p)` | `client.Enqueue(ctx, q, h, p)` | `client.enqueue(q, h, p)` | `await client.enqueue(q, h, p)` |
| `client.consume(q)` → Stream | `client.Consume(ctx, q)` → chan | `client.consume(q)` → Iterator | `client.consume(q)` → AsyncIterator |
| `client.ack(q, id)` | `client.Ack(ctx, q, id)` | `client.ack(q, id)` | `await client.ack(q, id)` |
| `client.nack(q, id, e)` | `client.Nack(ctx, q, id, e)` | `client.nack(q, id, e)` | `await client.nack(q, id, e)` |

### Sync vs Async Design

Two separate client classes, not a shared base with sync wrappers:

```python
# Sync client
class Client:
    def __init__(self, addr: str) -> None: ...
    def enqueue(self, queue: str, headers: dict[str, str] | None, payload: bytes) -> str: ...
    def consume(self, queue: str) -> Iterator[ConsumeMessage]: ...
    def ack(self, queue: str, msg_id: str) -> None: ...
    def nack(self, queue: str, msg_id: str, error: str) -> None: ...
    def close(self) -> None: ...

# Async client
class AsyncClient:
    def __init__(self, addr: str) -> None: ...
    async def enqueue(self, queue: str, headers: dict[str, str] | None, payload: bytes) -> str: ...
    async def consume(self, queue: str) -> AsyncIterator[ConsumeMessage]: ...
    async def ack(self, queue: str, msg_id: str) -> None: ...
    async def nack(self, queue: str, msg_id: str, error: str) -> None: ...
    async def close(self) -> None: ...
```

### Exception Pattern

```python
class FilaError(Exception):
    """Base exception for all Fila SDK errors."""

class QueueNotFoundError(FilaError):
    """Raised when the specified queue does not exist."""

class MessageNotFoundError(FilaError):
    """Raised when the specified message does not exist."""

class RPCError(FilaError):
    """Raised for unexpected gRPC failures."""
    def __init__(self, code: grpc.StatusCode, message: str) -> None: ...
```

### Streaming Consume Design

**Sync:** `consume()` returns a generator that yields `ConsumeMessage`. Uses `stream.next()` under the hood. Skip nil message frames (keepalive).

**Async:** `consume()` returns an async generator. Uses `async for response in stream:`. Skip nil frames.

Both close cleanly when the stream ends or when cancelled.

### Integration Test Setup

Tests need a running `fila-server`. Use pytest fixtures following the same pattern as fila-go:

1. Build `fila-server` binary or have it on PATH
2. Start on a random available port with a temp data dir and `fila.toml` config
3. Wait for server to be ready (poll gRPC endpoint)
4. Run tests using both sync and async clients
5. Kill server and cleanup on test completion

Use the generated admin gRPC stubs directly for queue creation in test fixtures.

### Python Dependencies

```
grpcio          # gRPC runtime
grpcio-tools    # Proto generation (dev dependency)
protobuf        # Protobuf runtime
```

Dev dependencies:
```
pytest
pytest-asyncio
ruff
mypy
mypy-protobuf   # Type stubs for generated protobuf code
```

### pyproject.toml Setup

Use modern `pyproject.toml` with `[build-system]` using `setuptools`:
- Package name: `fila-python` (pip install name)
- Import name: `fila` (Python package name)
- Python requires: `>=3.10`
- Include generated proto files in the package

### CI Notes

The CI pipeline **must** be a first-class deliverable (CLAUDE.md requirement).

Integration tests require a `fila-server` binary. Use skip-if-not-found approach:
- `@pytest.mark.skipif(not FILA_SERVER_AVAILABLE, reason="fila-server not found")`
- Check `FILA_SERVER_BIN` env var, then fall back to `../fila/target/release/fila-server`

### What NOT To Do

- Do NOT add admin operations to the public SDK API — hot-path only
- Do NOT implement retry/reconnection logic — keep SDK thin
- Do NOT use `buf` or proto submodules — copy and commit
- Do NOT expose raw protobuf/gRPC types in the public API — wrap in SDK types
- Do NOT use `asyncio.run()` inside sync client — use separate gRPC channels
- Do NOT bundle both sync and async in one class — keep them separate

### References

- [Source: proto/fila/v1/service.proto — RPC definitions (Enqueue, Consume, Ack, Nack)]
- [Source: proto/fila/v1/messages.proto — Message, MessageMetadata types]
- [Source: crates/fila-sdk/src/client.rs — Rust SDK pattern to mirror]
- [Source: /Users/lucas/code/faisca/fila-go/client.go — Go SDK pattern to mirror]
- [Source: /Users/lucas/code/faisca/fila-go/client_test.go — Integration test pattern]
- [Source: _bmad-output/planning-artifacts/epics.md#Story 9.2: Python Client SDK]
- [Source: _bmad-output/implementation-artifacts/epic-8-retro-2026-02-19.md — Proto distribution decisions]

## Dev Agent Record

### Agent Model Used

Claude Opus 4.6

### Debug Log References

### Completion Notes List

### File List
