# Story 5.1: Configuration Listing & Operator Visibility

Status: ready-for-dev

## Story

As an operator,
I want to list and inspect all runtime configuration entries with optional prefix filtering,
so that I can understand the current broker configuration state without guessing key names.

## Acceptance Criteria

1. **Given** the broker has runtime configuration entries (set via `SetConfig`), **when** an operator calls `ListConfig` RPC with no prefix, **then** all key-value pairs from the `state` CF are returned
2. **Given** the broker has runtime configuration entries, **when** an operator calls `ListConfig` with a prefix (e.g. `throttle.`), **then** only entries whose keys start with that prefix are returned
3. **Given** the `ListConfig` response, **then** it includes the total count of matching entries
4. **Given** no entries match the prefix (or no entries exist), **when** `ListConfig` is called, **then** an empty list is returned (not an error)
5. **Given** multiple config values (throttle and non-throttle) have been set via `SetConfig`, **when** an integration test lists all entries, lists by prefix, **then** correct filtering is verified
6. **Given** a non-throttle config key is set via `SetConfig`, **when** a Lua `on_enqueue` script calls `fila.get(key)`, **then** the value is returned (end-to-end: SetConfig → Lua fila.get → verify)

## Tasks / Subtasks

- [ ] Task 1: Add `ListConfig` proto definitions (AC: #1, #2, #3)
  - [ ] Subtask 1.1: Add `ListConfigRequest { string prefix = 1; }` and `ListConfigResponse { repeated ConfigEntry entries = 1; uint32 total_count = 2; }` with `ConfigEntry { string key = 1; string value = 2; }` to `admin.proto`
  - [ ] Subtask 1.2: Add `rpc ListConfig(ListConfigRequest) returns (ListConfigResponse)` to `FilaAdmin` service
  - [ ] Subtask 1.3: Verify generated code compiles (`cargo build -p fila-proto`)
- [ ] Task 2: Add `ListConfig` scheduler command (AC: #1, #2)
  - [ ] Subtask 2.1: Add `ListConfig { prefix: String, reply: oneshot::Sender<Result<Vec<(String, String)>, ConfigError>> }` to `SchedulerCommand` in `command.rs`
  - [ ] Subtask 2.2: Add match arm in scheduler dispatch loop
- [ ] Task 3: Implement `handle_list_config` in scheduler (AC: #1, #2, #3, #4)
  - [ ] Subtask 3.1: Call `self.storage.list_state_by_prefix(&prefix)` (already exists from Story 4.3)
  - [ ] Subtask 3.2: Convert `Vec<(String, Vec<u8>)>` to `Vec<(String, String)>` with UTF-8 conversion
  - [ ] Subtask 3.3: Return results (empty vec is valid, not an error)
- [ ] Task 4: Implement `list_config` in admin service (AC: #1, #2, #3, #4)
  - [ ] Subtask 4.1: Validate prefix length (`<= MAX_CONFIG_KEY_LEN` = 256 bytes)
  - [ ] Subtask 4.2: Send `ListConfig` command to scheduler, await reply
  - [ ] Subtask 4.3: Map response to `ListConfigResponse` with entries and total_count
- [ ] Task 5: Scheduler unit tests (AC: #1, #2, #4)
  - [ ] Subtask 5.1: Test list_config with no prefix returns all entries
  - [ ] Subtask 5.2: Test list_config with prefix returns only matching entries
  - [ ] Subtask 5.3: Test list_config with no entries returns empty vec
- [ ] Task 6: Admin service unit tests (AC: #1, #2, #4)
  - [ ] Subtask 6.1: Test list_config returns all entries
  - [ ] Subtask 6.2: Test list_config with prefix filtering
  - [ ] Subtask 6.3: Test list_config with no entries returns empty list
  - [ ] Subtask 6.4: Test list_config with oversized prefix returns InvalidArgument
- [ ] Task 7: Integration test — ListConfig with prefix filtering (AC: #5)
  - [ ] Subtask 7.1: Set multiple config values (throttle and non-throttle) via SetConfig
  - [ ] Subtask 7.2: List all configs, verify all are returned
  - [ ] Subtask 7.3: List with `throttle.` prefix, verify only throttle entries returned
  - [ ] Subtask 7.4: List with non-matching prefix, verify empty list
- [ ] Task 8: Integration test — Lua e2e with non-throttle config (AC: #6)
  - [ ] Subtask 8.1: Create a queue with an `on_enqueue` Lua script that reads `fila.get("app.routing_key")` and uses it as the fairness key
  - [ ] Subtask 8.2: Set config `app.routing_key` = `"tenant-priority"` via SetConfig
  - [ ] Subtask 8.3: Enqueue a message to the queue
  - [ ] Subtask 8.4: Lease the message and verify its fairness_key is `"tenant-priority"` (proving Lua read the non-throttle config)

## Dev Notes

### What Already Exists (from Story 4.3)

All of these are implemented and merged to main — do NOT reimplement:
- `SetConfig` / `GetConfig` RPCs in admin service (`crates/fila-server/src/admin_service.rs:118-190`)
- `SetConfig` / `GetConfig` scheduler commands (`crates/fila-core/src/broker/command.rs:67-76`)
- `SetConfig` / `GetConfig` handlers in scheduler (`crates/fila-core/src/broker/scheduler.rs:406-455`)
- `Storage::list_state_by_prefix()` trait method and RocksDB implementation (`crates/fila-core/src/storage/traits.rs:69-81`, `rocksdb.rs:177-194`)
- `ConfigError` type (`crates/fila-core/src/error.rs:84-90`) and its `IntoStatus` mapping (`crates/fila-server/src/error.rs:56-63`)
- `fila.get()` Lua bridge reading from state CF (`crates/fila-core/src/lua/bridge.rs:13-36`)
- Input validation: `MAX_CONFIG_KEY_LEN = 256`, `MAX_CONFIG_VALUE_LEN = 1024` (`crates/fila-server/src/admin_service.rs`)

### What This Story Adds

1. **Proto:** `ListConfig` RPC + request/response messages
2. **Command:** `ListConfig` variant in `SchedulerCommand`
3. **Scheduler:** `handle_list_config` calling existing `list_state_by_prefix`
4. **Admin service:** `list_config` gRPC handler
5. **Tests:** scheduler unit tests, admin service unit tests, two integration tests

### Routing Pattern

All config operations route through the scheduler (single-writer guarantee). The ListConfig flow:

```
gRPC ListConfig → AdminService → SchedulerCommand::ListConfig → Scheduler:
  1. Call storage.list_state_by_prefix(prefix)
  2. Convert bytes to strings
  3. Reply with Vec<(String, String)>
```

### Proto Design

Add to `admin.proto`:
```protobuf
message ConfigEntry {
  string key = 1;
  string value = 2;
}

message ListConfigRequest {
  string prefix = 1;  // empty string = list all
}

message ListConfigResponse {
  repeated ConfigEntry entries = 1;
  uint32 total_count = 2;
}
```

Add to `FilaAdmin` service: `rpc ListConfig(ListConfigRequest) returns (ListConfigResponse);`

### Scheduler Handler

```rust
fn handle_list_config(&self, prefix: &str) -> Result<Vec<(String, String)>, ConfigError> {
    let entries = self.storage.list_state_by_prefix(prefix)?;
    entries.into_iter().map(|(k, v)| {
        let value_str = String::from_utf8(v)
            .map_err(|_| ConfigError::Storage(StorageError::Corruption("non-UTF8 config value".into())))?;
        Ok((k, value_str))
    }).collect()
}
```

### Admin Service Handler

Follow the exact pattern of `get_config` and `set_config`:
- Validate prefix length (reuse `MAX_CONFIG_KEY_LEN`)
- Empty prefix is valid (means "list all")
- Send `ListConfig` command to scheduler via broker
- Map result to `ListConfigResponse`

### Lua E2E Test Strategy

The Lua bridge (`fila.get()`) already reads from the state CF — no code changes needed for this. The test proves the end-to-end path: SetConfig → state CF → Lua fila.get → fairness key assignment.

The test queue needs an `on_enqueue` script like:
```lua
local routing = fila.get("app.routing_key")
return { fairness_key = routing or "default", weight = 1 }
```

Then: set config → enqueue message → lease → verify fairness_key matches config value.

### Broker API

The `Broker` struct needs a `list_config` method following the same pattern as `set_config`/`get_config`:
```rust
pub async fn list_config(&self, prefix: String) -> Result<Vec<(String, String)>, ConfigError> {
    let (tx, rx) = tokio::sync::oneshot::channel();
    self.send_command(SchedulerCommand::ListConfig { prefix, reply: tx })?;
    rx.await.map_err(|_| ...)?
}
```

Look at the existing `get_config` method in `crates/fila-core/src/broker/mod.rs` and follow the same pattern.

### Error Handling

`ConfigError` already covers what's needed:
- `InvalidValue` for prefix validation failures (if prefix too long)
- `Storage` for underlying storage errors

No new error variants needed.

### Key Files to Modify

- `proto/fila/v1/admin.proto` — add ListConfig RPC + messages
- `crates/fila-core/src/broker/command.rs` — add ListConfig variant
- `crates/fila-core/src/broker/mod.rs` — add list_config method to Broker
- `crates/fila-core/src/broker/scheduler.rs` — add handle_list_config + dispatch + tests
- `crates/fila-server/src/admin_service.rs` — implement list_config + tests

### Testing Patterns

- **Scheduler tests:** use `test_setup()` helper (creates scheduler + storage + channels in tempdir)
- **Admin service tests:** use `test_admin_service()` helper (creates real broker + tempdir)
- **Integration tests:** use the real gRPC server via tonic client (see existing integration test patterns in `crates/fila-server/src/admin_service.rs`)

### References

- [Source: proto/fila/v1/admin.proto] Current RPC definitions
- [Source: crates/fila-server/src/admin_service.rs:118-190] SetConfig/GetConfig implementation
- [Source: crates/fila-core/src/broker/command.rs:67-76] Existing config commands
- [Source: crates/fila-core/src/broker/scheduler.rs:406-455] Config handlers
- [Source: crates/fila-core/src/storage/traits.rs:69-81] list_state_by_prefix trait
- [Source: crates/fila-core/src/storage/rocksdb.rs:177-194] RocksDB prefix iteration
- [Source: crates/fila-core/src/lua/bridge.rs:13-36] fila.get() bridge
- [Source: crates/fila-core/src/error.rs:84-90] ConfigError
- [Source: _bmad-output/implementation-artifacts/4-3-runtime-throttle-rate-management.md] Previous story

## Dev Agent Record

### Agent Model Used

### Debug Log References

### Completion Notes List

### File List
