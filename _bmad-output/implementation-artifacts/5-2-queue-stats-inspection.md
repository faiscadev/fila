# Story 5.2: Queue Stats & Inspection

Status: ready-for-dev

## Story

As an operator,
I want to view queue stats including depth, per-key throughput, and scheduling state,
so that I can understand the health and behavior of my message infrastructure.

## Acceptance Criteria

1. **Given** a queue has messages and active consumers, **when** an operator calls `GetStats` RPC for that queue, **then** the response includes: total message count (queue depth), number of in-flight leases, number of active consumers for this queue
2. **Given** a queue has active fairness keys, **when** `GetStats` is called, **then** per-fairness-key stats are included: pending message count, current deficit, weight
3. **Given** throttle keys are configured, **when** `GetStats` is called, **then** per-throttle-key state is included: current token count, rate per second, burst size
4. **Given** a queue has DRR scheduling state, **when** `GetStats` is called, **then** DRR state is included: quantum, active keys count
5. **Given** a non-existent queue ID, **when** `GetStats` is called, **then** `NOT_FOUND` status is returned
6. **Given** an integration test that enqueues messages across multiple fairness keys, leases some, and configures throttle rates, **when** `GetStats` is called, **then** accurate counts are verified for depth, in-flight, per-key pending, and throttle state

## Tasks / Subtasks

