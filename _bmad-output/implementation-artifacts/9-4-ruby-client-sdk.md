# Story 9.4: Ruby Client SDK

Status: review

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

- [x] Task 1: Create `fila-ruby` repository structure (AC: #1, #2, #3, #12)
- [x] Task 2: Implement `Fila::Client` class (AC: #4, #7, #9)
- [x] Task 3: Implement streaming `consume` method (AC: #5, #6)
- [x] Task 4: Error hierarchy (AC: #8)
- [x] Task 5: Integration tests (AC: #10)
- [x] Task 6: CI pipeline (AC: #11)
- [x] Task 7: README and documentation (AC: #13)

## Dev Agent Record

### Agent Model Used

Claude Opus 4.6

### Completion Notes List

- Installed Ruby 4.0 via Homebrew (system Ruby 2.6 too old for modern grpc gem)
- Proto load path fix: `$LOAD_PATH.unshift` to add `lib/fila/proto` so generated requires resolve
- rubocop auto-corrected 110 offenses (mostly string quoting style), extracted `build_consume_message` to reduce ABC complexity
- grpc-ruby doesn't expose channel close on stubs — `close` method is a no-op for API symmetry

### Change Log

- 2026-02-19: Initial implementation — Client class, error hierarchy, 3 integration tests, CI, README

### File List

- `fila-ruby/fila-client.gemspec` — Gem specification
- `fila-ruby/Gemfile` — Dependencies
- `fila-ruby/Rakefile` — Test task
- `fila-ruby/.rubocop.yml` — Linter config
- `fila-ruby/.gitignore` — Exclusions
- `fila-ruby/.github/workflows/ci.yml` — CI pipeline
- `fila-ruby/LICENSE` — AGPLv3
- `fila-ruby/README.md` — Usage docs, API reference
- `fila-ruby/lib/fila.rb` — Main entrypoint
- `fila-ruby/lib/fila/client.rb` — Client class
- `fila-ruby/lib/fila/errors.rb` — Error hierarchy
- `fila-ruby/lib/fila/consume_message.rb` — ConsumeMessage struct
- `fila-ruby/lib/fila/version.rb` — Gem version
- `fila-ruby/lib/fila/proto/` — Generated gRPC stubs
- `fila-ruby/proto/` — Source proto files
- `fila-ruby/test/test_helper.rb` — TestServer helper
- `fila-ruby/test/test_client.rb` — 3 integration tests
