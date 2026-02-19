# Story 8.1: Blackbox End-to-End Test Suite

Status: review

## Story

As a developer,
I want a comprehensive blackbox e2e test suite that exercises the full system through the SDK and CLI,
so that I can refactor scheduler internals safely with behavioral regression coverage.

## Acceptance Criteria

1. **Given** a test harness, **when** the e2e test suite runs, **then** a `fila-server` instance is started as a subprocess per test (or per group) using the proven `TestServer` pattern from `fila-sdk`
2. **Given** the e2e test crate, **when** its dependencies are inspected, **then** it depends only on `fila-sdk` and `fila-proto` — no `fila-core` or `fila-server` internal types
3. **Given** the test harness, **when** producer/consumer operations execute, **then** they use the `fila-sdk` `FilaClient` (not raw gRPC calls)
4. **Given** the test harness, **when** admin operations execute, **then** they use the `fila` CLI binary as a subprocess (not `FilaAdminClient` directly) — true blackbox
5. **Given** the e2e suite, **when** all tests pass, **then** the following flows are verified:
   - **(a)** Enqueue → Lease → Ack lifecycle (basic message flow via SDK)
   - **(b)** Enqueue → Lease → Nack → re-Lease (retry with attempt count increment via SDK)
   - **(c)** Lua `on_enqueue` assigns fairness key, weight, and throttle keys from headers
   - **(d)** Lua `on_failure` decides retry vs DLQ based on attempt count
   - **(e)** DLQ flow: nack to exhaustion → message in DLQ → Redrive via CLI → re-Lease from source queue
   - **(f)** DRR fairness: multi-key weighted delivery (higher-weight key gets proportionally more)
   - **(g)** Throttle: rate-limited key skipped, unthrottled keys served immediately
   - **(h)** Config: `fila config set` → `fila config get` → ListConfig with prefix filter → Lua `fila.get()` reads value
   - **(i)** Queue management: `fila queue create` → `fila queue list` → `fila queue inspect` → `fila queue delete`
   - **(j)** Crash recovery: enqueue messages → kill server → restart → verify all messages available for lease
   - **(k)** Visibility timeout: lease message → wait for expiry → message available for re-lease
6. **Given** the test design, **when** tests run, **then** each test is independent — separate ports, separate temp data dirs — and can run in parallel
7. **Given** the test harness, **when** test infrastructure is inspected, **then** a shared test helper module starts/stops `fila-server` instances and creates SDK clients
8. **Given** the new e2e tests, **when** `cargo nextest run` executes, **then** all existing unit/integration tests continue to pass alongside the new e2e tests

## Tasks / Subtasks

