# Story 1.4: Broker Core & Scheduler Loop

Status: ready-for-dev

## Story

As a developer,
I want the single-threaded scheduler core with channel-based communication,
So that all state mutations happen on a dedicated thread without lock contention.

## Acceptance Criteria

1. A `Broker` struct spawns a dedicated `std::thread` for the scheduler core
2. `crossbeam-channel` bounded channels connect IO threads (inbound) to the scheduler (outbound)
3. A `SchedulerCommand` enum defines all commands: `Enqueue`, `Ack`, `Nack`, `RegisterConsumer`, `UnregisterConsumer`, `Shutdown`
4. Each command variant includes a `oneshot::Sender` for request-response patterns (where applicable)
5. The scheduler core runs a tight event loop: drain inbound commands (non-blocking `try_recv`), then park with timeout if idle
6. The scheduler loop contains no async code and never blocks on IO
7. The `Broker` provides a public API for sending commands that internally sends via the crossbeam channel
8. `BrokerConfig` struct deserializes from TOML with sections for server, scheduler, lua, and telemetry
9. Graceful shutdown is supported: signal → stop accepting commands → drain in-flight → flush RocksDB WAL → exit
10. A `tracing` subscriber is configured for structured stdout logging (JSON in release, pretty-print in debug)
11. All public functions in the broker module use `#[tracing::instrument]` or manual spans
12. Unit tests verify the scheduler processes commands in order

## Tasks / Subtasks

