# Story 9.5: Java Client SDK

Status: done

## Story

As a Java developer,
I want an idiomatic Java client SDK for Fila,
so that I can integrate Fila into my Java applications with minimal effort.

## Acceptance Criteria

1. **Given** the Fila proto definitions are available, **when** the Java SDK is built, **then** the SDK lives in a separate repository (`fila-java`)
2. **Given** the repo is created, **when** proto files are set up, **then** proto files are copied into the repo and generated Java code is committed (no submodules, no Buf)
3. **Given** proto files are in the repo, **when** gRPC stubs are generated, **then** `protoc-gen-grpc-java` and `protoc-gen-java` produce Java stubs and message classes
4. **Given** the stubs exist, **when** the ergonomic wrapper is built, **then** `client.enqueue(queue, headers, payload)` returns the message ID (String)
5. **Given** the wrapper is built, **when** streaming consume is implemented, **then** `client.consume(queue, observer)` accepts a callback/observer pattern for message delivery
6. **Given** the ConsumeMessage type, **when** inspected, **then** it includes: `getId()`, `getHeaders()`, `getPayload()`, `getFairnessKey()`, `getAttemptCount()`, `getQueue()`
7. **Given** the wrapper is built, **when** ack/nack are implemented, **then** `client.ack(queue, msgId)` and `client.nack(queue, msgId, error)` are available
8. **Given** the error types, **when** operations fail, **then** per-operation exception classes are defined (`QueueNotFoundException`, `MessageNotFoundException`) extending a base `FilaException`
9. **Given** the SDK is built, **when** it follows Java conventions, **then** builder pattern for client creation, unchecked exceptions, and Javadoc are used
10. **Given** a running `fila-server` binary, **when** integration tests execute, **then** all four operations (enqueue, consume, ack, nack) are verified end-to-end
11. **Given** the repo is created, **when** CI is configured, **then** GitHub Actions runs build, test, and lint (checkstyle or spotless) on every PR
12. **Given** the repo is created, **when** packaging is set up, **then** the artifact is publishable to Maven Central as `dev.faisca:fila-client`
13. **Given** the repo is created, **when** documentation is added, **then** a README with usage examples is included

## Tasks / Subtasks

- [x] Task 1: Create `fila-java` repository structure (AC: #1, #2, #3, #12)
  - [x] Subtask 1.1: Initialize Gradle project with Gradle Wrapper
  - [x] Subtask 1.2: Configure `build.gradle` with protobuf plugin, gRPC dependencies, and Java 17 target
  - [x] Subtask 1.3: Copy proto files from `fila` repo
  - [x] Subtask 1.4: Generate Java stubs via protobuf Gradle plugin
  - [x] Subtask 1.5: Set up `dev.faisca:fila-client` Maven coordinates in build.gradle

- [x] Task 2: Implement `FilaClient` class (AC: #4, #7, #9)
  - [x] Subtask 2.1: `FilaClient.Builder` with `address(String)` builder pattern
  - [x] Subtask 2.2: `enqueue(String queue, Map<String,String> headers, byte[] payload)` returns String
  - [x] Subtask 2.3: `ack(String queue, String msgId)` and `nack(String queue, String msgId, String error)`
  - [x] Subtask 2.4: `close()` to shut down the gRPC channel (implements `AutoCloseable`)

- [x] Task 3: Implement streaming consume (AC: #5, #6)
  - [x] Subtask 3.1: `consume(String queue, Consumer<ConsumeMessage> handler)` with background thread
  - [x] Subtask 3.2: `ConsumeMessage` class with id, headers, payload, fairnessKey, attemptCount, queue
  - [x] Subtask 3.3: Skip nil message frames (keepalive signals)
  - [x] Subtask 3.4: Return a `ConsumerHandle` with `cancel()` for stopping the stream

- [x] Task 4: Exception hierarchy (AC: #8)
  - [x] Subtask 4.1: `FilaException` (unchecked, extends RuntimeException)
  - [x] Subtask 4.2: `QueueNotFoundException extends FilaException`
  - [x] Subtask 4.3: `MessageNotFoundException extends FilaException`
  - [x] Subtask 4.4: `RpcException extends FilaException` (preserves status code + message)

- [x] Task 5: Integration tests (AC: #10)
  - [x] Subtask 5.1: Test helper: start `fila-server` binary with TOML config + temp data dir
  - [x] Subtask 5.2: Test helper: create queue via admin gRPC stub
  - [x] Subtask 5.3: Test enqueue → consume → ack lifecycle
  - [x] Subtask 5.4: Test enqueue → consume → nack → redelivery on same stream
  - [x] Subtask 5.5: Test enqueue to nonexistent queue → verify `QueueNotFoundException`

- [x] Task 6: CI pipeline (AC: #11)
  - [x] Subtask 6.1: `.github/workflows/ci.yml` — trigger on PR
  - [x] Subtask 6.2: Steps: checkout, setup JDK 17, gradle build, spotless check
  - [x] Subtask 6.3: Spotless with google-java-format for code formatting

- [x] Task 7: README and documentation (AC: #13)
  - [x] Subtask 7.1: README with Gradle/Maven coordinates, quickstart, error handling, API reference
  - [x] Subtask 7.2: Javadoc on all public classes and methods

## Dev Agent Record

### Agent Model Used

Claude Opus 4.6

### Completion Notes List

- Gradle 8.12 with protobuf plugin 0.9.4 for proto compilation
- gRPC-Java 1.71.0, protobuf 4.29.3
- Server working directory must contain `fila.toml` (not `--config` flag) — fixed in TestServer
- Attempt count starts at 0 for first delivery, increments on nack
- `grpc-netty-shaded` used instead of plain `grpc-netty` to avoid Netty version conflicts
- Spotless with google-java-format 1.25.2 auto-corrected formatting
- Consumer runs on a daemon background thread with CancellableContext for clean shutdown

### Change Log

- 2026-02-20: Initial implementation — FilaClient, exception hierarchy, 3 integration tests, CI, README

### File List

- `fila-java/build.gradle` — Gradle build config with protobuf plugin
- `fila-java/settings.gradle` — Project settings
- `fila-java/gradlew`, `fila-java/gradlew.bat`, `fila-java/gradle/wrapper/` — Gradle wrapper
- `fila-java/.github/workflows/ci.yml` — CI pipeline
- `fila-java/.gitignore` — Exclusions
- `fila-java/LICENSE` — AGPLv3
- `fila-java/README.md` — Usage docs, API reference
- `fila-java/proto/fila/v1/*.proto` — Source proto files
- `fila-java/src/main/java/dev/faisca/fila/FilaClient.java` — Client class with builder
- `fila-java/src/main/java/dev/faisca/fila/ConsumeMessage.java` — Message type
- `fila-java/src/main/java/dev/faisca/fila/ConsumerHandle.java` — Cancellable consumer handle
- `fila-java/src/main/java/dev/faisca/fila/FilaException.java` — Base exception
- `fila-java/src/main/java/dev/faisca/fila/QueueNotFoundException.java` — Queue not found
- `fila-java/src/main/java/dev/faisca/fila/MessageNotFoundException.java` — Message not found
- `fila-java/src/main/java/dev/faisca/fila/RpcException.java` — Unexpected gRPC failure
- `fila-java/src/test/java/dev/faisca/fila/TestServer.java` — Test server helper
- `fila-java/src/test/java/dev/faisca/fila/FilaClientTest.java` — 3 integration tests
