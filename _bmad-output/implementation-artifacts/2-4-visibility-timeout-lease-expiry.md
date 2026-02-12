# Story 2.4: Visibility Timeout & Lease Expiry

Status: review

## Story

As an operator,
I want messages with expired visibility timeouts to automatically become available again,
so that stuck or crashed consumers don't block message processing.

## Acceptance Criteria

1. The scheduler checks for expired leases on every loop iteration (idle timeout wakeup)
2. When a lease expires, the message re-enters the ready pool for its fairness key with attempt_count incremented
3. Expired lease and lease_expiry entries are removed atomically via WriteBatch
4. The message's `leased_at` is cleared when its lease expires
5. An integration test verifies: enqueue → lease → wait for expiry → message is re-leased with incremented attempt_count
6. Recovery on startup also increments attempt_count (currently it only deletes lease entries)
7. Lease expiry check is efficient: scans `lease_expiry` CF from earliest, stops at first non-expired entry

## Tasks / Subtasks

- [x] Task 1: Extract `reclaim_expired_leases()` method (AC: #1, #2, #3, #4, #7)
  - [x] 1.1 Extract shared logic from `recover()` into `reclaim_expired_leases()` that: scans lease_expiry CF, for each expired entry: deletes lease + lease_expiry, retrieves message, increments attempt_count, clears leased_at, writes updated message, re-adds fairness key to DRR active set
  - [x] 1.2 Update `recover()` to call `reclaim_expired_leases()` instead of inline lease scanning
  - [x] 1.3 All existing recovery tests must still pass

- [x] Task 2: Wire periodic expiry check into scheduler loop (AC: #1)
  - [x] 2.1 Call `reclaim_expired_leases()` at the idle timeout wakeup point (replace the comment)
  - [x] 2.2 Call `drr_deliver()` after reclaim so re-queued messages are delivered immediately

- [x] Task 3: Fix recovery to increment attempt_count (AC: #6)
  - [x] 3.1 Since `reclaim_expired_leases()` now handles attempt_count, this is covered by Task 1
  - [x] 3.2 Update `recovery_reclaims_expired_leases` test to verify attempt_count is incremented

- [x] Task 4: Integration tests (AC: #5)
  - [x] 4.1 `lease_expiry_redelivers_message_with_incremented_attempt_count` — enqueue, lease, wait for visibility timeout, verify message reappears with attempt_count=1
  - [x] 4.2 `lease_expiry_clears_lease_and_expiry_entries` — verify leases and lease_expiry CFs are clean after expiry reclaim
  - [x] 4.3 `lease_expiry_multiple_messages_different_timeouts` — two messages with different queue visibility timeouts, verify correct expiry ordering
  - [x] 4.4 `ack_before_expiry_prevents_redelivery` — ack within timeout, verify message is not redelivered

## Dev Notes

### Architecture Pattern

The scheduler loop (scheduler.rs:55-100) has three phases:
1. **Phase 1:** Drain all buffered commands (non-blocking `try_recv`)
2. **Phase 2:** DRR delivery round (`drr_deliver()`)
3. **Phase 3:** Park until next command or idle timeout (`recv_timeout`)

The idle timeout wakeup (Phase 3, line 83-85) has a placeholder comment: `"// Normal idle wakeup — future stories add periodic work here"`. This is where `reclaim_expired_leases()` should be called.

**Important:** After reclaiming expired leases, call `drr_deliver()` to immediately attempt delivery of the re-queued messages. The loop already calls `drr_deliver()` after Phase 1, but expired leases reclaimed during idle wakeup need their own delivery pass.

### Key Implementation Details

**`reclaim_expired_leases()` pattern — follows `handle_nack()` closely:**

The nack handler (scheduler.rs:278-342) already does exactly what lease expiry needs per-message:
- Look up lease → parse expiry → find message key → retrieve message
- Increment attempt_count, clear leased_at
- WriteBatch: update message + delete lease + delete lease_expiry
- Re-add fairness key to DRR active set

The difference is that `reclaim_expired_leases()` operates in batch:
1. Scan `lease_expiry` CF with `list_expired_leases(up_to_key)` where `up_to_key` = current timestamp + 0xFF padding
2. For each expired key, parse `(queue_id, msg_id)` via `parse_lease_expiry_key()`
3. For each expired message, do the nack-like update (increment attempt_count, clear leased_at, re-add to DRR)
4. Skip corrupt keys with a warning (same as current recovery)

**WriteBatch per message vs. single batch:** Use one WriteBatch per expired message (not one giant batch for all). This matches the nack pattern and limits blast radius if one message's data is corrupt.

### Existing Code to Reuse

- `storage::keys::parse_lease_expiry_key()` — extracts (queue_id, msg_id) from lease_expiry key
- `storage::keys::lease_key()` — constructs lease key from queue_id + msg_id
- `storage.list_expired_leases(up_to)` — range scan on lease_expiry CF
- `find_message_key()` — O(n) prefix scan to find full message key (pre-existing, acceptable for now)
- `storage.get_message()` / `storage.write_batch()` — read and write message data
- `drr.add_key()` — re-add fairness key to DRR active set

### Testing Strategy

**Challenge:** Visibility timeouts are wall-clock based. Tests need short timeouts to avoid being slow.

**Approach:** Use a queue with `visibility_timeout_ms = 50` (50ms). The scheduler's `idle_timeout_ms` is 10ms in tests, so it will wake up and check for expired leases every 10ms. After the 50ms visibility timeout, the next idle wakeup will reclaim the lease.

**Test flow for 4.1:**
1. Create queue with `visibility_timeout_ms = 50`
2. Register consumer, enqueue message
3. Run scheduler for enough ticks to deliver + expire + redeliver
4. Collect both deliveries from consumer channel
5. Verify second delivery has attempt_count = 1

**Important test detail:** The scheduler runs synchronously in tests via `scheduler.run()` which blocks until Shutdown. For time-based tests, you need the scheduler running on a background thread (like the 10k fairness test in Story 2.2) and use `std::thread::sleep()` to wait for expiry.

### File Targets

- `crates/fila-core/src/broker/scheduler.rs` — extract `reclaim_expired_leases()`, wire into loop, update recovery, add tests
- No changes needed in gRPC layer, error types, or storage layer

### Previous Story Learnings (from 2.3)

- Unregister consumer before checking lease state in tests where you don't want immediate redelivery
- `drr_deliver_queue` called after nack creates new leases — same will happen after reclaim, which is the desired behavior
- Guard `drr_deliver` calls on success (don't deliver after failed reclaim)

### References

- [Source: architecture.md — Scheduler Core Loop, lines 199-235]
- [Source: architecture.md — Data Architecture — RocksDB, lines 238-277]
- [Source: scheduler.rs:480-552 — existing recover() method]
- [Source: scheduler.rs:278-342 — handle_nack() pattern to follow]
- [Source: scheduler.rs:55-100 — main scheduler loop with idle timeout]
- [Source: queue.rs — QueueConfig with visibility_timeout_ms field]

## Dev Agent Record

### Agent Model Used

Claude Opus 4.6

### Debug Log References

- No issues encountered — all tests passed on first run after fixing mut bindings

### Completion Notes List

- `reclaim_expired_leases()` extracts and enhances the recovery lease reclaim logic to also increment attempt_count, clear leased_at, and re-add fairness key to DRR
- `recover()` now delegates to `reclaim_expired_leases()` for lease cleanup, then rebuilds DRR from messages
- Scheduler loop calls `reclaim_expired_leases()` on every idle timeout wakeup, followed by `drr_deliver()` if any leases were reclaimed
- Orphaned lease_expiry entries (message not found) are cleaned up gracefully with a warning
- Tests use threaded scheduler with short visibility timeouts (50ms) to test wall-clock expiry behavior

### File List

- `crates/fila-core/src/broker/scheduler.rs` — extracted `reclaim_expired_leases()`, wired into scheduler loop, updated recovery, added 4 integration tests + 1 assertion to existing recovery test
