# Story 1.5: gRPC Server & Queue Management

Status: ready-for-dev

## Story

As an operator,
I want to start the broker and create/delete queues via gRPC,
So that I can set up the message infrastructure for my application.

## Acceptance Criteria

1. A tonic gRPC server listens on the configured address (default `0.0.0.0:5555`)
2. `CreateQueue` RPC accepts a queue name and configuration, persists it to the `queues` CF, and returns success
3. `CreateQueue` returns `ALREADY_EXISTS` status if the queue name is taken
4. `DeleteQueue` RPC removes the queue definition from storage
5. `DeleteQueue` returns `NOT_FOUND` status if the queue does not exist
6. gRPC handlers send `CreateQueue`/`DeleteQueue` commands to the scheduler via crossbeam channel
7. The scheduler processes admin commands and replies via oneshot channel
8. gRPC error responses use standard status codes: `NOT_FOUND`, `ALREADY_EXISTS`, `INVALID_ARGUMENT`, `INTERNAL`
9. The server binary (`fila-server`) reads config from `fila.toml` or `/etc/fila/fila.toml`, with env var overrides
10. An integration test creates a queue, verifies it exists, deletes it, and verifies it is gone

## Tasks / Subtasks

- [ ] Task 1: Add SchedulerCommand variants for queue management (AC: #6, #7)
  - [ ] 1.1 Add `CreateQueue { name: String, config: QueueConfig, reply }` to SchedulerCommand
  - [ ] 1.2 Add `DeleteQueue { queue_id: String, reply }` to SchedulerCommand
- [ ] Task 2: Implement queue management in scheduler (AC: #2, #3, #4, #5)
  - [ ] 2.1 Handle `CreateQueue`: check if queue exists, persist to storage, reply with success/error
  - [ ] 2.2 Handle `DeleteQueue`: check if queue exists, delete from storage, reply with success/error
- [ ] Task 3: Implement gRPC admin service (AC: #1, #6, #8)
  - [ ] 3.1 Create `crates/fila-server/src/admin_service.rs` implementing `FilaAdmin`
  - [ ] 3.2 `CreateQueue` handler: validate name, send command to broker, await reply, map errors to gRPC status
  - [ ] 3.3 `DeleteQueue` handler: send command to broker, await reply, map errors to gRPC status
  - [ ] 3.4 Stub remaining admin RPCs (SetConfig, GetConfig, GetStats, Redrive) as UNIMPLEMENTED
- [ ] Task 4: Implement server binary (AC: #1, #9)
  - [ ] 4.1 Update fila-server main.rs with tokio async main
  - [ ] 4.2 Read config from fila.toml (current dir or /etc/fila/)
  - [ ] 4.3 Init tracing, create RocksDB storage, create Broker
  - [ ] 4.4 Start tonic server with FilaAdmin service, serve until signal
  - [ ] 4.5 Graceful shutdown on SIGTERM/SIGINT
- [ ] Task 5: Update dependencies
  - [ ] 5.1 Add tonic, tokio, tracing to fila-server Cargo.toml
- [ ] Task 6: Write tests (AC: #10)
  - [ ] 6.1 Unit tests for scheduler queue management (create, duplicate, delete, delete nonexistent)
  - [ ] 6.2 Integration test: start server, create queue via gRPC, delete queue via gRPC
- [ ] Task 7: Verify build
  - [ ] 7.1 cargo build, clippy, fmt, nextest

## Dev Notes

### gRPC Service Architecture

The `FilaAdmin` gRPC service wraps a `Broker` handle. Each RPC:
1. Validates input
2. Creates a `SchedulerCommand` with a oneshot reply channel
3. Sends the command to the broker via `send_command()`
4. Awaits the reply via `reply_rx.await`
5. Maps the result to gRPC response or error status

### Error Mapping

| `FilaError` | gRPC Status |
|---|---|
| `QueueNotFound` | `NOT_FOUND` |
| `QueueAlreadyExists` | `ALREADY_EXISTS` |
| `InvalidConfig` | `INVALID_ARGUMENT` |
| `StorageError` | `INTERNAL` |
| Other | `INTERNAL` |

### Config File Resolution

Priority order:
1. `./fila.toml` (current directory)
2. `/etc/fila/fila.toml`
3. Default config (if no file found)

### References

- [Source: _bmad-output/planning-artifacts/architecture.md#Wire Protocol — gRPC]
- [Source: _bmad-output/planning-artifacts/architecture.md#Error Handling Strategy]
- [Source: _bmad-output/planning-artifacts/epics.md#Story 1.5]

## Dev Agent Record

### Agent Model Used

### Debug Log References

### Completion Notes List

### File List