- [ ] Task 1: Expand `GetStats` proto definitions (AC: #1, #2, #3, #4)
  - [ ] Subtask 1.1: Add `PerFairnessKeyStats` message with `key`, `pending_count`, `current_deficit`, `weight`
  - [ ] Subtask 1.2: Add `PerThrottleKeyStats` message with `key`, `tokens`, `rate_per_second`, `burst`
  - [ ] Subtask 1.3: Expand `GetStatsResponse` with `active_consumers`, `quantum`, `per_key_stats`, `per_throttle_stats`
  - [ ] Subtask 1.4: Verify generated code compiles (`cargo build -p fila-proto`)
- [ ] Task 2: Add `StatsError` type and `IntoStatus` mapping (AC: #5)
  - [ ] Subtask 2.1: Add `StatsError { QueueNotFound(String), Storage(StorageError) }` to `error.rs`
  - [ ] Subtask 2.2: Add `IntoStatus` impl mapping `QueueNotFound` → `NOT_FOUND`, `Storage` → `INTERNAL`
- [ ] Task 3: Add `GetStats` scheduler command (AC: #1)
  - [ ] Subtask 3.1: Define `QueueStats` struct in a new `crates/fila-core/src/broker/stats.rs` module
  - [ ] Subtask 3.2: Add `GetStats { queue_id: String, reply: oneshot::Sender<Result<QueueStats, StatsError>> }` to `SchedulerCommand`
  - [ ] Subtask 3.3: Add match arm in scheduler dispatch loop
- [ ] Task 4: Add accessor methods to DRR and ThrottleManager (AC: #2, #3)
  - [ ] Subtask 4.1: Add `DrrScheduler::key_stats(queue_id) -> Vec<(String, i64, u32)>` returning `(key, deficit, weight)` tuples
  - [ ] Subtask 4.2: Add `DrrScheduler::quantum() -> u32` getter
  - [ ] Subtask 4.3: Add `ThrottleManager::key_stats() -> Vec<(String, f64, f64, f64)>` returning `(key, tokens, rate, burst)` tuples
- [ ] Task 5: Implement `handle_get_stats` in scheduler (AC: #1, #2, #3, #4, #5)
  - [ ] Subtask 5.1: Verify queue exists via `self.storage.get_queue(queue_id)`
  - [ ] Subtask 5.2: Count depth from `self.pending` entries for this queue + leased count
  - [ ] Subtask 5.3: Count in-flight from `self.leased_msg_keys` (scan for queue prefix in stored keys)
  - [ ] Subtask 5.4: Count active consumers from `self.consumers` for this queue
  - [ ] Subtask 5.5: Collect per-fairness-key stats from `self.pending` and DRR accessor
  - [ ] Subtask 5.6: Collect throttle stats from ThrottleManager accessor
  - [ ] Subtask 5.7: Collect DRR scheduling state (quantum, active keys count)
- [ ] Task 6: Implement `get_stats` in admin service (AC: #1, #5)
  - [ ] Subtask 6.1: Validate queue ID not empty
  - [ ] Subtask 6.2: Send `GetStats` command to scheduler via broker, await reply
  - [ ] Subtask 6.3: Map `QueueStats` to `GetStatsResponse`
- [ ] Task 7: Scheduler unit tests (AC: #1, #2, #3, #4, #5)
  - [ ] Subtask 7.1: Test get_stats with enqueued and leased messages returns correct depth and in-flight
  - [ ] Subtask 7.2: Test get_stats returns per-fairness-key pending counts
  - [ ] Subtask 7.3: Test get_stats returns throttle key state after SetConfig
  - [ ] Subtask 7.4: Test get_stats for non-existent queue returns QueueNotFound
  - [ ] Subtask 7.5: Test get_stats for empty queue returns zero counts (not error)
- [ ] Task 8: Admin service unit tests (AC: #1, #5)
  - [ ] Subtask 8.1: Test get_stats returns populated response for queue with messages
  - [ ] Subtask 8.2: Test get_stats with empty queue ID returns InvalidArgument
  - [ ] Subtask 8.3: Test get_stats for non-existent queue returns NotFound
- [ ] Task 9: Integration test — GetStats with multi-key messages and leases (AC: #6)
  - [ ] Subtask 9.1: Create queue, enqueue messages across 3 fairness keys with throttle keys
  - [ ] Subtask 9.2: Lease some messages (creating in-flight state)
  - [ ] Subtask 9.3: Set throttle rate via SetConfig
  - [ ] Subtask 9.4: Call GetStats and verify depth, in-flight, per-key pending counts, throttle state

## Dev Notes

### What Already Exists

- `GetStats` RPC stub in admin.proto with `GetStatsRequest { queue }` and basic `GetStatsResponse { depth, in_flight, active_fairness_keys }` — needs expansion
- `get_stats` method in admin_service.rs returns `Status::unimplemented("GetStats not yet implemented")` — replace with real implementation
- `DrrScheduler` public API: `has_active_keys()`, `active_keys()`, `queue_ids()` — needs new `key_stats()` and `quantum()` accessors
- `ThrottleManager` public API: `has_key()`, `len()`, `is_empty()` — needs new `key_stats()` accessor
- `Scheduler.pending: HashMap<(String, String), VecDeque<PendingEntry>>` — key is `(queue_id, fairness_key)`, iterate to get per-queue/per-key counts
- `Scheduler.leased_msg_keys: HashMap<Uuid, Vec<u8>>` — storage keys encode queue_id, need to decode for per-queue count
- `Scheduler.consumers: HashMap<String, ConsumerEntry>` — `ConsumerEntry.queue_id` field, filter by queue to count

### Counting In-Flight Messages

`leased_msg_keys` maps `msg_id → storage_key`. The storage key encodes the queue_id. To count in-flight per queue, iterate and decode the queue_id from each storage key. Look at `crate::storage::keys` for the key format.

Alternative: scan `self.pending` for the queue and subtract from total depth. But the pending index only tracks *pending* (not leased) messages, so the simpler approach is: `depth = pending_count + in_flight_count` where pending comes from the index and in-flight comes from scanning leased_msg_keys.

Actually, a cleaner approach: add a `leased_count: HashMap<String, usize>` field to Scheduler that increments on lease and decrements on ack/nack/expiry. This avoids scanning leased_msg_keys entirely. Review both approaches and pick the simpler one.

### Proto Design

Expand the existing proto definitions in `admin.proto`:

```protobuf
message PerFairnessKeyStats {
  string key = 1;
  uint64 pending_count = 2;
  int64 current_deficit = 3;
  uint32 weight = 4;
}

message PerThrottleKeyStats {
  string key = 1;
  double tokens = 2;
  double rate_per_second = 3;
  double burst = 4;
}

message GetStatsResponse {
  uint64 depth = 1;
  uint64 in_flight = 2;
  uint64 active_fairness_keys = 3;
  uint32 active_consumers = 4;
  uint32 quantum = 5;
  repeated PerFairnessKeyStats per_key_stats = 6;
  repeated PerThrottleKeyStats per_throttle_stats = 7;
}
```

Note: `per_throttle_stats` are global (not per-queue) since throttle keys span queues. Include them in the response for operator convenience.

### QueueStats Domain Type

Create `crates/fila-core/src/broker/stats.rs`:

```rust
pub struct FairnessKeyStats {
    pub key: String,
    pub pending_count: u64,
    pub current_deficit: i64,
    pub weight: u32,
}

pub struct ThrottleKeyStats {
    pub key: String,
    pub tokens: f64,
    pub rate_per_second: f64,
    pub burst: f64,
}

pub struct QueueStats {
    pub depth: u64,
    pub in_flight: u64,
    pub active_fairness_keys: u64,
    pub active_consumers: u32,
    pub quantum: u32,
    pub per_key_stats: Vec<FairnessKeyStats>,
    pub per_throttle_stats: Vec<ThrottleKeyStats>,
}
```

### New Accessor Methods Needed

**DrrScheduler** (`crates/fila-core/src/broker/drr.rs`):
```rust
/// Return stats for all active keys in a queue: (key, deficit, weight).
pub fn key_stats(&self, queue_id: &str) -> Vec<(String, i64, u32)> {
    // iterate active_keys, look up deficit and weight for each
}

/// Return the quantum value.
pub fn quantum(&self) -> u32 {
    self.quantum
}
```

**ThrottleManager** (`crates/fila-core/src/broker/throttle.rs`):
```rust
/// Return stats for all throttle keys: (key, tokens, rate, burst).
pub fn key_stats(&self) -> Vec<(String, f64, f64, f64)> {
    // iterate buckets, collect stats
}
```

TokenBucket needs `rate_per_second()` and `burst()` getters (currently fields are private).

### Error Handling

Create `StatsError` following per-command pattern:
```rust
#[derive(Debug, thiserror::Error)]
pub enum StatsError {
    #[error("queue not found: {0}")]
    QueueNotFound(String),

    #[error(transparent)]
    Storage(#[from] StorageError),
}
```

Map `QueueNotFound` → `Status::not_found`, `Storage` → `Status::internal` in `IntoStatus`.

### Routing Pattern

Same as all config operations — route through scheduler (single-writer guarantee, single-reader of in-memory state):

```
gRPC GetStats(queue) → AdminService → SchedulerCommand::GetStats → Scheduler:
  1. Verify queue exists
  2. Count pending messages from self.pending
  3. Count in-flight from self.leased_msg_keys (or tracked counter)
  4. Count consumers from self.consumers
  5. Collect DRR stats from self.drr
  6. Collect throttle stats from self.throttle
  7. Reply with QueueStats
```

### Testing Patterns

- **Scheduler tests:** use `test_setup()` helper. Enqueue messages, register consumers, lease messages, then call GetStats.
- **Admin service tests:** use `test_admin_service()` helper.
- **Integration test:** Full flow: create queue → set throttle → enqueue messages with different fairness keys → lease some → call GetStats → verify all counts.

### Key Files to Modify

- `proto/fila/v1/admin.proto` — expand GetStats messages
- `crates/fila-core/src/error.rs` — add StatsError
- `crates/fila-server/src/error.rs` — add IntoStatus for StatsError
- `crates/fila-core/src/broker/command.rs` — add GetStats variant
- `crates/fila-core/src/broker/stats.rs` — new file: QueueStats domain types
- `crates/fila-core/src/broker/mod.rs` — pub mod stats
- `crates/fila-core/src/broker/drr.rs` — add key_stats(), quantum() accessors
- `crates/fila-core/src/broker/throttle.rs` — add key_stats() accessor, TokenBucket getters
- `crates/fila-core/src/broker/scheduler.rs` — add handle_get_stats, dispatch arm, tests
- `crates/fila-server/src/admin_service.rs` — implement get_stats handler, tests

### References

- [Source: proto/fila/v1/admin.proto:65-73] Current GetStats stub messages
- [Source: crates/fila-server/src/admin_service.rs:231-236] GetStats unimplemented stub
- [Source: crates/fila-core/src/broker/scheduler.rs:36-51] Scheduler state fields (pending, leased_msg_keys, consumers)
- [Source: crates/fila-core/src/broker/drr.rs:28-31] DrrScheduler struct with queues + quantum
- [Source: crates/fila-core/src/broker/drr.rs:163-189] DRR public inspection methods
- [Source: crates/fila-core/src/broker/throttle.rs:90-175] ThrottleManager public API
- [Source: crates/fila-core/src/broker/throttle.rs:10-83] TokenBucket struct and methods
- [Source: crates/fila-core/src/error.rs:75-81] DeleteQueueError pattern for StatsError
- [Source: _bmad-output/implementation-artifacts/5-1-configuration-listing-operator-visibility.md] Story 5.1 spec (pattern reference)

## Dev Agent Record

### Agent Model Used

Claude Opus 4.6

### Debug Log References

### Completion Notes List

### File List