- [ ] Task 1: Add workspace dependencies (AC: #2, #8, #10)
  - [ ] 1.1 Add `crossbeam-channel = "0.5"` to fila-core deps (already in workspace)
  - [ ] 1.2 Add `tracing = "0.1"` to fila-core deps (already in workspace)
  - [ ] 1.3 Add `tracing-subscriber` to workspace deps and fila-core deps (with `fmt` and `json` features)
  - [ ] 1.4 Add `toml` to workspace deps and fila-core deps for config deserialization
  - [ ] 1.5 Add `tokio` to fila-core dev-deps (for oneshot channels in tests)
- [ ] Task 2: Define SchedulerCommand and related types (AC: #3, #4)
  - [ ] 2.1 Create `crates/fila-core/src/broker/command.rs`
  - [ ] 2.2 Define `SchedulerCommand` enum with variants: `Enqueue`, `Ack`, `Nack`, `RegisterConsumer`, `UnregisterConsumer`, `Shutdown`
  - [ ] 2.3 `Enqueue` variant: `{ message: Message, reply: oneshot::Sender<Result<uuid::Uuid>> }` — returns assigned message ID
  - [ ] 2.4 `Ack` variant: `{ queue_id: String, msg_id: uuid::Uuid, reply: oneshot::Sender<Result<()>> }`
  - [ ] 2.5 `Nack` variant: `{ queue_id: String, msg_id: uuid::Uuid, error: String, reply: oneshot::Sender<Result<()>> }`
  - [ ] 2.6 `RegisterConsumer` variant: `{ queue_id: String, consumer_id: String, tx: crossbeam_channel::Sender<ReadyMessage> }` — no reply needed, fire-and-forget registration
  - [ ] 2.7 `UnregisterConsumer` variant: `{ consumer_id: String }` — fire-and-forget
  - [ ] 2.8 `Shutdown` variant: no fields, signals graceful drain
  - [ ] 2.9 Define `ReadyMessage` struct: `{ msg_id: uuid::Uuid, queue_id: String, headers: HashMap<String, String>, payload: Vec<u8>, fairness_key: String, attempt_count: u32 }`
- [ ] Task 3: Define BrokerConfig (AC: #8)
  - [ ] 3.1 Create `crates/fila-core/src/broker/config.rs`
  - [ ] 3.2 Define `BrokerConfig` with nested sections: `server: ServerConfig`, `scheduler: SchedulerConfig`
  - [ ] 3.3 `ServerConfig`: `listen_addr: String` (default `"0.0.0.0:5555"`)
  - [ ] 3.4 `SchedulerConfig`: `command_channel_capacity: usize` (default 10_000), `idle_timeout_ms: u64` (default 100)
  - [ ] 3.5 Derive `serde::Deserialize` for TOML parsing
  - [ ] 3.6 Implement `Default` for `BrokerConfig` with sensible defaults
- [ ] Task 4: Implement Scheduler core loop (AC: #1, #5, #6)
  - [ ] 4.1 Create `crates/fila-core/src/broker/scheduler.rs`
  - [ ] 4.2 Define `Scheduler` struct owning: `storage: Arc<dyn Storage>`, `inbound: crossbeam_channel::Receiver<SchedulerCommand>`, `config: SchedulerConfig`
  - [ ] 4.3 Implement `Scheduler::run(&mut self)` with the event loop: drain commands via `try_recv`, park with `recv_timeout` when idle
  - [ ] 4.4 Implement `handle_command(&mut self, cmd: SchedulerCommand)` with match on each variant
  - [ ] 4.5 For this story, command handlers should be minimal stubs that reply with success (full logic in stories 1.5-1.8)
  - [ ] 4.6 Handle `Shutdown` command: set a `running` flag to false, break the loop
  - [ ] 4.7 Ensure the loop processes ALL buffered commands before checking idle/parking
- [ ] Task 5: Implement Broker struct (AC: #1, #2, #7, #9)
  - [ ] 5.1 Create `crates/fila-core/src/broker/mod.rs`
  - [ ] 5.2 Define `Broker` struct owning: `command_tx: crossbeam_channel::Sender<SchedulerCommand>`, `scheduler_thread: Option<std::thread::JoinHandle<()>>`
  - [ ] 5.3 Implement `Broker::new(config: BrokerConfig, storage: Arc<dyn Storage>) -> Result<Self>` — creates the channel, spawns the scheduler thread
  - [ ] 5.4 Implement `Broker::send_command(&self, cmd: SchedulerCommand) -> Result<()>` — sends on the crossbeam channel
  - [ ] 5.5 Implement `Broker::shutdown(self) -> Result<()>` — sends Shutdown command, joins the scheduler thread
  - [ ] 5.6 Name the scheduler thread `"fila-scheduler"` via `std::thread::Builder::new().name(...)`
- [ ] Task 6: Set up tracing (AC: #10, #11)
  - [ ] 6.1 Create `crates/fila-core/src/telemetry.rs`
  - [ ] 6.2 Implement `init_tracing()` that configures a `tracing_subscriber::fmt` subscriber
  - [ ] 6.3 Use JSON format in release builds, pretty-print in debug builds (feature-gated or env-based)
  - [ ] 6.4 Add `#[tracing::instrument]` to all public `Broker` methods
- [ ] Task 7: Update module declarations (AC: all)
  - [ ] 7.1 Add `pub mod broker;` and `pub mod telemetry;` to `crates/fila-core/src/lib.rs`
  - [ ] 7.2 Re-export key types: `Broker`, `BrokerConfig`, `SchedulerCommand`, `ReadyMessage`
- [ ] Task 8: Write unit tests (AC: #12)
  - [ ] 8.1 Test scheduler processes commands in FIFO order
  - [ ] 8.2 Test Broker::new spawns a thread and Broker::shutdown joins it cleanly
  - [ ] 8.3 Test Shutdown command causes scheduler loop to exit
  - [ ] 8.4 Test command reply is received via oneshot channel
  - [ ] 8.5 Test BrokerConfig defaults and TOML parsing
- [ ] Task 9: Verify build (AC: all)
  - [ ] 9.1 Run `cargo build` — all crates compile
  - [ ] 9.2 Run `cargo clippy -- -D warnings` — no warnings
  - [ ] 9.3 Run `cargo fmt --check` — formatted
  - [ ] 9.4 Run `cargo nextest run` — all tests pass

## Dev Notes

### Concurrency Architecture (from Architecture doc)

**Redis-inspired single-threaded scheduler core:**
- IO threads: tokio multi-threaded runtime for tonic gRPC server
- Scheduler thread: Single dedicated `std::thread`. Owns all mutable scheduler state. Runs a tight event loop. Never blocks on IO.
- Communication: `crossbeam-channel` for lock-free, bounded MPMC between IO and scheduler. `oneshot` channels for request-response.

**Scheduler Core Loop (pseudocode from Architecture doc):**
```
loop {
    // 1. Drain inbound commands (non-blocking)
    while let Ok(cmd) = inbound.try_recv() {
        handle_command(cmd);
    }

    // 2. Check expired visibility timeouts (story 2.4)
    // reclaim_expired_leases();

    // 3. Refill token buckets (story 4.1)
    // refill_token_buckets(now);

    // 4. Run DRR scheduling round (story 2.1)
    // (not implemented in this story)

    // 5. Reset deficits when round completes (story 2.1)
    // (not implemented in this story)

    // 6. Park if idle (with timeout for lease expiry checking)
    if nothing_to_do { recv_timeout(idle_timeout); }
}
```

For **this story**, the scheduler loop only drains commands and handles them with stub responses. Steps 2-5 are wired in by later stories. The park behavior uses `crossbeam_channel::Receiver::recv_timeout` to block until the next command or timeout.

### Channel Design

```rust
// Inbound: IO threads → Scheduler (crossbeam bounded MPMC)
let (cmd_tx, cmd_rx) = crossbeam_channel::bounded::<SchedulerCommand>(capacity);

// Reply: Scheduler → specific IO thread (tokio oneshot per command)
// Each command variant carries its own oneshot::Sender
```

**Why crossbeam for inbound, tokio oneshot for reply:**
- Inbound: crossbeam because the scheduler thread is `std::thread`, not async. `try_recv()` is non-blocking and never touches the async runtime.
- Reply: tokio oneshot because the gRPC handlers are async and need to `.await` the response. `tokio::sync::oneshot` is safe to use from both sync and async code (the sender side doesn't require a runtime).

### Storage Trait Must Be `Arc<dyn Storage>`

The Storage trait from story 1.3 is `Send + Sync`. The scheduler thread needs access, so wrap in `Arc<dyn Storage>`. The `Broker` and `Scheduler` both hold an `Arc<dyn Storage>`.

### Graceful Shutdown Flow

1. External signal (SIGTERM/SIGINT) → caller sends `SchedulerCommand::Shutdown` via `Broker::shutdown()`
2. Scheduler receives `Shutdown` → sets `running = false`
3. Scheduler loop breaks after draining remaining buffered commands
4. Scheduler drops Storage (which flushes RocksDB WAL via Drop)
5. `Broker::shutdown()` calls `join()` on the scheduler thread handle
6. Broker drops the channel sender, ensuring no more commands can be sent

### Tracing Configuration

Use `tracing-subscriber` with layered setup:
- `fmt::Layer` for console output
- JSON format detection: check `cfg!(debug_assertions)` — pretty in debug, JSON in release
- Filter level from `RUST_LOG` env var with default `info`

### BrokerConfig TOML Format

```toml
[server]
listen_addr = "0.0.0.0:5555"

[scheduler]
command_channel_capacity = 10000
idle_timeout_ms = 100
```

All fields should have defaults so an empty config file works.

### Important: What This Story Does NOT Implement

- DRR scheduling (story 2.1)
- Lease expiry checking (story 2.4)
- Token bucket refills (story 4.1)
- Actual enqueue/ack/nack logic (stories 1.5-1.8)
- Consumer message delivery (story 1.7)
- Lua hooks (story 3.1)

Command handlers should be **stubs** that log receipt and reply with success (or `Ok(())`). This establishes the skeleton that later stories fill in.

### Dependencies to Add

**Workspace Cargo.toml additions:**
```toml
tracing-subscriber = { version = "0.3", features = ["fmt", "json", "env-filter"] }
toml = "0.8"
```

**fila-core Cargo.toml additions:**
```toml
[dependencies]
crossbeam-channel.workspace = true
tracing.workspace = true
tracing-subscriber.workspace = true
toml.workspace = true

[dev-dependencies]
tempfile = "3"
tokio = { workspace = true, features = ["sync"] }  # for oneshot in tests
```

### Previous Story Intelligence

**Story 1.3 learnings:**
- `Storage` trait is `Send + Sync`, defined in `crates/fila-core/src/storage/traits.rs`
- `RocksDbStorage` implements `Storage`, opened via `RocksDbStorage::open(path)`
- `FilaError` defined in `crates/fila-core/src/error.rs` — add new variants if needed
- Key types re-exported from `crates/fila-core/src/lib.rs`
- Use `#[allow(dead_code)]` on modules whose public API isn't used yet (like keys module)
- `uuid` has `serde` feature enabled
- `u16::try_from` for safe casts (Cubic review finding)

**Story 1.2 learnings:**
- CI runs on all PRs, not just those targeting main
- GitHub Actions pinned to commit SHAs

### Anti-Patterns to Avoid

- Do NOT use `Arc<Mutex<...>>` for scheduler state — the whole point is single-threaded
- Do NOT use async code in the scheduler loop — it's a sync `std::thread`
- Do NOT call `recv()` (blocking) in the command drain phase — use `try_recv()` for non-blocking drain, then `recv_timeout()` for parking
- Do NOT pass raw RocksDB handles between threads — all access through `Arc<dyn Storage>`
- Do NOT make the scheduler thread a tokio task — it must be a dedicated OS thread
- Do NOT use `unwrap()` in production code — use proper error handling
- Do NOT log message payloads (could contain PII)
- Do NOT skip `#[tracing::instrument]` on public functions

### Project Structure After This Story

```
crates/fila-core/
├── Cargo.toml
└── src/
    ├── lib.rs              # Module declarations + re-exports
    ├── error.rs            # FilaError enum
    ├── message.rs          # Message domain type
    ├── queue.rs            # QueueConfig type
    ├── telemetry.rs        # tracing subscriber init
    ├── broker/
    │   ├── mod.rs          # Broker struct + re-exports
    │   ├── command.rs      # SchedulerCommand, ReadyMessage
    │   ├── config.rs       # BrokerConfig
    │   └── scheduler.rs    # Scheduler event loop
    └── storage/
        ├── mod.rs          # Re-export Storage trait + RocksDbStorage
        ├── traits.rs       # Storage trait definition + WriteBatchOp
        ├── keys.rs         # Key encoding/decoding functions
        └── rocksdb.rs      # RocksDbStorage implementation
```

### References

- [Source: _bmad-output/planning-artifacts/architecture.md#Concurrency Architecture]
- [Source: _bmad-output/planning-artifacts/architecture.md#Communication Patterns]
- [Source: _bmad-output/planning-artifacts/architecture.md#Error Handling Strategy]
- [Source: _bmad-output/planning-artifacts/architecture.md#Enforcement Guidelines]
- [Source: _bmad-output/planning-artifacts/architecture.md#Process Patterns — Graceful Shutdown]
- [Source: _bmad-output/planning-artifacts/epics.md#Story 1.4: Broker Core & Scheduler Loop]

## Dev Agent Record

### Agent Model Used

{{agent_model_name_version}}

### Debug Log References

### Completion Notes List

### File List