- [x] Task 1: Create `fila-e2e` test crate in workspace (AC: #2)
  - [x] Subtask 1.1: Create `crates/fila-e2e/Cargo.toml` with dependencies: `fila-sdk` (workspace), `fila-proto` (workspace), `tokio`, `tempfile`, `tokio-stream`
  - [x] Subtask 1.2: Add `fila-e2e` to workspace members in root `Cargo.toml`
  - [x] Subtask 1.3: Create `crates/fila-e2e/src/lib.rs` (empty — this is a test-only crate)
  - [x] Subtask 1.4: Create `crates/fila-e2e/tests/` directory for integration test files

- [x] Task 2: Build test harness with TestServer and CLI helpers (AC: #1, #3, #4, #7)
  - [x] Subtask 2.1: Create `crates/fila-e2e/tests/helpers/mod.rs` with `TestServer` struct (port from existing `fila-sdk/tests/integration.rs` pattern)
  - [x] Subtask 2.2: `TestServer::start()` — find free port, write `fila.toml` to temp dir, spawn `fila-server` binary, poll TCP until ready (with assertion on success)
  - [x] Subtask 2.3: Add `cli_run(addr, args)` helper — spawns `fila` CLI binary with `--addr` flag and given arguments, returns stdout/stderr and exit code
  - [x] Subtask 2.4: Add `create_queue_via_cli(addr, name, opts)` helper — wraps `fila queue create` with optional `--on-enqueue`, `--on-failure`, `--visibility-timeout` flags
  - [x] Subtask 2.5: Add `sdk_client(addr)` convenience — wraps `FilaClient::connect(addr)`
  - [x] Subtask 2.6: Binary path resolution for both `fila-server` and `fila` CLI from `CARGO_MANIFEST_DIR` or `target/debug/`

- [x] Task 3: Basic lifecycle e2e tests (AC: #5a, #5b)
  - [x] Subtask 3.1: Test `e2e_enqueue_lease_ack` — create queue via CLI → enqueue via SDK → lease via SDK → ack via SDK → verify double-ack returns error
  - [x] Subtask 3.2: Test `e2e_enqueue_lease_nack_retry` — create queue via CLI → enqueue → lease → nack → re-lease → verify attempt_count incremented

- [x] Task 4: Lua hook e2e tests (AC: #5c, #5d)
  - [x] Subtask 4.1: Test `e2e_lua_on_enqueue_assigns_keys` — create queue via CLI with `--on-enqueue` script that reads `msg.headers["tenant"]` as fairness_key → enqueue messages with different tenant headers → lease → verify fairness_key on received messages
  - [x] Subtask 4.2: Test `e2e_lua_on_failure_retry_vs_dlq` — create queue via CLI with `--on-failure` script that dead-letters after 3 attempts → enqueue → nack 3 times → verify message appears in DLQ (lease from `{queue}.dlq`)

- [x] Task 5: DLQ and redrive e2e test (AC: #5e)
  - [x] Subtask 5.1: Test `e2e_dlq_redrive` — use on_failure to dlq after N attempts → nack to exhaustion → verify in DLQ → redrive via CLI (`fila redrive {queue}.dlq`) → lease from source queue → verify message available with reset attempt count

- [x] Task 6: DRR fairness e2e test (AC: #5f)
  - [x] Subtask 6.1: Test `e2e_drr_weighted_fairness` — create queue with on_enqueue assigning fairness_key and weight from headers → enqueue many messages (e.g., 100+ across 2-3 keys with different weights) → lease all → verify distribution is within 15% of expected fair share (relaxed tolerance for e2e timing)

- [x] Task 7: Throttle e2e test (AC: #5g)
  - [x] Subtask 7.1: Test `e2e_throttle_rate_limiting` — create queue with on_enqueue assigning throttle_keys → set throttle rate via CLI (`fila config set throttle.{key} {rate}`) → enqueue messages → verify delivery rate is throttled (measure time between received messages)

- [x] Task 8: Config e2e test (AC: #5h)
  - [x] Subtask 8.1: Test `e2e_config_set_get_list_lua` — `fila config set mykey myvalue` via CLI → `fila config get mykey` via CLI → verify output → create queue with on_enqueue that calls `fila.get("mykey")` and uses it as fairness_key → enqueue → lease → verify fairness_key equals "myvalue"

- [x] Task 9: Queue management e2e test (AC: #5i)
  - [x] Subtask 9.1: Test `e2e_queue_management_lifecycle` — `fila queue create foo` → `fila queue list` → verify "foo" in output → `fila queue inspect foo` → verify stats output → `fila queue delete foo` → `fila queue list` → verify "foo" gone

- [x] Task 10: Crash recovery e2e test (AC: #5j)
  - [x] Subtask 10.1: Test `e2e_crash_recovery` — start server → create queue → enqueue messages via SDK → kill server process (SIGKILL, not graceful) → start new server on same data dir and port → lease from queue → verify all messages available (zero loss)

- [x] Task 11: Visibility timeout e2e test (AC: #5k)
  - [x] Subtask 11.1: Test `e2e_visibility_timeout_expiry` — create queue with short visibility timeout (e.g., 2s) via CLI → enqueue → lease (hold without ack) → wait for timeout + 1s → lease again on new stream → verify same message redelivered with incremented attempt count

- [x] Task 12: Verify all existing tests pass (AC: #8)
  - [x] Subtask 12.1: Run `cargo nextest run` across the entire workspace — zero regressions in existing 267 tests

## Dev Notes

### Architecture Compliance

- **New crate**: `fila-e2e` is a test-only workspace member at `crates/fila-e2e/`. It exists solely for integration tests — `src/lib.rs` can be empty.
- **Blackbox constraint**: The crate MUST NOT depend on `fila-core` or `fila-server`. Only `fila-sdk` and `fila-proto` (for admin client types needed alongside CLI). Admin operations should use the `fila` CLI binary as a subprocess wherever possible.
- **CLI binary name**: The CLI binary is `fila` (built from crate `fila-cli`). The server binary is `fila-server` (built from crate `fila-server`).

### TestServer Pattern (from fila-sdk)

The existing `TestServer` in `crates/fila-sdk/tests/integration.rs` is the proven pattern. Port it to the e2e crate's helpers module:

1. **Free port**: Bind to `127.0.0.1:0`, read assigned port
2. **Temp dir**: `tempfile::tempdir()` for data and config
3. **Config**: Write minimal `fila.toml` with `[server]` and `[telemetry]` sections to temp dir
4. **Binary**: Resolve from `CARGO_MANIFEST_DIR` → workspace root → `target/debug/fila-server`
5. **Spawn**: `std::process::Command` with `FILA_DATA_DIR` env var pointing to temp dir
6. **Readiness**: Poll TCP connection to port with timeout; assert connection succeeded
7. **Cleanup**: `Drop` impl kills child process

For the `fila` CLI binary, resolve similarly: workspace root → `target/debug/fila`

### CLI Helper Design

Create a `cli_run` function that:
```
fn cli_run(addr: &str, args: &[&str]) -> CliOutput {
    // Spawn: fila --addr {addr} {args...}
    // Capture stdout, stderr, exit code
    // Return structured result
}
```

Use this for all admin operations: queue create/delete/list/inspect, config set/get, redrive.

### Key Test Design Decisions

- **Fairness tolerance**: Unit tests use 5% tolerance. E2e tests should use 15% tolerance — delivery timing jitter, scheduler wake timing, and process startup overhead make exact measurements unreliable in e2e context.
- **Throttle verification**: Rather than measuring exact rate, verify ordering (unthrottled messages arrive first) or use a very low rate (1 msg/s) with a reasonable time window.
- **Crash recovery**: Use `child.kill()` (SIGKILL) to simulate crash, NOT graceful shutdown. Restart with same data dir. This verifies RocksDB WAL recovery.
- **Visibility timeout**: Use a short timeout (2s) to keep tests fast. Wait slightly longer than the timeout before re-leasing.
- **Lua scripts**: Pass scripts as string arguments to `fila queue create --on-enqueue '...'`. Keep scripts simple and deterministic.

### Lua Script Examples for Tests

**on_enqueue (fairness key from header)**:
```lua
return { fairness_key = msg.headers["tenant"] or "default", weight = tonumber(msg.headers["weight"]) or 1, throttle_keys = {} }
```

**on_enqueue (with throttle keys)**:
```lua
local keys = {}
if msg.headers["provider"] then table.insert(keys, "provider:" .. msg.headers["provider"]) end
return { fairness_key = msg.headers["tenant"] or "default", weight = 1, throttle_keys = keys }
```

**on_failure (DLQ after 3 attempts)**:
```lua
if msg.attempts >= 3 then return { action = "dlq" } end
return { action = "retry", delay_ms = 0 }
```

### Proto Types for Admin Client (supplementary)

While admin operations should use CLI where possible, some verification may need `FilaAdminClient` from `fila-proto`:
- `fila_proto::fila_admin_client::FilaAdminClient` — for queue creation when CLI doesn't support all options
- `CreateQueueRequest { name, config: Some(QueueConfig { on_enqueue_script, on_failure_script, visibility_timeout_ms }) }`

The AC says admin operations use CLI. However, for queue creation with Lua scripts, using `FilaAdminClient` directly is acceptable if the CLI's `--on-enqueue` flag doesn't handle multi-line scripts well. The key blackbox constraint is: no `fila-core` or `fila-server` internal types.

### Dependencies (Cargo.toml)

```toml
[package]
name = "fila-e2e"
version = "0.1.0"
edition = "2021"

[dependencies]
# Empty — test-only crate

[dev-dependencies]
fila-sdk = { workspace = true }
fila-proto = { workspace = true }
tokio = { workspace = true, features = ["full"] }
tokio-stream = { workspace = true }
tempfile = "3"
tonic = { workspace = true }
```

### Previous Story Intelligence (from Story 7.1)

- `TestServer` startup poll must assert server actually became reachable (Cubic finding from PR #30)
- Binary path resolution: `CARGO_MANIFEST_DIR` → pop twice to workspace root → `target/debug/{binary}`
- `filter_map` for `None` lease messages is correct (tonic keepalive frames)
- Per-operation error types (`EnqueueError`, `LeaseError`, `AckError`, `NackError`) — assert exact error variants in tests using `matches!()` macro
- Each test gets its own server instance with random port and temp dir for isolation

### What NOT To Do

- Do NOT import from `fila-core` or `fila-server` — this is blackbox testing
- Do NOT duplicate the SDK integration tests already in `fila-sdk/tests/integration.rs` — the e2e suite tests higher-level flows
- Do NOT add retry/reconnection logic to tests — if a connection fails, the test should fail fast
- Do NOT use hardcoded ports — always bind to port 0 and discover
- Do NOT use `sleep` for synchronization — use `tokio::time::timeout` around `stream.next()` with generous timeouts

### Project Structure Notes

```
crates/fila-e2e/
├── Cargo.toml
├── src/
│   └── lib.rs              # Empty (test-only crate)
└── tests/
    ├── helpers/
    │   └── mod.rs           # TestServer, cli_run, binary resolution
    ├── lifecycle.rs          # AC 5a, 5b — basic enqueue/lease/ack/nack
    ├── lua_hooks.rs          # AC 5c, 5d — on_enqueue, on_failure
    ├── dlq_redrive.rs        # AC 5e — DLQ flow and redrive
    ├── fairness.rs           # AC 5f — DRR weighted delivery
    ├── throttle.rs           # AC 5g — rate limiting
    ├── config.rs             # AC 5h — config set/get/list/lua
    ├── queue_management.rs   # AC 5i — queue CRUD via CLI
    ├── crash_recovery.rs     # AC 5j — kill/restart/verify
    └── visibility_timeout.rs # AC 5k — lease expiry
```

### References

- [Source: _bmad-output/planning-artifacts/epics.md#Story 8.1]
- [Source: _bmad-output/planning-artifacts/architecture.md#Testing Strategy]
- [Source: crates/fila-sdk/tests/integration.rs — TestServer pattern, binary resolution]
- [Source: crates/fila-sdk/src/client.rs — FilaClient API surface]
- [Source: crates/fila-sdk/src/error.rs — per-operation error types]
- [Source: crates/fila-cli/src/main.rs — CLI commands and --addr flag]
- [Source: _bmad-output/implementation-artifacts/epic-7-retro-2026-02-19.md — TestServer reliability fix, SDK-first validation]

## Dev Agent Record

### Agent Model Used
Claude Opus 4.6

### Debug Log References
None

### Completion Notes List
- All 11 e2e test scenarios implemented and passing (278 total workspace tests, 0 regressions)
- Lua scripts must define named functions (`function on_enqueue(msg) ... end`), not bare return statements — the execution flow loads bytecode, extracts the named function from globals, then calls it
- Throttle config format is `"rate_per_second,burst"` (e.g., `"1,1"`)
- DRR fairness test requires small quantum (1) to observe weighted distribution immediately — with default quantum=1000, both keys alternate 1-for-1 for the first 2000 deliveries, masking the weight difference
- DLQ redrive test must drop consumer streams before redrive — open streams capture redriven messages
- TestServer supports `start_with_quantum()` for configurable DRR quantum via TOML config

### File List
- `crates/fila-e2e/Cargo.toml` (new) — test-only crate with fila-sdk, fila-proto, tokio, tokio-stream, tonic, tempfile
- `crates/fila-e2e/src/lib.rs` (new) — empty library marker
- `crates/fila-e2e/tests/helpers/mod.rs` (new) — TestServer, TestServerOptions, CLI helpers, binary resolution
- `crates/fila-e2e/tests/lifecycle.rs` (new) — AC 5a, 5b: enqueue/lease/ack and nack/retry
- `crates/fila-e2e/tests/lua_hooks.rs` (new) — AC 5c, 5d: on_enqueue and on_failure hooks
- `crates/fila-e2e/tests/dlq_redrive.rs` (new) — AC 5e: DLQ flow and redrive via CLI
- `crates/fila-e2e/tests/fairness.rs` (new) — AC 5f: DRR weighted delivery with quantum=1
- `crates/fila-e2e/tests/throttle.rs` (new) — AC 5g: rate-limited key skipped
- `crates/fila-e2e/tests/config.rs` (new) — AC 5h: config set/get/list and Lua fila.get()
- `crates/fila-e2e/tests/queue_management.rs` (new) — AC 5i: queue CRUD via CLI
- `crates/fila-e2e/tests/crash_recovery.rs` (new) — AC 5j: kill/restart/verify
- `crates/fila-e2e/tests/visibility_timeout.rs` (new) — AC 5k: lease expiry and redelivery
- `Cargo.toml` (modified) — added fila-e2e to workspace members
