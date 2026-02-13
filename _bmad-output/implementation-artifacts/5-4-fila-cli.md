# Story 5.4: fila-cli

Status: ready-for-dev

## Story

As an operator,
I want a command-line interface for all admin operations,
so that I can manage the broker from the terminal without writing code.

## Acceptance Criteria

1. **Given** the broker is running, **when** an operator runs `fila queue create <name> [--on-enqueue <script>] [--on-failure <script>] [--visibility-timeout <ms>]`, **then** a queue is created and a confirmation message is printed
2. **Given** the broker is running, **when** an operator runs `fila queue delete <name>`, **then** the queue is deleted and a confirmation message is printed
3. **Given** the broker is running, **when** an operator runs `fila queue list`, **then** all queues are listed with basic stats in an aligned table
4. **Given** the broker is running, **when** an operator runs `fila queue inspect <name>`, **then** detailed queue stats are displayed (depth, in_flight, fairness keys, consumers, per-key stats, per-throttle stats)
5. **Given** the broker is running, **when** an operator runs `fila config set <key> <value>`, **then** the config value is set and a confirmation is printed
6. **Given** the broker is running, **when** an operator runs `fila config get <key>`, **then** the config value is printed
7. **Given** the broker is running, **when** an operator runs `fila config list [--prefix <prefix>]`, **then** all matching config entries are listed in an aligned table
8. **Given** the broker is running, **when** an operator runs `fila redrive <dlq-name> [--count <n>]`, **then** DLQ messages are redrived and a confirmation with count is printed
9. **Given** a gRPC error occurs, **when** the CLI receives it, **then** a human-friendly error message is printed (e.g., `NOT_FOUND` -> `Error: queue "foo" does not exist`)
10. **Given** any command, **when** the operator passes `--addr <host:port>`, **then** the CLI connects to the specified address instead of `localhost:5555`
11. **Given** any command, **when** `--help` is passed, **then** usage information is displayed with brief descriptions
12. **Given** the CLI crate, **when** compiled, **then** the binary is named `fila` (already configured in Cargo.toml)

## Tasks / Subtasks

- [ ] Task 1: Add dependencies to fila-cli/Cargo.toml (AC: all)
  - [ ] Subtask 1.1: Add `clap`, `tokio`, `tonic`, `tonic-prost`, `fila-proto` workspace deps
