# Story 4.3: Runtime Throttle Rate Management

Status: review

## Story

As an operator,
I want to set and update throttle rates at runtime without restarting the broker,
so that I can respond to production conditions by adjusting rate limits.

## Acceptance Criteria

1. **Given** the broker is running with active throttle keys, **when** an operator calls `SetConfig` with a throttle-prefixed key (e.g. `throttle.provider_a`), **then** the ThrottleManager creates or updates the token bucket for that throttle key
2. **Given** a throttle rate is set via `SetConfig`, **when** the scheduler processes the next refill cycle, **then** the new rate takes effect without restart (FR16)
3. **Given** a throttle rate has been set, **when** the operator removes it by calling `SetConfig` with a delete convention (empty value), **then** the token bucket is removed and messages with that key become unthrottled
4. **Given** throttle rates have been set via `SetConfig`, **when** the broker restarts, **then** throttle rates are restored from the `state` CF and applied to the ThrottleManager during recovery
5. **Given** a throttle rate key is stored in the `state` CF, **when** a Lua script calls `fila.get("throttle.provider_a")`, **then** it can read the throttle config value for dynamic decisions
6. **Given** the `GetConfig` RPC is called with a throttle-prefixed key, **when** the key exists in the `state` CF, **then** the current value is returned
7. **Given** a queue with Lua assigning throttle keys and a low rate limit, **when** an integration test sets a throttle rate, verifies enforcement, updates the rate, and verifies the new rate takes effect, **then** the test passes end-to-end

## Tasks / Subtasks

