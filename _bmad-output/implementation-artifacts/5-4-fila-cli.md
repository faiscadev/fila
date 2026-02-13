# Story 5.4: fila-cli

Status: done

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
- [x] Task 1: Add dependencies to fila-cli/Cargo.toml (AC: all)
  - [x] Subtask 1.1: Added clap, tokio, tonic, tonic-prost, fila-proto workspace deps
- [x] Task 2: Add `ListQueues` RPC to admin.proto and server (AC: #3)
  - [x] Subtask 2.1: Added `ListQueuesRequest`/`ListQueuesResponse`/`QueueInfo` messages to admin.proto
  - [x] Subtask 2.2: Added `rpc ListQueues` to FilaAdmin service
  - [x] Subtask 2.3: Added `ListQueues` command variant + `QueueSummary` struct to SchedulerCommand
  - [x] Subtask 2.4: Implemented `handle_list_queues` in scheduler with O(Q+K) pre-computed maps
  - [x] Subtask 2.5: Implemented `list_queues` in admin_service.rs
  - [x] Subtask 2.6: Added 2 scheduler tests + 1 admin test for ListQueues
- [x] Task 3: CLI scaffolding with clap derive (AC: #10, #11, #12)
  - [x] Subtask 3.1: Defined `Cli` struct with `--addr` global option (default `http://localhost:5555`)
  - [x] Subtask 3.2: Defined `Commands` enum: `Queue(QueueCommands)`, `Config(ConfigCommands)`, `Redrive`
  - [x] Subtask 3.3: Defined `QueueCommands`: `Create`, `Delete`, `List`, `Inspect`
  - [x] Subtask 3.4: Defined `ConfigCommands`: `Set`, `Get`, `List`
- [x] Task 4: gRPC client helper (AC: all)
  - [x] Subtask 4.1: Created async `connect(addr)` with error detail in failure message
  - [x] Subtask 4.2: Created `format_rpc_error(status, context)` with per-code mapping
- [x] Task 5: Implement queue commands (AC: #1, #2, #3, #4, #9)
  - [x] Subtask 5.1: `queue create` — calls CreateQueue, prints `Created queue "<name>"`
  - [x] Subtask 5.2: `queue delete` — calls DeleteQueue, prints `Deleted queue "<name>"`
  - [x] Subtask 5.3: `queue list` — calls ListQueues, prints aligned table (name, depth, in_flight, consumers)
  - [x] Subtask 5.4: `queue inspect` — calls GetStats, prints detailed stats with per-key and per-throttle tables
- [x] Task 6: Implement config commands (AC: #5, #6, #7, #9)
  - [x] Subtask 6.1: `config set` — calls SetConfig, prints `Set "<key>" = "<value>"`
  - [x] Subtask 6.2: `config get` — calls GetConfig, prints value or "(not set)"
  - [x] Subtask 6.3: `config list` — calls ListConfig, prints aligned table (key, value)
- [x] Task 7: Implement redrive command (AC: #8, #9)
  - [x] Subtask 7.1: `redrive` — calls Redrive, prints `Redrive complete: N message(s) moved from "<dlq>"`
- [x] Task 8: Human-friendly error messages (AC: #9)
  - [x] Subtask 8.1: format_rpc_error maps all gRPC status codes to actionable messages
  - [x] Subtask 8.2: `NOT_FOUND` -> `Error: <context> does not exist`
  - [x] Subtask 8.3: `ALREADY_EXISTS` -> `Error: <context> already exists`
  - [x] Subtask 8.4: `INVALID_ARGUMENT` -> `Error: <status.message()>`
  - [x] Subtask 8.5: `UNAVAILABLE` -> `Error: broker unavailable (connection lost or server down)`
  - [x] Subtask 8.6: Connection errors -> `Error: cannot connect to broker at <addr>: <error>`
- [x] Task 9: Tests (AC: all)
  - [x] Subtask 9.1: 2 scheduler tests + 1 admin test for ListQueues
  - [x] Subtask 9.2: extract_queue_id unit + proptest + rejection test

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

- `proto/fila/v1/admin.proto` — Added ListQueues RPC, QueueInfo, ListQueuesRequest/Response messages
- `crates/fila-core/src/error.rs` — Added ListQueuesError enum
- `crates/fila-core/src/lib.rs` — Re-exported ListQueuesError, QueueSummary
- `crates/fila-core/src/broker/command.rs` — Added QueueSummary struct, ListQueues command variant
- `crates/fila-core/src/broker/mod.rs` — Re-exported QueueSummary
- `crates/fila-core/src/broker/scheduler.rs` — handle_list_queues handler, dispatch arm, 2 tests
- `crates/fila-core/src/storage/keys.rs` — Added extract_queue_id function + 3 tests + 1 proptest
- `crates/fila-server/src/admin_service.rs` — list_queues gRPC handler, 1 test
- `crates/fila-server/src/error.rs` — IntoStatus for ListQueuesError
- `crates/fila-cli/Cargo.toml` — Added clap, tokio, tonic, tonic-prost deps
- `crates/fila-cli/src/main.rs` — Full CLI implementation (queue, config, redrive commands)

### Changelog

- `036c40f` feat: fila-cli with queue, config, and redrive commands
- `e0c3e45` fix: address code review findings for story 5.4
- `bb33a5f` test: add unit and property tests for extract_queue_id

### Test Count

241 tests passing (6 new: 2 scheduler + 1 admin + 3 keys)
