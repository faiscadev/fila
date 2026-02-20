# Story 9.4: Ruby Client SDK

Status: ready-for-dev

## Story

As a Ruby developer,
I want an idiomatic Ruby client SDK for Fila,
so that I can integrate Fila into my Ruby applications with minimal effort.

## Acceptance Criteria

1. **Given** the Fila proto definitions are available, **when** the Ruby SDK is built, **then** the SDK lives in a separate repository (`fila-ruby`)
2. **Given** the repo is created, **when** proto files are set up, **then** proto files are copied into the repo and generated Ruby code is committed (no submodules, no Buf)
3. **Given** proto files are in the repo, **when** gRPC stubs are generated, **then** `grpc-tools` gem produces Ruby client stubs
4. **Given** the stubs exist, **when** the ergonomic wrapper is built, **then** `client.enqueue(queue:, headers:, payload:)` returns the message ID (string)
5. **Given** the wrapper is built, **when** streaming consume is implemented, **then** `client.consume(queue:)` yields `ConsumeMessage` objects via block or returns an `Enumerator`
6. **Given** the ConsumeMessage type, **when** inspected, **then** it includes: `id`, `headers`, `payload`, `fairness_key`, `attempt_count`, `queue`
7. **Given** the wrapper is built, **when** ack/nack are implemented, **then** `client.ack(queue:, msg_id:)` and `client.nack(queue:, msg_id:, error:)` are available
8. **Given** the error types, **when** operations fail, **then** per-operation error classes are defined (`Fila::QueueNotFoundError`, `Fila::MessageNotFoundError`) extending a base `Fila::Error`
9. **Given** the SDK is built, **when** it follows Ruby conventions, **then** keyword arguments, snake_case methods, and block patterns are used
10. **Given** a running `fila-server` binary, **when** integration tests execute, **then** all four operations (enqueue, consume, ack, nack) are verified end-to-end
11. **Given** the repo is created, **when** CI is configured, **then** GitHub Actions runs lint (`rubocop`) and test on every PR
12. **Given** the repo is created, **when** packaging is set up, **then** the gem is publishable as `fila-client`
13. **Given** the repo is created, **when** documentation is added, **then** a README with usage examples is included

## Tasks / Subtasks