- [ ] Task 2: Add `ListQueues` RPC to admin.proto and server (AC: #3)
  - [ ] Subtask 2.1: Add `ListQueuesRequest`/`ListQueuesResponse` messages to admin.proto
  - [ ] Subtask 2.2: Add `rpc ListQueues` to FilaAdmin service
  - [ ] Subtask 2.3: Add `ListQueues` command variant to SchedulerCommand
  - [ ] Subtask 2.4: Implement `handle_list_queues` in scheduler (use `storage.list_queues()`)
  - [ ] Subtask 2.5: Implement `list_queues` in admin_service.rs
  - [ ] Subtask 2.6: Add scheduler + admin tests for ListQueues
- [ ] Task 3: CLI scaffolding with clap derive (AC: #10, #11, #12)
  - [ ] Subtask 3.1: Define `Cli` struct with `--addr` global option (default `http://localhost:5555`)
  - [ ] Subtask 3.2: Define `Commands` enum: `Queue(QueueCommands)`, `Config(ConfigCommands)`, `Redrive(RedriveArgs)`
  - [ ] Subtask 3.3: Define `QueueCommands`: `Create`, `Delete`, `List`, `Inspect`
  - [ ] Subtask 3.4: Define `ConfigCommands`: `Set`, `Get`, `List`
- [ ] Task 4: gRPC client helper (AC: all)
  - [ ] Subtask 4.1: Create async `connect(addr)` function returning `FilaAdminClient`
  - [ ] Subtask 4.2: Create `map_rpc_error(status, context)` function for human-friendly errors
- [ ] Task 5: Implement queue commands (AC: #1, #2, #3, #4, #9)
  - [ ] Subtask 5.1: `queue create` — call CreateQueue, print `Created queue "<name>"`
  - [ ] Subtask 5.2: `queue delete` — call DeleteQueue, print `Deleted queue "<name>"`
  - [ ] Subtask 5.3: `queue list` — call ListQueues, print aligned table (name, depth, in_flight, consumers)
  - [ ] Subtask 5.4: `queue inspect` — call GetStats, print detailed stats with per-key and per-throttle tables
- [ ] Task 6: Implement config commands (AC: #5, #6, #7, #9)
  - [ ] Subtask 6.1: `config set` — call SetConfig, print `Set "<key>" = "<value>"`
  - [ ] Subtask 6.2: `config get` — call GetConfig, print value (or "not set")
  - [ ] Subtask 6.3: `config list` — call ListConfig, print aligned table (key, value)
- [ ] Task 7: Implement redrive command (AC: #8, #9)
  - [ ] Subtask 7.1: `redrive` — call Redrive, print `Redrived <n> messages from "<dlq>" to "<parent>"`
- [ ] Task 8: Human-friendly error messages (AC: #9)
  - [ ] Subtask 8.1: Map gRPC status codes to actionable messages per command context
  - [ ] Subtask 8.2: `NOT_FOUND` -> `Error: queue "<name>" does not exist`
  - [ ] Subtask 8.3: `ALREADY_EXISTS` -> `Error: queue "<name>" already exists`
  - [ ] Subtask 8.4: `INVALID_ARGUMENT` -> `Error: <detail from status message>`
  - [ ] Subtask 8.5: `UNAVAILABLE` -> `Error: cannot connect to broker at <addr>`
  - [ ] Subtask 8.6: Connection errors -> `Error: cannot connect to broker at <addr>`
- [ ] Task 9: Tests (AC: all)
  - [ ] Subtask 9.1: ListQueues scheduler + admin tests
  - [ ] Subtask 9.2: CLI argument parsing tests (clap derive validates at compile time, but test subcommand routing)

## Dev Notes

### What Already Exists

- `fila-cli` crate with Cargo.toml (binary name `fila`) and placeholder main.rs
- `fila-proto` crate with generated gRPC client/server stubs for all admin RPCs
- Workspace deps: `clap` (v4, derive feature), `tokio`, `tonic`, `fila-proto`
- All admin RPCs except `ListQueues` already exist: CreateQueue, DeleteQueue, SetConfig, GetConfig, ListConfig, GetStats, Redrive
- `storage.list_queues()` exists in the storage layer — returns `Vec<QueueConfig>`

### Missing: ListQueues RPC

The proto has no `ListQueues` RPC. The `fila queue list` command needs one. Must add:
- Proto: `ListQueuesRequest {}` and `ListQueuesResponse { repeated QueueInfo queues }` with `QueueInfo { string name, uint64 depth, uint64 in_flight, uint32 consumers }`
- Scheduler: `ListQueues` command variant that calls `storage.list_queues()` and enriches each queue with depth/in_flight/consumer counts from in-memory state
- Admin service: handler that sends command and maps response

### CLI Architecture

Single-file CLI is fine — the command set is small. Structure:
```
src/main.rs:
  - Cli (clap derive) with --addr
  - Commands enum (Queue, Config, Redrive)
  - QueueCommands, ConfigCommands sub-enums
  - connect() helper
  - map_rpc_error() for human-friendly errors
  - One function per command (cmd_queue_create, cmd_queue_delete, etc.)
  - main() matches commands and calls functions
```

### Error Message Mapping

gRPC status → human-friendly text:
- `NOT_FOUND` → `Error: queue "<name>" does not exist` (for queue ops) or `Error: config key "<key>" not found`
- `ALREADY_EXISTS` → `Error: queue "<name>" already exists`
- `INVALID_ARGUMENT` → `Error: <status.message()>` (pass through, server already formats well)
- `FAILED_PRECONDITION` → `Error: <status.message()>` (e.g., parent queue not found for redrive)
- `UNAVAILABLE` → `Error: cannot connect to broker at <addr>`
- `RESOURCE_EXHAUSTED` → `Error: broker overloaded, try again`
- Transport/connection errors → `Error: cannot connect to broker at <addr>`
- Other → `Error: <status.message()>`

### Table Formatting

Use manual padding (no external crate needed). Example for `queue list`:
```
NAME             DEPTH   IN_FLIGHT  CONSUMERS
orders           1234    56         2
notifications    89      12         1
```

For `queue inspect`:
```
Queue: orders
  Depth:              1234
  In-flight:          56
  Active fairness keys: 3
  Active consumers:   2
  Quantum:            100

Fairness Keys:
  KEY              PENDING  DEFICIT  WEIGHT
  merchant-123     500      -200     1
  merchant-456     734      100      2

Throttle Keys:
  KEY              TOKENS  RATE/S   BURST
  rate:api         45.2    10.0     50.0
```

### Proto Changes

Add to `admin.proto`:
```protobuf
rpc ListQueues(ListQueuesRequest) returns (ListQueuesResponse);

message ListQueuesRequest {}

message QueueInfo {
  string name = 1;
  uint64 depth = 2;
  uint64 in_flight = 3;
  uint32 active_consumers = 4;
}

message ListQueuesResponse {
  repeated QueueInfo queues = 1;
}
```

### Redundant Commands Note

Epic says both `fila queue inspect <name>` and `fila stats <queue>`. These are the same operation (GetStats). Implement only `fila queue inspect <name>` to avoid duplication. The `stats` top-level is dropped — `inspect` under `queue` is the canonical path.

### Key Files to Modify

- `proto/fila/v1/admin.proto` — add ListQueues RPC
- `crates/fila-core/src/broker/command.rs` — add ListQueues command variant
- `crates/fila-core/src/broker/scheduler.rs` — handle_list_queues handler, dispatch, tests
- `crates/fila-server/src/admin_service.rs` — list_queues handler, tests
- `crates/fila-cli/Cargo.toml` — add dependencies
- `crates/fila-cli/src/main.rs` — full CLI implementation

### References

- [Source: proto/fila/v1/admin.proto] All existing admin RPCs
- [Source: crates/fila-core/src/storage/traits.rs:67] `list_queues()` storage method
- [Source: crates/fila-cli/Cargo.toml] Binary name already `fila`
- [Source: Cargo.toml:28] clap v4 with derive feature in workspace deps
- [Source: _bmad-output/planning-artifacts/epics.md:727-753] Epic 5 Story 5.4 definition

## Dev Agent Record

### Agent Model Used

Claude Opus 4.6

### Debug Log References

### Completion Notes List

### File List
