# Story 9.5: Java Client SDK

Status: ready-for-dev

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

- [ ] Task 1: Create `fila-java` repository structure (AC: #1, #2, #3, #12)
  - [ ] Subtask 1.1: Initialize Gradle project with Gradle Wrapper (no global Gradle install needed)
  - [ ] Subtask 1.2: Configure `build.gradle` with protobuf plugin, gRPC dependencies, and Java 17 target
  - [ ] Subtask 1.3: Copy proto files from `fila` repo: `proto/fila/v1/messages.proto`, `proto/fila/v1/service.proto`, `proto/fila/v1/admin.proto` (admin for test helpers)
  - [ ] Subtask 1.4: Generate Java stubs via protobuf Gradle plugin, commit generated output
  - [ ] Subtask 1.5: Set up `dev.faisca:fila-client` Maven coordinates in build.gradle

- [ ] Task 2: Implement `FilaClient` class (AC: #4, #7, #9)
  - [ ] Subtask 2.1: `FilaClient.Builder` with `address(String)` builder pattern
  - [ ] Subtask 2.2: `enqueue(String queue, Map<String,String> headers, byte[] payload)` returns String (message ID)
  - [ ] Subtask 2.3: `ack(String queue, String msgId)` and `nack(String queue, String msgId, String error)`
  - [ ] Subtask 2.4: `close()` to shut down the gRPC channel (implements `AutoCloseable`)

- [ ] Task 3: Implement streaming consume (AC: #5, #6)
  - [ ] Subtask 3.1: `consume(String queue, Consumer<ConsumeMessage> handler)` — blocking consumer that calls handler for each message
  - [ ] Subtask 3.2: `ConsumeMessage` record/class with `id`, `headers`, `payload`, `fairnessKey`, `attemptCount`, `queue`
  - [ ] Subtask 3.3: Skip nil message frames (keepalive signals)
  - [ ] Subtask 3.4: Return a `ConsumerHandle` with `cancel()` for stopping the stream

- [ ] Task 4: Exception hierarchy (AC: #8)
  - [ ] Subtask 4.1: `FilaException` (unchecked, extends RuntimeException)
  - [ ] Subtask 4.2: `QueueNotFoundException extends FilaException`
  - [ ] Subtask 4.3: `MessageNotFoundException extends FilaException`
  - [ ] Subtask 4.4: `RpcException extends FilaException` for unexpected gRPC failures (preserves status code + message)

- [ ] Task 5: Integration tests (AC: #10)
  - [ ] Subtask 5.1: Test helper: start `fila-server` binary with TOML config + temp data dir, wait for ready, cleanup
  - [ ] Subtask 5.2: Test helper: create queue via admin gRPC stub (SDK doesn't wrap admin ops)
  - [ ] Subtask 5.3: Test enqueue → consume → ack lifecycle
  - [ ] Subtask 5.4: Test enqueue → consume → nack → redelivery on same stream (same-stream pattern)
  - [ ] Subtask 5.5: Test enqueue to nonexistent queue → verify `QueueNotFoundException`

- [ ] Task 6: CI pipeline (AC: #11)
  - [ ] Subtask 6.1: `.github/workflows/ci.yml` — trigger on PR
  - [ ] Subtask 6.2: Steps: checkout, setup JDK 17, gradle build, gradle test
  - [ ] Subtask 6.3: Spotless or checkstyle for code formatting

- [ ] Task 7: README and documentation (AC: #13)
  - [ ] Subtask 7.1: README with installation (Gradle/Maven coordinates), quickstart example, error handling, API reference
  - [ ] Subtask 7.2: Javadoc on all public classes and methods

## Dev Notes

### Repository Structure

```
fila-java/
├── build.gradle
├── settings.gradle
├── gradlew, gradlew.bat, gradle/wrapper/
├── .github/workflows/ci.yml
├── .gitignore
├── LICENSE (AGPLv3)
├── README.md
├── proto/fila/v1/*.proto           # Source protos (copied)
└── src/
    ├── main/java/dev/faisca/fila/
    │   ├── FilaClient.java         # Client class with builder
    │   ├── ConsumeMessage.java     # Message type
    │   ├── ConsumerHandle.java     # Cancellable consumer handle
    │   ├── FilaException.java      # Base exception
    │   ├── QueueNotFoundException.java
    │   ├── MessageNotFoundException.java
    │   └── RpcException.java
    └── test/java/dev/faisca/fila/
        ├── TestServer.java         # fila-server lifecycle helper
        └── FilaClientTest.java     # Integration tests
```

### Key Patterns from Prior SDKs

- **Proto distribution**: Copy `messages.proto`, `service.proto`, `admin.proto` into `proto/fila/v1/`. Admin proto is for test helpers only (queue creation), NOT exposed in public API.
- **Same-stream nack redelivery**: Nacked messages are redelivered on the same consume stream. Tests must verify redelivery without opening a new stream.
- **Test server helper**: Start `fila-server` binary from `../../fila/target/release/fila-server`, create TOML config with `[server]\nlisten_addr`, set `FILA_DATA_DIR` env var to temp directory, wait for port to become available, clean up on test end.
- **Per-operation exceptions**: Each operation throws only exceptions it can produce. `enqueue` throws `QueueNotFoundException`. `ack`/`nack` throw `MessageNotFoundException`. `consume` can throw `QueueNotFoundException`. All throw `RpcException` for unexpected failures.
- **Keepalive frames**: The consume stream sends frames where `message` may be null or have empty ID — skip these.
- **AutoCloseable**: `FilaClient` implements `AutoCloseable` for try-with-resources support.

### Build Configuration

- Use Gradle with `com.google.protobuf` plugin for proto compilation
- `io.grpc:grpc-protobuf`, `io.grpc:grpc-stub`, `io.grpc:grpc-netty-shim` (or `grpc-netty`) for runtime
- Java 17 target (matches installed JDK)
- Gradle Wrapper must be committed so CI works without global Gradle install
- JUnit 5 for tests

### References

- [Source: _bmad-output/planning-artifacts/epics.md#Story 9.5]
- [Source: proto/fila/v1/service.proto — gRPC service definition]
- [Source: proto/fila/v1/messages.proto — message types]
- [Source: proto/fila/v1/admin.proto — admin service for test setup]

## Dev Agent Record

### Agent Model Used

Claude Opus 4.6

### Completion Notes List

### Change Log

### File List
