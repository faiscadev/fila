# Story 9.3: JavaScript/Node.js Client SDK

Status: review

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

- [x] Task 1: Create `fila-js` repository structure (AC: #1, #2, #3, #12)
  - [x] Subtask 1.1: Initialize repository with `package.json` (name: `@fila/client`), `tsconfig.json`
  - [x] Subtask 1.2: Copy proto files from `fila` repo: `messages.proto`, `service.proto`, `admin.proto` (admin for test helpers)
  - [x] Subtask 1.3: Set up proto generation — use `proto-loader-gen-types` with `@grpc/grpc-js` to generate TypeScript types, commit generated output
  - [x] Subtask 1.4: Verify generated stubs compile and produce correct types

- [x] Task 2: Implement `Client` class (AC: #4, #7, #9)
  - [x] Subtask 2.1: Create `src/client.ts` with `Client` class wrapping `grpc-js` channel
  - [x] Subtask 2.2: `constructor(addr: string)` — creates insecure channel
  - [x] Subtask 2.3: `close(): void` — close the underlying gRPC channel
  - [x] Subtask 2.4: `enqueue(queue, headers, payload): Promise<string>` — returns message ID
  - [x] Subtask 2.5: `ack(queue, msgId): Promise<void>` and `nack(queue, msgId, error): Promise<void>`

- [x] Task 3: Implement streaming `consume` method (AC: #5, #6)
  - [x] Subtask 3.1: `consume(queue): AsyncIterable<ConsumeMessage>` — async generator yielding messages
  - [x] Subtask 3.2: `ConsumeMessage` interface with `id`, `headers`, `payload`, `fairnessKey`, `attemptCount`, `queue`
  - [x] Subtask 3.3: Skip nil message frames (keepalive signals)
  - [x] Subtask 3.4: Clean stream closure on abort/error — `finally { stream.cancel() }`

- [x] Task 4: Error hierarchy (AC: #8)
  - [x] Subtask 4.1: Define `FilaError` base class extending `Error`
  - [x] Subtask 4.2: `QueueNotFoundError` and `MessageNotFoundError` extending `FilaError`
  - [x] Subtask 4.3: `RPCError` for unexpected gRPC failures, preserving status code and message
  - [x] Subtask 4.4: Per-operation error mapping from gRPC status codes

- [x] Task 5: Integration tests (AC: #10)
  - [x] Subtask 5.1: Test helper: start `fila-server` binary on random port with temp data dir, wait for ready
  - [x] Subtask 5.2: Test helper: create queue via direct gRPC admin call
  - [x] Subtask 5.3: Test enqueue-consume-ack lifecycle
  - [x] Subtask 5.4: Test enqueue-consume-nack-redeliver on same stream
  - [x] Subtask 5.5: Test enqueue to nonexistent queue raises `QueueNotFoundError`
  - [x] Subtask 5.6: Tests skip gracefully when fila-server binary not found

- [x] Task 6: CI pipeline (AC: #11)
  - [x] Subtask 6.1: `.github/workflows/ci.yml` — trigger on PR and push to main
  - [x] Subtask 6.2: Steps: checkout, setup Node, install deps, lint, type check, build, test

- [x] Task 7: README and documentation (AC: #13)
  - [x] Subtask 7.1: README with: installation (`npm install`), quickstart example, error handling, API reference overview
  - [x] Subtask 7.2: JSDoc/TSDoc comments on all exported types and methods

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
│           ├── FilaService.ts      # Service client types
│           ├── FilaAdmin.ts        # Admin service types (test only)
│           ├── Message.ts          # Message types
│           └── ...                 # Request/response types
├── test/
│   ├── helpers.ts                  # Server management
│   └── client.test.ts              # Integration tests
└── dist/                           # Build output (gitignored)
```

### Implementation Decisions

- **Proto generation**: Used `proto-loader-gen-types` (from `@grpc/proto-loader`) instead of `ts-proto`. Generates TypeScript type definitions that work with `@grpc/proto-loader`'s dynamic loading at runtime.
- **Proto path resolution**: `resolveProtoDir()` tries `../proto` (dev/test) then `../../proto` (built dist) relative to `__dirname`.
- **Stream cancellation**: `consume()` async generator includes `finally { stream.cancel() }` to ensure server-side stream is cleaned up when consumer breaks out of the loop.
- **Same-stream redelivery**: Nacked messages are redelivered on the same consume stream (consistent with Go/Python SDKs and Rust e2e tests).

## Dev Agent Record

### Agent Model Used

Claude Opus 4.6

### Debug Log References

### Completion Notes List

- Used `@grpc/proto-loader` + `proto-loader-gen-types` instead of `ts-proto` — better integration with `@grpc/grpc-js` dynamic loading
- Code review found silent timeout in test helper; fixed to throw explicit error
- Nack test initially opened a new stream; fixed to use same-stream pattern matching Go/Python SDKs

### Change Log

- 2026-02-19: Initial implementation — Client class, error hierarchy, 3 integration tests, CI, README
- 2026-02-19: Code review fix — test helper throws on server startup timeout

### File List

- `fila-js/package.json` — Package config, scripts, dependencies
- `fila-js/tsconfig.json` — TypeScript config (rootDir: ".", strict)
- `fila-js/vitest.config.ts` — Test config (30s timeout)
- `fila-js/eslint.config.mjs` — ESLint flat config with TypeScript plugin
- `fila-js/.gitignore` — node_modules, dist, data
- `fila-js/.github/workflows/ci.yml` — CI pipeline: lint, typecheck, build, test
- `fila-js/LICENSE` — AGPLv3
- `fila-js/README.md` — Usage docs, API reference, error handling
- `fila-js/proto/fila/v1/messages.proto` — Copied proto definitions
- `fila-js/proto/fila/v1/service.proto` — Copied proto definitions
- `fila-js/proto/fila/v1/admin.proto` — Copied proto definitions (test only)
- `fila-js/generated/` — Generated TypeScript types from proto-loader-gen-types
- `fila-js/src/index.ts` — Public API exports
- `fila-js/src/client.ts` — Client class (enqueue, consume, ack, nack)
- `fila-js/src/errors.ts` — FilaError, QueueNotFoundError, MessageNotFoundError, RPCError
- `fila-js/src/types.ts` — ConsumeMessage interface
- `fila-js/test/helpers.ts` — TestServer helper (subprocess management, admin client)
- `fila-js/test/client.test.ts` — 3 integration tests
