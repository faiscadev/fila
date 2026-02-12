# Story 3.3: on_failure Hook & Retry Decisions

Status: ready-for-dev

## Story

As a platform engineer,
I want to write a Lua script that decides whether a failed message should be retried or dead-lettered,
so that I can implement custom retry policies like exponential backoff with max attempts.

## Acceptance Criteria

1. **Given** a queue is created with an `on_failure` Lua script, **when** a consumer nacks a message, **then** the Lua script receives: `msg.headers` (table), `msg.id` (string), `msg.attempts` (number), `msg.queue` (string), and `error` (string), and returns `{ action = "retry"|"dlq", delay_ms = <number> }`
2. **Given** the on_failure script returns `action = "retry"`, **then** the message is requeued with incremented attempt count (delay_ms logged as warning, not yet supported)
3. **Given** the on_failure script returns `action = "dlq"`, **then** the message is moved to the queue's dead-letter queue via atomic WriteBatch
4. **Given** no on_failure script is attached, **then** the default behavior is immediate retry (backward compatible with current nack behavior)
5. **Given** the circuit breaker is active for on_failure, **then** the default action is "retry" with no delay
6. **Given** the same safety limits (timeout, memory, circuit breaker) apply to on_failure as to on_enqueue

## Tasks / Subtasks

- [ ] Task 1: Create `on_failure.rs` module with `OnFailureResult` and `try_run_on_failure`
- [ ] Task 2: Add on_failure cache and execution to `LuaEngine` (parallel to on_enqueue)
- [ ] Task 3: Update `handle_nack` and `handle_create_queue` to use on_failure scripts
- [ ] Task 4: Implement DLQ message routing in `handle_nack`
- [ ] Task 5: Update recovery to re-cache on_failure scripts
- [ ] Task 6: Integration tests

## Dev Notes

### File Structure

```
crates/fila-core/src/lua/
├── on_failure.rs  # NEW: OnFailureResult, try_run_on_failure
├── on_enqueue.rs  # Existing (no changes)
├── mod.rs         # Add on_failure cache, run_on_failure method
├── safety.rs      # Existing (shared safety, no changes)
└── ...
```

### Key Design Decisions

- on_failure script receives `error` as a field on the `msg` table (simplest API)
- `delay_ms` is accepted but not implemented (logs warning if > 0), future infrastructure needed
- DLQ routing requires `dlq_queue_id` to be set on the queue config (Story 3.4 will auto-create DLQs; for now, it must be manually configured)
- Separate circuit breaker tracking for on_failure vs on_enqueue (they are independent scripts)
- The `on_failure_script` field already exists in `QueueConfig` but is not yet wired

### on_failure Lua Script API

Input:
```lua
msg = {
    headers = <table>,     -- read-only string key-value pairs
    id = <string>,         -- message UUID
    attempts = <number>,   -- current attempt count (already incremented)
    queue = <string>,      -- queue name
    error = <string>,      -- error string from consumer nack
}
```

Output:
```lua
{
    action = "retry" | "dlq",  -- required, defaults to "retry"
    delay_ms = <number>,       -- optional, logged as warning if > 0
}
```
