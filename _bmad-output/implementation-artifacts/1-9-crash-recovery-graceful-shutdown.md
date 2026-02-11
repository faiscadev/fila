# Story 1.9: Crash Recovery & Graceful Shutdown

Status: in-progress

## Story

As an operator,
I want the broker to recover all state after a crash with zero message loss,
So that I can trust the system to be durable without manual intervention.

## Acceptance Criteria

1. Given the broker has messages, leases, and queue definitions persisted in RocksDB, when the broker process is killed and restarted, all queue definitions are loaded from the `queues` CF
2. The scheduler rebuilds in-memory state by scanning the `messages` CF (active keys, pending message counts)
3. In-flight leases are restored from the `leases` CF
4. Already-expired leases (identified by scanning `lease_expiry` CF) are reclaimed immediately — messages re-enter the ready pool
5. The broker is ready to accept gRPC connections after recovery completes
6. No messages are lost during the recovery process
7. Graceful shutdown flushes the RocksDB WAL before exit
8. An integration test enqueues messages, kills the broker process, restarts, and verifies all messages are still available for leasing

## Tasks / Subtasks

- [x] Task 1: Add lease expiry key parsing (AC: #4)
  - [x] 1.1 Add `parse_lease_expiry_key()` to keys.rs to extract (queue_id, msg_id) from expiry keys
- [x] Task 2: Add flush to Storage trait (AC: #7)
  - [x] 2.1 Add `fn flush(&self) -> Result<()>` to Storage trait
  - [x] 2.2 Implement `flush()` in RocksDbStorage using `flush_wal(true)`
- [x] Task 3: Implement crash recovery in scheduler (AC: #1, #2, #3, #4, #5, #6)
  - [x] 3.1 Add `recover()` method that scans lease_expiry CF for expired leases
  - [x] 3.2 Parse expired lease_expiry keys and atomically delete lease + lease_expiry entries
  - [x] 3.3 Call `self.recover()` at the start of `Scheduler::run()`
  - [x] 3.4 Log recovery summary (reclaimed leases, queue count)
- [x] Task 4: Implement graceful shutdown WAL flush (AC: #7)
  - [x] 4.1 Call `self.storage.flush()` at the end of `Scheduler::run()` before exit
- [x] Task 5: Tests (AC: #4, #6, #7, #8)
  - [x] 5.1 `recovery_preserves_messages_after_restart`: enqueue 5 messages, restart scheduler, verify all available
  - [x] 5.2 `recovery_reclaims_expired_leases`: create expired lease, restart, verify reclaimed and message delivered
  - [x] 5.3 `recovery_preserves_queue_definitions`: create 3 queues, restart, verify all present
  - [x] 5.4 `shutdown_flushes_wal`: enqueue, shutdown, reopen storage, verify data survived
