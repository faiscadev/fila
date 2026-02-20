# Story 9.3: JavaScript/Node.js Client SDK

Status: ready-for-dev

## Story

As a JavaScript/Node.js developer,
I want an idiomatic JS/TS client SDK for Fila,
so that I can integrate Fila into my Node.js services with minimal effort.

## Acceptance Criteria

1. **Given** the Fila proto definitions are available, **when** the JS SDK is built, **then** the SDK lives in a separate repository (`fila-js`)
2. **Given** the repo is created, **when** proto files are set up, **then** proto files are copied into the repo and generated TypeScript code is committed (no submodules, no Buf)
3. **Given** proto files are in the repo, **when** gRPC stubs are generated, **then** `@grpc/proto-loader` with `grpc-js` or `protoc` with `ts-proto` produces TypeScript types and client stubs
4. **Given** the TS stubs exist, **when** the ergonomic wrapper is built, **then** `client.enqueue(queue, headers, payload): Promise<string>` is available
5. **Given** the wrapper is built, **when** streaming consume is implemented, **then** `client.consume(queue)` returns an `AsyncIterable<ConsumeMessage>` usable with `for await (const msg of client.consume(queue))`
6. **Given** the ConsumeMessage type, **when** inspected, **then** it includes: `id`, `headers`, `payload`, `fairnessKey`, `attemptCount`, `queue`
7. **Given** the wrapper is built, **when** ack/nack are implemented, **then** `client.ack(queue, msgId): Promise<void>` and `client.nack(queue, msgId, error): Promise<void>` are available
8. **Given** the error types, **when** operations fail, **then** per-operation error classes are defined (`QueueNotFoundError`, `MessageNotFoundError`) extending a base `FilaError`
9. **Given** the SDK is built, **when** it follows JS/TS conventions, **then** Promise-based API, camelCase methods, and TypeScript type definitions are present
10. **Given** a running `fila-server` binary, **when** integration tests execute, **then** all four operations (enqueue, consume, ack, nack) are verified end-to-end
11. **Given** the repo is created, **when** CI is configured, **then** GitHub Actions runs lint, type check, and test on every PR
12. **Given** the repo is created, **when** packaging is set up, **then** the package is publishable as `@fila/client` via npm
13. **Given** the repo is created, **when** documentation is added, **then** a README with usage examples is included

## Tasks / Subtasks