- [x] Task 1: Implement `SetConfig` RPC in admin service (AC: #1, #2, #3)
  - [x] Subtask 1.1: Validate input: key must not be empty, value is a string
  - [x] Subtask 1.2: Parse throttle config routed through scheduler (not in admin service)
  - [x] Subtask 1.3–1.6: All routing through SetConfig scheduler command
- [x] Task 2: Implement `GetConfig` RPC in admin service (AC: #5, #6)
  - [x] Subtask 2.1: Validate input: key must not be empty
  - [x] Subtask 2.2: Read value from state CF via GetConfig scheduler command
  - [x] Subtask 2.3: Return the value or an empty string if not found
- [x] Task 3: Add `SetConfig` and `GetConfig` scheduler commands (AC: #1, #6)
  - [x] Subtask 3.1: Add `SetConfig { key, value, reply }` and `GetConfig { key, reply }` to `SchedulerCommand`
  - [x] Subtask 3.2: Handle `SetConfig` in scheduler: persist to state CF, then if throttle-prefixed, also call `self.throttle.set_rate()` or `self.throttle.remove_rate()`
  - [x] Subtask 3.3: Handle `GetConfig` in scheduler: read from state CF and return
- [x] Task 4: Persist throttle rates to `state` CF (AC: #4, #5)
  - [x] Subtask 4.1: On `SetConfig` with throttle prefix: `put_state("throttle.{key}", "{rate},{burst}")` to state CF
  - [x] Subtask 4.2: On `SetConfig` with empty value: `delete_state("throttle.{key}")` from state CF
- [x] Task 5: Add `list_state_by_prefix` to storage trait (AC: #4)
  - [x] Subtask 5.1: Add `list_state_by_prefix(prefix: &str) -> StorageResult<Vec<(String, Vec<u8>)>>` to `Storage` trait
  - [x] Subtask 5.2: Implement in `RocksDbStorage` using RocksDB prefix iterator on the state CF
  - [x] Subtask 5.3: Add unit test for the new method
- [x] Task 6: Load throttle rates on recovery (AC: #4)
  - [x] Subtask 6.1: In `Scheduler::recover()`, after restoring queues and messages, scan `state` CF for `throttle.` prefixed keys
  - [x] Subtask 6.2: For each throttle key found, parse the value and call `self.throttle.set_rate()`
  - [x] Subtask 6.3: Log the number of throttle rates restored
- [x] Task 7: Unit tests for SetConfig/GetConfig (AC: #1, #2, #3, #6)
  - [x] Subtask 7.1: Test SetConfig with throttle key sets rate in ThrottleManager
  - [x] Subtask 7.2: Test SetConfig with empty value removes throttle rate
  - [x] Subtask 7.3: Test SetConfig persists to state CF
  - [x] Subtask 7.4: Test GetConfig reads throttle config from state CF
  - [x] Subtask 7.5: Test SetConfig with non-throttle key persists without affecting ThrottleManager
  - [x] Subtask 7.6: Test SetConfig with invalid throttle value returns error
- [x] Task 8: Unit test for recovery loading throttle rates (AC: #4)
  - [x] Subtask 8.1: Set throttle rates, restart scheduler, verify ThrottleManager has the rates
- [x] Task 9: Integration test for runtime rate changes (AC: #7)
  - [x] Subtask 9.1: End-to-end test: set throttle rate via SetConfig, enqueue throttled messages, verify enforcement
- [x] Task 10: Admin service unit tests (AC: #1, #6)
  - [x] Subtask 10.1: Test set_config with valid throttle key
  - [x] Subtask 10.2: Test set_config with empty key returns InvalidArgument
  - [x] Subtask 10.3: Test get_config with empty key returns InvalidArgument
  - [x] Subtask 10.4: Test get_config with missing key returns empty value

## Dev Notes

### Architecture Overview

This story wires the existing `SetThrottleRate`/`RemoveThrottleRate` scheduler commands (added in Story 4.2) to the gRPC admin API, adds persistence, and adds recovery. The `SetConfig`/`GetConfig` RPCs are generic key-value operations, but throttle-prefixed keys trigger additional side effects (applying rate to ThrottleManager).

### Key Convention

Following the architecture doc's `dot.separated.path` pattern for RocksDB state CF keys:
- `throttle.{throttle_key}` → value: `"{rate_per_second},{burst}"` (comma-separated f64 pair)
- Example: `throttle.provider_a` → `"10.0,100.0"`
- Example: `throttle.region:us-east-1` → `"50.0,200.0"`

The `throttle.` prefix is the discriminator. Everything after `throttle.` is the throttle key name passed to `ThrottleManager::set_rate()`.

### Routing: Admin Service → Scheduler

Since the scheduler is single-threaded and owns both `ThrottleManager` and `Storage`, all config operations route through scheduler commands:

```
gRPC SetConfig → AdminService → SchedulerCommand::SetConfig → Scheduler handles:
  1. Persist to state CF
  2. If throttle-prefixed: apply to ThrottleManager
```

This avoids concurrent access to storage and keeps the single-writer guarantee.

### Why Not Direct Storage Access from Admin Service?

The architecture uses a single-threaded scheduler as the sole writer to storage. Bypassing it would introduce concurrent writes. Route everything through commands.

### SetConfig/GetConfig Commands

Add to `SchedulerCommand`:
```rust
SetConfig {
    key: String,
    value: String,
    reply: tokio::sync::oneshot::Sender<Result<(), ConfigError>>,
},
GetConfig {
    key: String,
    reply: tokio::sync::oneshot::Sender<Result<Option<String>, ConfigError>>,
},
```

The handler for `SetConfig`:
```rust
// 1. Persist raw key-value to state CF
if value.is_empty() {
    self.storage.delete_state(&key)?;
} else {
    self.storage.put_state(&key, value.as_bytes())?;
}
// 2. If throttle-prefixed, apply side effect
if let Some(throttle_key) = key.strip_prefix("throttle.") {
    if value.is_empty() {
        self.throttle.remove_rate(throttle_key);
    } else {
        let (rate, burst) = parse_throttle_value(&value)?;
        self.throttle.set_rate(throttle_key, rate, burst);
    }
}
```

### Removing Existing SetThrottleRate/RemoveThrottleRate

Once `SetConfig` handles throttle operations, the existing `SetThrottleRate` and `RemoveThrottleRate` commands become redundant. However, keep them for now — they're used in tests for direct low-level control. The admin API routes through `SetConfig` which persists + applies, while the direct commands are useful for tests that don't need persistence.

### list_state_by_prefix

Need a new method on `Storage` to scan state CF keys with a prefix (for recovery). The RocksDB implementation uses `prefix_iterator_cf` or a bounded `iterator_cf` with `ReadOptions::set_iterate_range`.

```rust
fn list_state_by_prefix(&self, prefix: &str) -> StorageResult<Vec<(String, Vec<u8>)>> {
    let cf = self.cf(CF_STATE)?;
    let mut result = Vec::new();
    let iter = self.db.prefix_iterator_cf(&cf, prefix.as_bytes());
    for item in iter {
        let (key, value) = item?;
        let key_str = std::str::from_utf8(&key)
            .map_err(|_| StorageError::Corruption("non-UTF8 state key".into()))?;
        if !key_str.starts_with(prefix) {
            break; // Past the prefix range
        }
        result.push((key_str.to_string(), value.to_vec()));
    }
    Ok(result)
}
```

### Recovery: Loading Throttle Rates

In `Scheduler::recover()`, after the existing queue/message/lease recovery, add:
```rust
// Restore throttle rates from state CF
let throttle_entries = self.storage.list_state_by_prefix("throttle.")?;
for (key, value) in &throttle_entries {
    if let Some(throttle_key) = key.strip_prefix("throttle.") {
        if let Ok(value_str) = std::str::from_utf8(value) {
            if let Some((rate, burst)) = parse_throttle_value(value_str) {
                self.throttle.set_rate(throttle_key, rate, burst);
            }
        }
    }
}
info!(count = throttle_entries.len(), "restored throttle rates");
```

### Error Type

Add a new `ConfigError` per the per-command error pattern:
```rust
pub enum ConfigError {
    InvalidValue(String),
    Storage(StorageError),
}
```

### Proto Changes

No proto changes needed — `SetConfigRequest { key, value }` and `GetConfigRequest { key }` are already defined in `admin.proto` and sufficient for this use case.

### Existing Test Patterns

- Admin service tests: use `test_admin_service()` helper creating a real broker + tempdir
- Scheduler tests: use `create_test_scheduler()` helper with storage + channel
- Both patterns are fine for the tests needed here

### Key Files to Modify

- `crates/fila-server/src/admin_service.rs` — implement SetConfig and GetConfig
- `crates/fila-server/src/error.rs` — add ConfigError → Status conversion
- `crates/fila-core/src/broker/command.rs` — add SetConfig and GetConfig commands
- `crates/fila-core/src/broker/scheduler.rs` — handle new commands, recovery loading
- `crates/fila-core/src/error.rs` — add ConfigError
- `crates/fila-core/src/storage/traits.rs` — add list_state_by_prefix
- `crates/fila-core/src/storage/rocksdb.rs` — implement list_state_by_prefix
- `crates/fila-core/src/lib.rs` — re-export ConfigError

### References

- [Source: _bmad-output/planning-artifacts/architecture.md — Admin RPCs] SetConfig/GetConfig definition
- [Source: _bmad-output/planning-artifacts/architecture.md — Runtime Config Layer 2] state CF pattern
- [Source: _bmad-output/planning-artifacts/epics.md — Story 4.3] Full acceptance criteria
- [Source: crates/fila-core/src/broker/command.rs:57-64] SetThrottleRate/RemoveThrottleRate
- [Source: crates/fila-core/src/broker/scheduler.rs:207-218] SetThrottleRate/RemoveThrottleRate handling
- [Source: crates/fila-server/src/admin_service.rs:114-126] Unimplemented SetConfig/GetConfig stubs
- [Source: proto/fila/v1/admin.proto:35-48] SetConfig/GetConfig proto definitions
- [Source: crates/fila-core/src/storage/traits.rs:72-78] State CF operations

## Dev Agent Record

### Agent Model Used

Claude Opus 4.6

### Debug Log References

- Fixed `create_test_scheduler()` → `test_setup()` (wrong test helper name from spec)
- Fixed recovery test: `Scheduler::new` doesn't call `recover()`, need to call `run()` with `Shutdown`
- Fixed `mut consumer_rx` for `try_recv()` calls

### Completion Notes List

- All 10 tasks complete, 15 new tests (10 scheduler, 1 storage, 4 admin service)
- Chose unified SetConfig/GetConfig commands over separate throttle commands for admin API
- Throttle-prefix side effects happen inside scheduler handler (single-writer guarantee)
- ConfigError follows per-command error type pattern
- Existing SetThrottleRate/RemoveThrottleRate commands retained for direct test use

### Change Log

- `a9d10fe` feat: runtime throttle rate management via setconfig/getconfig rpcs
- `6021c10` fix: address code review findings for story 4.3 (H1: validate-before-persist, H2: reject NaN/Inf, M3: iterator_cf, M4: empty key, M5: size limits, L7: integration test)

### File List

- `crates/fila-core/src/broker/command.rs` — added SetConfig, GetConfig command variants
- `crates/fila-core/src/broker/scheduler.rs` — handle_set_config, handle_get_config, parse_throttle_value, throttle recovery in recover(), 10 new tests
- `crates/fila-core/src/error.rs` — added ConfigError enum
- `crates/fila-core/src/lib.rs` — re-export ConfigError
- `crates/fila-core/src/storage/traits.rs` — added list_state_by_prefix to Storage trait
- `crates/fila-core/src/storage/rocksdb.rs` — implemented list_state_by_prefix + 1 test
- `crates/fila-server/src/admin_service.rs` — implemented set_config, get_config + 4 tests
- `crates/fila-server/src/error.rs` — added ConfigError → Status conversion
