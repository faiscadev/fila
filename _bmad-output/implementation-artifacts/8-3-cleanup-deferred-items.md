# Story 8.3: Cleanup & Deferred Items

Status: review

## Story

As a developer,
I want accumulated structural debt and deferred items addressed,
so that the codebase is clean for the remaining SDK epics.

## Acceptance Criteria

1. **Given** the scheduler decomposition from Story 8.2 is complete, **when** cleanup is performed, **then** the deferred tonic interceptor for trace context extraction is wired (Story 6.1 Task 7 completion)
2. **Given** incoming gRPC requests with W3C `traceparent` metadata, **when** the request is processed, **then** handler spans link to the caller's trace via the extracted parent context
3. **Given** the decomposition from Story 8.2, **when** dead code and unused imports are checked, **then** none are found (clippy clean)
4. **Given** the cleanup changes, **when** all test suites run, **then** all 278 tests pass, clippy clean, fmt clean

## Dev Agent Record

### Agent Model Used
claude-opus-4-6

### Completion Notes List
- Wired W3C trace context extraction via `TraceContextLayer` tower middleware
- Layer extracts `traceparent`/`tracestate` from HTTP headers using globally registered `TraceContextPropagator`
- Creates a parent `grpc.server` tracing span linked to the incoming trace; all handler `#[instrument]` spans become children
- Added workspace deps: tower 0.5, http 1, pin-project-lite 0.2 (all already transitive deps from tonic)
- No dead code, unused imports, or stale comments found after decomposition — clippy clean
- Fixed pre-existing test issue found by Cubic in Story 8.2 PR (recovery_preserves_queue_definitions now properly reopens storage)

### File List
- crates/fila-server/src/trace_context.rs (new — TraceContextLayer, TraceContextService, Instrumented future)
- crates/fila-server/src/main.rs (modified — added `.layer(trace_context::TraceContextLayer)` to Server builder)
- crates/fila-server/Cargo.toml (modified — added tower, http, pin-project-lite deps)
- Cargo.toml (modified — added tower, http, pin-project-lite workspace deps)