- [ ] Task 1: Create `fila-js` repository structure (AC: #1, #2, #3, #12)
  - [ ] Subtask 1.1: Initialize repository with `package.json` (name: `@fila/client`), `tsconfig.json`
  - [ ] Subtask 1.2: Copy proto files from `fila` repo: `messages.proto`, `service.proto`, `admin.proto` (admin for test helpers)
  - [ ] Subtask 1.3: Set up proto generation — use `grpc_tools_node_protoc` with `ts-proto` plugin to generate TypeScript types and client stubs, commit generated output
  - [ ] Subtask 1.4: Verify generated stubs compile and produce correct types

- [ ] Task 2: Implement `Client` class (AC: #4, #7, #9)
  - [ ] Subtask 2.1: Create `src/client.ts` with `Client` class wrapping `grpc-js` channel
  - [ ] Subtask 2.2: `constructor(addr: string)` — creates insecure channel
  - [ ] Subtask 2.3: `close(): void` — close the underlying gRPC channel
  - [ ] Subtask 2.4: `enqueue(queue, headers, payload): Promise<string>` — returns message ID
  - [ ] Subtask 2.5: `ack(queue, msgId): Promise<void>` and `nack(queue, msgId, error): Promise<void>`

- [ ] Task 3: Implement streaming `consume` method (AC: #5, #6)
  - [ ] Subtask 3.1: `consume(queue): AsyncIterable<ConsumeMessage>` — async generator yielding messages
  - [ ] Subtask 3.2: `ConsumeMessage` interface with `id`, `headers`, `payload`, `fairnessKey`, `attemptCount`, `queue`
  - [ ] Subtask 3.3: Skip nil message frames (keepalive signals)
  - [ ] Subtask 3.4: Clean stream closure on abort/error

- [ ] Task 4: Error hierarchy (AC: #8)
  - [ ] Subtask 4.1: Define `FilaError` base class extending `Error`
  - [ ] Subtask 4.2: `QueueNotFoundError` and `MessageNotFoundError` extending `FilaError`
  - [ ] Subtask 4.3: `RPCError` for unexpected gRPC failures, preserving status code and message
  - [ ] Subtask 4.4: Per-operation error mapping from gRPC status codes

- [ ] Task 5: Integration tests (AC: #10)
  - [ ] Subtask 5.1: Test helper: start `fila-server` binary on random port with temp data dir, wait for ready
  - [ ] Subtask 5.2: Test helper: create queue via direct gRPC admin call
  - [ ] Subtask 5.3: Test enqueue-consume-ack lifecycle
  - [ ] Subtask 5.4: Test enqueue-consume-nack-redeliver on same stream
  - [ ] Subtask 5.5: Test enqueue to nonexistent queue raises `QueueNotFoundError`
  - [ ] Subtask 5.6: Tests skip gracefully when fila-server binary not found

- [ ] Task 6: CI pipeline (AC: #11)
  - [ ] Subtask 6.1: `.github/workflows/ci.yml` — trigger on PR and push to main
  - [ ] Subtask 6.2: Steps: checkout, setup Node, install deps, lint, type check, build, test

- [ ] Task 7: README and documentation (AC: #13)
  - [ ] Subtask 7.1: README with: installation (`npm install`), quickstart example, error handling, API reference overview
  - [ ] Subtask 7.2: JSDoc/TSDoc comments on all exported types and methods

## Dev Notes

### Repository Structure

```
fila-js/
├── package.json
├── tsconfig.json
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
├── src/
│   ├── index.ts                    # Public API exports
│   ├── client.ts                   # Client class
│   ├── errors.ts                   # Error hierarchy
│   └── types.ts                    # ConsumeMessage interface
├── generated/                      # Generated proto code (committed)
│   └── fila/
│       └── v1/
│           ├── messages.ts
│           ├── service.ts
│           └── admin.ts            # For test helpers
├── test/
│   ├── helpers.ts                  # Server management
│   └── client.test.ts              # Integration tests
└── dist/                           # Build output (gitignored)
```

### Proto Generation Approach

Use `ts-proto` plugin with `protoc` for idiomatic TypeScript output:
```bash
protoc --plugin=./node_modules/.bin/protoc-gen-ts_proto \
  --ts_proto_out=generated \
  --ts_proto_opt=outputServices=grpc-js \
  --ts_proto_opt=esModuleInterop=true \
  -Iproto \
  proto/fila/v1/messages.proto \
  proto/fila/v1/service.proto \
  proto/fila/v1/admin.proto
```

Commit the generated `generated/` files — consumers should not need protoc installed.

### API Design

```typescript
class Client {
  constructor(addr: string);
  close(): void;
  enqueue(queue: string, headers: Record<string, string> | null, payload: Buffer): Promise<string>;
  consume(queue: string): AsyncIterable<ConsumeMessage>;
  ack(queue: string, msgId: string): Promise<void>;
  nack(queue: string, msgId: string, error: string): Promise<void>;
}

interface ConsumeMessage {
  id: string;
  headers: Record<string, string>;
  payload: Buffer;
  fairnessKey: string;
  attemptCount: number;
  queue: string;
}
```

### Streaming Consume Design

`consume()` returns an `AsyncIterable` implemented as an async generator. Internally reads from the gRPC server stream and yields `ConsumeMessage` values. Skip nil frames. The async iterable ends when the stream closes or errors.

```typescript
async *consume(queue: string): AsyncIterable<ConsumeMessage> {
  const stream = this.client.Consume({ queue });
  for await (const resp of stream) {
    if (!resp.message) continue; // keepalive
    yield convertMessage(resp.message);
  }
}
```

### Error Pattern

```typescript
class FilaError extends Error { }
class QueueNotFoundError extends FilaError { }
class MessageNotFoundError extends FilaError { }
class RPCError extends FilaError {
  constructor(public code: number, message: string) { super(message); }
}
```

### Integration Test Setup

Same subprocess pattern as Go/Python SDKs:
1. Check `FILA_SERVER_BIN` env var, fall back to `../../fila/target/release/fila-server`
2. Start on random port with temp data dir and `fila.toml` config
3. Wait for ready (poll gRPC endpoint)
4. Use admin gRPC client for queue creation in tests
5. Kill and cleanup on test completion

### Dependencies

```
@grpc/grpc-js       # gRPC runtime
@grpc/proto-loader   # Proto loading (if using dynamic approach)
```

Dev:
```
typescript
ts-proto             # Proto generation
vitest               # Test runner
eslint               # Linter
```

### What NOT To Do

- Do NOT add admin operations to the public SDK API — hot-path only
- Do NOT implement retry/reconnection logic — keep SDK thin
- Do NOT use `buf` or proto submodules — copy and commit
- Do NOT expose raw gRPC types in the public API

### References

- [Source: proto/fila/v1/service.proto — RPC definitions]
- [Source: proto/fila/v1/messages.proto — Message types]
- [Source: /Users/lucas/code/faisca/fila-go/ — Go SDK pattern]
- [Source: /Users/lucas/code/faisca/fila-python/ — Python SDK pattern]
- [Source: _bmad-output/planning-artifacts/epics.md#Story 9.3]

## Dev Agent Record

### Agent Model Used

Claude Opus 4.6

### Debug Log References

### Completion Notes List

### File List