- [ ] Task 1: Create `fila-ruby` repository structure (AC: #1, #2, #3, #12)
  - [ ] Subtask 1.1: Initialize repository with `fila-client.gemspec`, `Gemfile`
  - [ ] Subtask 1.2: Copy proto files from `fila` repo: `messages.proto`, `service.proto`, `admin.proto` (admin for test helpers)
  - [ ] Subtask 1.3: Generate Ruby stubs with `grpc_tools_ruby_protoc`, commit generated output under `lib/fila/proto/`
  - [ ] Subtask 1.4: Verify generated stubs load correctly

- [ ] Task 2: Implement `Fila::Client` class (AC: #4, #7, #9)
  - [ ] Subtask 2.1: Create `lib/fila/client.rb` with `Fila::Client` class wrapping gRPC stub
  - [ ] Subtask 2.2: `initialize(addr)` — creates insecure channel
  - [ ] Subtask 2.3: `close` — close the underlying gRPC channel
  - [ ] Subtask 2.4: `enqueue(queue:, headers:, payload:)` — returns message ID string
  - [ ] Subtask 2.5: `ack(queue:, msg_id:)` and `nack(queue:, msg_id:, error:)`

- [ ] Task 3: Implement streaming `consume` method (AC: #5, #6)
  - [ ] Subtask 3.1: `consume(queue:)` — yields `ConsumeMessage` via block, or returns `Enumerator` if no block given
  - [ ] Subtask 3.2: `ConsumeMessage` struct/class with `id`, `headers`, `payload`, `fairness_key`, `attempt_count`, `queue`
  - [ ] Subtask 3.3: Skip nil message frames (keepalive signals)
  - [ ] Subtask 3.4: Proper stream cleanup on break/error

- [ ] Task 4: Error hierarchy (AC: #8)
  - [ ] Subtask 4.1: Define `Fila::Error` base class extending `StandardError`
  - [ ] Subtask 4.2: `Fila::QueueNotFoundError` and `Fila::MessageNotFoundError` extending `Fila::Error`
  - [ ] Subtask 4.3: `Fila::RPCError` for unexpected gRPC failures, preserving status code and message
  - [ ] Subtask 4.4: Per-operation error mapping from gRPC status codes

- [ ] Task 5: Integration tests (AC: #10)
  - [ ] Subtask 5.1: Test helper: start `fila-server` binary on random port with temp data dir, wait for ready
  - [ ] Subtask 5.2: Test helper: create queue via direct gRPC admin call
  - [ ] Subtask 5.3: Test enqueue-consume-ack lifecycle
  - [ ] Subtask 5.4: Test enqueue-consume-nack-redeliver on same stream
  - [ ] Subtask 5.5: Test enqueue to nonexistent queue raises `Fila::QueueNotFoundError`
  - [ ] Subtask 5.6: Tests skip gracefully when fila-server binary not found

- [ ] Task 6: CI pipeline (AC: #11)
  - [ ] Subtask 6.1: `.github/workflows/ci.yml` — trigger on PR and push to main
  - [ ] Subtask 6.2: Steps: checkout, setup Ruby, install deps, rubocop, test

- [ ] Task 7: README and documentation (AC: #13)
  - [ ] Subtask 7.1: README with: installation (`gem install`), quickstart example, error handling, API reference overview
  - [ ] Subtask 7.2: YARD comments on all public classes and methods

## Dev Notes

### Repository Structure

```
fila-ruby/
├── fila-client.gemspec
├── Gemfile
├── Rakefile
├── README.md
├── LICENSE                          # AGPLv3
├── .rubocop.yml
├── .github/
│   └── workflows/
│       └── ci.yml
├── proto/
│   └── fila/
│       └── v1/
│           ├── messages.proto
│           ├── service.proto
│           └── admin.proto          # For test helpers only
├── lib/
│   ├── fila.rb                     # Main entrypoint, requires
│   ├── fila/
│   │   ├── client.rb               # Client class
│   │   ├── errors.rb               # Error hierarchy
│   │   ├── consume_message.rb      # ConsumeMessage struct
│   │   ├── version.rb              # Gem version
│   │   └── proto/                  # Generated proto stubs (committed)
│   │       └── fila/
│   │           └── v1/
│   │               ├── messages_pb.rb
│   │               ├── service_pb.rb
│   │               ├── service_services_pb.rb
│   │               ├── admin_pb.rb
│   │               └── admin_services_pb.rb
├── test/
│   ├── test_helper.rb              # Server management, minitest setup
│   └── test_client.rb              # Integration tests
```

### Proto Generation Approach

Use `grpc_tools_ruby_protoc` to generate Ruby stubs:
```bash
grpc_tools_ruby_protoc \
  --ruby_out=lib/fila/proto \
  --grpc_out=lib/fila/proto \
  -Iproto \
  proto/fila/v1/messages.proto \
  proto/fila/v1/service.proto \
  proto/fila/v1/admin.proto
```

Commit the generated files — consumers should not need protoc installed.

### API Design

```ruby
client = Fila::Client.new("localhost:5555")

# Enqueue
msg_id = client.enqueue(queue: "my-queue", headers: { "tenant" => "acme" }, payload: "hello")

# Consume with block (idiomatic Ruby)
client.consume(queue: "my-queue") do |msg|
  puts "Received: #{msg.id} (attempt #{msg.attempt_count})"
  client.ack(queue: "my-queue", msg_id: msg.id)
  break  # stop consuming after first message
end

# Consume with Enumerator
msgs = client.consume(queue: "my-queue")
msg = msgs.next
client.ack(queue: "my-queue", msg_id: msg.id)

client.close
```

### Streaming Consume Design

`consume` opens a gRPC server stream. If a block is given, yields messages to it. If no block, returns an `Enumerator`. Nil frames (keepalive) are skipped. Stream is cancelled on break or error.

### Error Pattern

```ruby
module Fila
  class Error < StandardError; end
  class QueueNotFoundError < Error; end
  class MessageNotFoundError < Error; end
  class RPCError < Error
    attr_reader :code
    def initialize(code, message)
      @code = code
      super("rpc error (code = #{code}): #{message}")
    end
  end
end
```

### Integration Test Setup

Same subprocess pattern as Go/Python/JS SDKs:
1. Check `FILA_SERVER_BIN` env var, fall back to `../../fila/target/release/fila-server`
2. Start on random port with temp data dir and `fila.toml` config
3. Wait for ready (poll gRPC endpoint)
4. Use admin gRPC client for queue creation in tests
5. Kill and cleanup on test completion

### Dependencies

Runtime:
```
grpc          # gRPC runtime
google-protobuf  # Protobuf runtime
```

Dev:
```
minitest      # Test framework
rubocop       # Linter
rake          # Build tool
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
- [Source: /Users/lucas/code/faisca/fila-js/ — JS SDK pattern]
- [Source: _bmad-output/planning-artifacts/epics.md#Story 9.4]

## Dev Agent Record

### Agent Model Used

Claude Opus 4.6

### Debug Log References

### Completion Notes List

### File List
