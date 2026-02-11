---
stepsCompleted: [1, 2, 3, 4, 5, 6, 7, 8]
inputDocuments:
  - '_bmad-output/planning-artifacts/prd.md'
  - '_bmad-output/planning-artifacts/product-brief-fila-2026-02-10.md'
  - '_bmad-output/brainstorming/brainstorming-session-2026-02-09.md'
workflowType: 'architecture'
project_name: 'fila'
user_name: 'Lucas'
date: '2026-02-10'
lastStep: 8
status: 'complete'
completedAt: '2026-02-10'
---

# Architecture Decision Document — Fila

_A Rust-based message broker where fair scheduling and per-key throttling are first-class primitives._

## Project Context Analysis

### Requirements Overview

**Functional Requirements:**

60 functional requirements across 10 categories:

| Category | FRs | Architectural Implication |
|----------|-----|--------------------------|
| Message Lifecycle (FR1–7) | Enqueue, Lease, Ack, Nack, visibility timeout, DLQ, redrive | Core message state machine; RocksDB persistence; at-least-once delivery |
| Fair Scheduling (FR8–12) | Fairness keys, DRR, weighted throughput, active keys, 5% accuracy | Single-threaded scheduler core; in-memory DRR state; per-key counters |
| Throttling (FR13–17) | Token buckets, multi-key, skip-throttled, runtime updates, hierarchical | In-memory token buckets; Lua assigns keys; API sets limits |
| Rules Engine (FR18–25) | Lua hooks, on_enqueue/on_failure, fila.get(), timeouts, circuit breaker | Embedded Lua VM (mlua); pre-compiled scripts; sandbox with limits |
| Runtime Config (FR26–29) | Key-value store, Lua-readable, no-restart updates | RocksDB state column family; gRPC admin API |
| Queue Management (FR30–34) | CRUD, auto-DLQ, stats, CLI | Admin gRPC RPCs; fila-cli binary |
| Observability (FR35–41) | OTel metrics, per-key throughput, throttle rates, DRR rounds, Lua histograms, tracing | tracing + opentelemetry crates; per-key metric labels |
| Client SDKs (FR42–48) | Go, Python, JS, Ruby, Rust, Java | Protobuf-first API; thin gRPC wrappers generated from .proto |
| Deployment (FR49–53) | Single binary, Docker, cargo install, curl install, crash recovery | Static linking; multi-stage Dockerfile; RocksDB WriteBatch atomicity |
| Documentation (FR54–60) | Docs, examples, tutorials, API ref, llms.txt | Generated from proto; example-driven docs |

**Non-Functional Requirements:**

21 NFRs across 4 categories driving architectural decisions:

| Category | Key NFRs | Architectural Driver |
|----------|----------|---------------------|
| Performance | 100k+ msg/s, <5% scheduling overhead, <50us Lua p99, <1ms lease latency | Single-threaded core eliminates lock contention; pre-compiled Lua; RocksDB key layout matches access patterns |
| Reliability | Zero message loss, atomic persistence, at-least-once delivery | RocksDB WriteBatch; visibility timeout; lease tracking |
| Operability | Single-engineer deploy, 10-minute onboarding, single binary, runtime config | No external dependencies; CLI-first admin; OTel built-in |
| Integration | OTel-compatible, gRPC best practices, protobuf backward compat, idiomatic SDKs | tonic + prost; proto-first API design; per-language SDK conventions |

**Scale & Complexity:**

- Primary domain: Systems infrastructure (message broker)
- Complexity level: High — novel scheduling primitive, embedded scripting engine, custom persistence layout, performance-critical hot path
- Estimated architectural components: 4 crates, 5 RocksDB column families, 3 gRPC service definitions, 2 Lua hook points

### Technical Constraints & Dependencies

- **Language**: Rust (non-negotiable — performance, safety, single-binary distribution)
- **Solo developer**: Architecture must be buildable incrementally by one person
- **Open-source**: AGPLv3 license; all dependencies must be AGPL-compatible
- **Single-node MVP**: Distribution is a future layer, not a constraint on MVP architecture
- **No external runtime dependencies**: Single binary, no Redis, no ZooKeeper, no JVM

### Cross-Cutting Concerns Identified

1. **Observability** — Every component emits structured traces and metrics from day one (OTel)
2. **Crash recovery** — All durable state must survive process kill; in-memory state rebuilt on restart
3. **Lua safety** — Scripts execute on the hot path; must be sandboxed with time/memory limits and circuit breaker
4. **Performance** — Lock-free communication between IO and scheduler; zero-copy where possible
5. **Testability** — Core library testable without gRPC; trait-based storage for test implementations

## Starter Template & Technology Foundation

### Primary Technology Domain

Systems infrastructure — standalone Rust binary (message broker). This is not a web/mobile/full-stack project. There is no traditional "starter template." The foundation is a Cargo workspace with purpose-built crate organization.

### Technology Stack

**Language & Runtime:**
- Rust (stable toolchain, MSRV to be pinned at project start)
- No nightly features required

**Core Dependencies (verify latest versions at project start):**

| Crate | Purpose | Role |
|-------|---------|------|
| `tokio` | Async runtime | gRPC IO threads, timers, signal handling |
| `tonic` | gRPC framework | Hot path + admin RPC server and client |
| `prost` | Protobuf codegen | Message serialization, API definitions |
| `rocksdb` | Embedded key-value store | Durable message and state persistence |
| `mlua` | Lua 5.4 bindings | Rules engine: on_enqueue, on_failure hooks |
| `crossbeam-channel` | Lock-free MPMC channels | IO thread ↔ scheduler core communication |
| `tracing` | Structured logging/tracing | Application-wide observability |
| `opentelemetry` + `opentelemetry-otlp` | OTel SDK | Metrics + trace export to collectors |
| `tracing-opentelemetry` | Bridge | Connect tracing spans to OTel |
| `clap` | CLI argument parsing | fila-server config, fila-cli commands |
| `serde` + `serde_json` | Serialization | Config files, runtime state |
| `thiserror` | Error derivation | Typed errors in fila-core |
| `bytes` | Byte buffer management | Zero-copy message payload handling |
| `uuid` | UUIDv7 generation | Message IDs — time-ordered, globally unique, no coordination needed |

**Build & Development:**

| Tool | Purpose |
|------|---------|
| `cargo` | Build, test, bench, publish |
| `cargo-nextest` | Faster test runner |
| `criterion` | Benchmarking framework |
| `proptest` | Property-based testing for scheduler invariants |
| `docker` | Container builds and local testing |
| `protoc` | Protobuf compiler (build dependency) |
| `buf` | Protobuf linting and breaking change detection |

**Configuration Format:** TOML for static broker config; gRPC API for runtime config.

### Initialization

```bash
cargo init --name fila
# Then restructure into workspace (see Project Structure section)
```

**Note:** Project initialization and workspace setup should be the first implementation task.

## Core Architectural Decisions

### Decision Priority Analysis

**Critical Decisions (Block Implementation):**

1. Concurrency model — single-threaded scheduler core + multi-threaded IO
2. Storage engine — RocksDB with column family isolation
3. Wire protocol — gRPC only (tonic)
4. Key encoding scheme — composite big-endian keys for RocksDB ordering
5. Crate organization — workspace with core/proto/server/cli separation

**Important Decisions (Shape Architecture):**

6. Lua integration model — per-queue pre-compiled scripts with sandbox limits
7. Channel architecture — crossbeam for scheduler, tokio channels for internal gRPC
8. Error handling strategy — thiserror in core, gRPC status codes at boundary
9. Configuration approach — TOML file + runtime key-value store
10. Testing strategy — unit + integration + property-based + benchmarks

**Deferred Decisions (Post-MVP):**

- Authentication mechanism (API keys vs mTLS vs both)
- Clustering and replication strategy
- Consumer group assignment algorithm
- REST API design (if demand materializes)
- Custom storage engine design

### Concurrency Architecture

**Decision:** Redis-inspired single-threaded scheduler core with lock-free IO channels.

**Rationale:** The scheduler makes fairness and throttle decisions that depend on shared mutable state (DRR deficits, token bucket counters, active keys set). A single-threaded core eliminates all lock contention on the hot path. Redis validates this model at millions of ops/sec.

**Architecture:**

```
Producers                                              Consumers
   │                                                       ▲
   │ gRPC Enqueue                              gRPC Lease  │
   ▼                                                       │
┌──────────────────────────────────────────────────────────────┐
│                    tonic gRPC IO Threads                      │
│                    (tokio multi-threaded runtime)             │
└──────────┬───────────────────────────────────────┬───────────┘
           │ crossbeam channel (inbound)           ▲ crossbeam channel (outbound)
           ▼                                       │
┌──────────────────────────────────────────────────────────────┐
│              Single-Threaded Scheduler Core                   │
│                   (dedicated OS thread)                       │
│                                                              │
│  ┌─────────────┐  ┌──────────────┐  ┌────────────────────┐  │
│  │     DRR     │  │    Token     │  │   Lua Sandbox      │  │
│  │  Scheduler  │  │   Buckets    │  │  (mlua, compiled)  │  │
│  └─────────────┘  └──────────────┘  └────────────────────┘  │
│                                                              │
│  ┌────────────────────────────────────────────────────────┐  │
│  │                   RocksDB Storage                      │  │
│  │   messages │ leases │ queues │ state                   │  │
│  └────────────────────────────────────────────────────────┘  │
└──────────────────────────────────────────────────────────────┘
```

**Thread model:**
- **IO threads**: tokio multi-threaded runtime runs the tonic gRPC server. Handles connection management, TLS (future), serialization/deserialization. Multiple threads.
- **Scheduler thread**: Single dedicated `std::thread`. Owns all mutable scheduler state. Runs a tight event loop. Never blocks on IO.
- **Communication**: `crossbeam-channel` for lock-free, bounded MPMC between IO and scheduler. `oneshot` channels for request-response patterns.

**Scheduler Core Loop (pseudocode):**

```
loop {
    // 1. Drain inbound commands (non-blocking)
    while let Ok(cmd) = inbound.try_recv() {
        handle_command(cmd);  // enqueue, ack, nack, register consumer, admin
    }

    // 2. Check expired visibility timeouts
    reclaim_expired_leases();

    // 3. Refill token buckets
    refill_token_buckets(now);

    // 4. Run DRR scheduling round — deliver ready messages
    for key in active_keys.round_robin() {
        if key.deficit <= 0 { continue; }
        if key.any_throttle_key_exhausted() { skip; continue; }

        if let Some(msg) = dequeue_next(key) {
            create_lease(msg, visibility_timeout);
            outbound.send(ReadyMessage(msg, consumer));
            key.deficit -= 1;  // or weight-based cost
        } else {
            active_keys.remove(key);  // no pending messages
        }
    }

    // 5. Reset deficits when round completes
    if round_complete() {
        for key in active_keys {
            key.deficit += key.weight * quantum;
        }
    }

    // 6. Park if idle (with timeout for lease expiry checking)
    if nothing_to_do { park_with_timeout(); }
}
```

### Data Architecture — RocksDB

**Decision:** RocksDB with column family isolation matching scheduler access patterns.

**Column Families:**

| Column Family | Key Format | Value | Purpose |
|---------------|-----------|-------|---------|
| `default` | — | — | RocksDB requires it; unused |
| `messages` | `{queue_id}:{fairness_key}:{enqueue_ts_ns}:{msg_id}` | Serialized message (headers, payload, metadata) | Message storage; key layout enables prefix iteration by queue → fairness key → enqueue order |
| `leases` | `{queue_id}:{msg_id}` | `{consumer_id}:{expiry_ts_ns}` | Active lease tracking |
| `lease_expiry` | `{expiry_ts_ns}:{queue_id}:{msg_id}` | `""` (empty) | Secondary index for efficient timeout scanning — iterate from earliest expiry |
| `queues` | `{queue_id}` | Serialized QueueConfig (name, lua scripts, settings, dlq_queue_id) | Queue definitions and metadata |
| `state` | `{key}` (e.g., `config:webhook:provider-Y:rate_limit`) | Arbitrary bytes | Runtime key-value config readable by Lua via `fila.get()` |

**Key Encoding:**
- All numeric values (timestamps, queue IDs) encoded as big-endian bytes for correct lexicographic ordering in RocksDB
- Message IDs are UUIDv7 (16 bytes, naturally lexicographically sortable — timestamp prefix ensures ordering, random suffix ensures uniqueness without coordination)
- Composite keys use `:` byte separator (0x3A)
- Fairness keys are user-defined strings (UTF-8, variable length) — length-prefixed in composite keys to avoid ambiguity

**Persistence Guarantees:**
- **Enqueue**: RocksDB `WriteBatch` atomically writes message + updates queue metadata. All-or-nothing.
- **Ack**: `WriteBatch` atomically removes message + lease + lease_expiry entry.
- **Nack**: `WriteBatch` atomically removes old message entry, writes updated message (new attempt count, possibly new fairness key), removes lease.

**In-Memory State (rebuilt on startup):**
- DRR scheduler state: active keys set, deficit counters, round position
- Token bucket state: tokens remaining, last refill timestamp (resets on restart — brief burst window, acceptable tradeoff per brainstorming decision)
- Consumer registry: connected consumers and their outbound channels
- Lua VM instances: pre-compiled scripts per queue

**Startup Recovery:**
1. Open RocksDB with all column families
2. Scan `queues` CF to load queue definitions and compile Lua scripts
3. Scan `messages` CF to rebuild active keys set and deficit state
4. Scan `leases` CF to restore in-flight lease state
5. Scan `lease_expiry` CF to identify already-expired leases for immediate reclaim
6. Ready to accept connections

### Wire Protocol — gRPC

**Decision:** gRPC only via tonic. Single protocol for hot path and admin operations.

**Protobuf Organization:**

| File | Contents |
|------|----------|
| `proto/fila/v1/messages.proto` | `Message` envelope, `MessageMetadata`, `Headers`, enums |
| `proto/fila/v1/service.proto` | `FilaService` — hot path RPCs: `Enqueue`, `Lease`, `Ack`, `Nack` |
| `proto/fila/v1/admin.proto` | `FilaAdmin` — admin RPCs: `CreateQueue`, `DeleteQueue`, `SetConfig`, `GetConfig`, `GetStats`, `Redrive` |

**Hot Path RPCs:**

| RPC | Type | Flow |
|-----|------|------|
| `Enqueue(EnqueueRequest) → EnqueueResponse` | Unary | Producer → Broker. Triggers on_enqueue Lua hook. Returns message ID. |
| `Lease(LeaseRequest) → stream LeaseResponse` | Server-streaming | Consumer ← Broker. Broker pushes ready messages. Stream held open. |
| `Ack(AckRequest) → AckResponse` | Unary | Consumer → Broker. Confirms processing. |
| `Nack(NackRequest) → NackResponse` | Unary | Consumer → Broker. Triggers on_failure Lua hook. |

**Admin RPCs:**

| RPC | Type | Description |
|-----|------|-------------|
| `CreateQueue(CreateQueueRequest) → CreateQueueResponse` | Unary | Create queue with Lua scripts and config |
| `DeleteQueue(DeleteQueueRequest) → DeleteQueueResponse` | Unary | Remove queue and all messages |
| `SetConfig(SetConfigRequest) → SetConfigResponse` | Unary | Set runtime key-value config |
| `GetConfig(GetConfigRequest) → GetConfigResponse` | Unary | Read runtime config |
| `GetStats(GetStatsRequest) → GetStatsResponse` | Unary | Queue stats, per-key metrics, DRR state |
| `Redrive(RedriveRequest) → RedriveResponse` | Unary | Move DLQ messages back to source queue |

**gRPC Design Rules:**
- Use standard gRPC status codes (NOT_FOUND, INVALID_ARGUMENT, INTERNAL, etc.)
- Propagate request metadata (trace context via gRPC metadata headers)
- Set deadlines on all client calls (SDKs default to 5s for unary, no deadline for streams)
- Protobuf backward compatibility: field addition only, never remove or renumber fields

### Lua Integration

**Decision:** mlua with Lua 5.4. Pre-compiled scripts cached per queue. Sandbox with time and memory limits. Circuit breaker on repeated failures.

**Hook Model:**

| Hook | Trigger | Input | Output | Purpose |
|------|---------|-------|--------|---------|
| `on_enqueue(msg)` | Every enqueue | Message headers, payload size | `{fairness_key, weight, throttle_keys}` | Assign scheduling metadata from message properties |
| `on_failure(msg, error, attempts)` | Every nack | Message headers, error info, attempt count | `{action: "retry" \| "dlq", delay_ms}` | Decide retry vs dead-letter, with optional delay |

**Lua API available to scripts:**

| Function | Description |
|----------|-------------|
| `fila.get(key)` | Read runtime config value from state store |
| `msg.headers` | Table of message headers (read-only) |
| `msg.payload_size` | Payload size in bytes (payload itself not exposed) |
| `msg.queue` | Queue name |
| `msg.id` | Message ID (on_failure only) |
| `msg.attempts` | Attempt count (on_failure only) |

**Safety Model:**
- **Pre-compilation**: Lua scripts compiled to bytecode at queue creation and cached. No parse overhead per message.
- **Execution timeout**: Configurable per-queue (default 10ms). Enforced via mlua instruction count hook.
- **Memory limit**: Configurable per-queue (default 1MB). Enforced via mlua allocator hook.
- **Circuit breaker**: After 3 consecutive Lua errors, bypass Lua and apply safe defaults (fairness_key = "default", weight = 1, no throttle keys) for a configurable cooldown period. Log error, increment counter.
- **No IO**: Lua sandbox has no access to filesystem, network, or OS functions. Only `fila.*` and standard Lua string/math/table libraries.

### Configuration Management

**Decision:** Two-layer configuration model.

**Layer 1 — Static Config (TOML file):**

Broker process configuration loaded at startup. Changes require restart.

```toml
[server]
listen_addr = "0.0.0.0:5555"
data_dir = "/var/lib/fila"

[scheduler]
quantum = 1000           # DRR quantum
visibility_timeout_ms = 30000
lease_expiry_check_interval_ms = 1000

[lua]
default_timeout_ms = 10
default_memory_limit_bytes = 1048576
circuit_breaker_threshold = 3
circuit_breaker_cooldown_ms = 10000

[telemetry]
otlp_endpoint = "http://localhost:4317"
service_name = "fila"
metrics_interval_ms = 10000
```

- File path: `fila.toml` (current dir) or `/etc/fila/fila.toml`
- Override any value with environment variables: `FILA_SERVER__LISTEN_ADDR=0.0.0.0:6666`

**Layer 2 — Runtime Config (gRPC API + RocksDB):**

Dynamic key-value pairs set via `SetConfig` RPC, stored in `state` column family, readable by Lua via `fila.get()`. Changes take effect on next message processing. No restart needed.

Used for: throttle rate limits, feature flags, business logic parameters that Lua scripts consume.

### Error Handling Strategy

**Decision:** Typed errors in core library, mapped to gRPC status codes at the server boundary.

| Layer | Error Approach | Crate |
|-------|---------------|-------|
| `fila-core` | `thiserror` enum with typed variants | `thiserror` |
| `fila-server` | Convert core errors → tonic `Status` | `tonic` |
| `fila-cli` | Convert tonic `Status` → user-friendly messages | `clap` |
| Lua hooks | Circuit breaker → safe defaults on error | `mlua` |

**Core Error Types:**

```rust
#[derive(Debug, thiserror::Error)]
pub enum FilaError {
    #[error("queue not found: {0}")]
    QueueNotFound(String),
    #[error("message not found: {0}")]
    MessageNotFound(String),
    #[error("lua script error: {0}")]
    LuaError(String),
    #[error("storage error: {0}")]
    StorageError(String),
    #[error("invalid configuration: {0}")]
    InvalidConfig(String),
    #[error("queue already exists: {0}")]
    QueueAlreadyExists(String),
}
```

**gRPC Status Code Mapping:**

| Core Error | gRPC Status |
|-----------|-------------|
| `QueueNotFound` | `NOT_FOUND` |
| `MessageNotFound` | `NOT_FOUND` |
| `LuaError` | `INTERNAL` (circuit breaker handles gracefully) |
| `StorageError` | `INTERNAL` |
| `InvalidConfig` | `INVALID_ARGUMENT` |
| `QueueAlreadyExists` | `ALREADY_EXISTS` |
| Unknown Ack/Nack | `NOT_FOUND` (idempotent) |

### Testing Strategy

**Decision:** Four-layer testing approach.

| Layer | Location | Framework | Scope |
|-------|----------|-----------|-------|
| Unit tests | Co-located `#[cfg(test)]` modules | `cargo test` | Individual functions, DRR logic, token bucket math, key encoding |
| Integration tests | `tests/integration/` | `cargo nextest` | Full broker lifecycle: start server, enqueue, lease, ack via gRPC |
| Property-based tests | Within unit/integration tests | `proptest` | Scheduler fairness invariants: "under sustained load, each key gets within 5% of fair share" |
| Benchmarks | `benches/` | `criterion` | Throughput, scheduling overhead, Lua execution latency |

**Integration Test Pattern:**
- Each test starts a real `fila-server` on a random port with a temp data directory
- Tests use a shared helper that sets up and tears down the broker
- Tests are independent and can run in parallel (separate ports, separate data dirs)

### Infrastructure & Deployment

**Decision:** Single binary, Docker image, cargo install, shell script. CI via GitHub Actions.

**Distribution Channels:**

| Channel | Command | Target User |
|---------|---------|-------------|
| Docker | `docker run ghcr.io/faisca/fila` | Quick evaluation, production deployment |
| Cargo | `cargo install fila-server` + `cargo install fila-cli` (installs as `fila`) | Rust developers, building from source |
| Shell script | `curl -fsSL https://get.fila.dev \| bash` | Quick binary install on Linux/macOS |
| GitHub Releases | Download binary from release page | Manual installation |

**Docker (multi-stage build):**

```dockerfile
FROM rust:latest AS builder
WORKDIR /build
COPY . .
RUN cargo build --release --bin fila-server --bin fila-cli

FROM debian:bookworm-slim
RUN apt-get update && apt-get install -y libssl3 && rm -rf /var/lib/apt/lists/*
COPY --from=builder /build/target/release/fila-server /usr/local/bin/
COPY --from=builder /build/target/release/fila /usr/local/bin/
EXPOSE 5555
ENTRYPOINT ["fila-server"]
```

**CI/CD (GitHub Actions):**
- On every PR: `cargo fmt --check`, `cargo clippy`, `cargo nextest run`, `cargo bench` (no regression check — informational)
- On merge to main: All above + build Docker image + push to ghcr.io
- On tag: All above + build release binaries (linux-amd64, linux-arm64, darwin-amd64, darwin-arm64) + create GitHub Release + push Docker tag

### Decision Impact Analysis

**Implementation Sequence:**
1. Workspace setup + proto definitions + fila-proto crate (M1 foundation)
2. fila-core: message types, storage trait, RocksDB implementation (M1)
3. fila-server: gRPC server with Enqueue, Lease, Ack (M1)
4. DRR scheduler in fila-core, wire into server (M2)
5. Lua sandbox + on_enqueue/on_failure hooks (M3)
6. Token buckets + throttle integration (M4)
7. Runtime config + admin RPCs + fila-cli (M5)
8. Streaming Lease delivery (M6)
9. Client SDKs (post-M6)

**Cross-Component Dependencies:**
- `fila-proto` is foundational — all other crates depend on it for message types
- `fila-core` depends on `fila-proto` for shared types
- `fila-server` depends on both `fila-core` and `fila-proto`
- `fila-cli` depends on `fila-proto` for gRPC client calls
- Storage trait in core enables test implementations without RocksDB

## Implementation Patterns & Consistency Rules

### Naming Patterns

**Rust Code Naming:**

| Element | Convention | Example |
|---------|-----------|---------|
| Crates | `kebab-case` | `fila-core`, `fila-server` |
| Modules | `snake_case` | `token_bucket`, `drr` |
| Types (structs, enums, traits) | `PascalCase` | `DrrScheduler`, `TokenBucket`, `StorageTrait` |
| Functions/methods | `snake_case` | `enqueue_message`, `run_on_enqueue` |
| Constants | `SCREAMING_SNAKE_CASE` | `DEFAULT_VISIBILITY_TIMEOUT_MS` |
| Variables | `snake_case` | `fairness_key`, `msg_id` |
| Type parameters | Single uppercase or `PascalCase` | `T`, `S: Storage` |
| Feature flags | `kebab-case` | (none planned for MVP) |

**Protobuf Naming:**

| Element | Convention | Example |
|---------|-----------|---------|
| Package | `lowercase.dot.separated` | `fila.v1` |
| Messages | `PascalCase` | `EnqueueRequest`, `LeaseResponse` |
| Fields | `snake_case` | `fairness_key`, `msg_id`, `throttle_keys` |
| Enums | `PascalCase` | `FailureAction` |
| Enum values | `SCREAMING_SNAKE_CASE` prefixed with enum name | `FAILURE_ACTION_RETRY`, `FAILURE_ACTION_DLQ` |
| RPCs | `PascalCase` | `Enqueue`, `Lease`, `CreateQueue` |
| Services | `PascalCase` | `FilaService`, `FilaAdmin` |

**RocksDB Key Naming:**

| Element | Convention | Example |
|---------|-----------|---------|
| Column family names | `snake_case` | `messages`, `lease_expiry` |
| Key separators | `:` byte (0x3A) | `queue_001:tenant_a:1707523200000000000:uuid` |
| Config keys in state CF | `dot.separated.path` | `config.webhook.provider-Y.rate_limit` |

**CLI Naming:**

| Element | Convention | Example |
|---------|-----------|---------|
| Subcommands | `kebab-case` | `fila queue create`, `fila config set` |
| Flags | `--kebab-case` | `--queue-name`, `--rate-limit` |
| Short flags | Single letter | `-q`, `-r` |
| Environment variables | `FILA_` prefix + `SCREAMING_SNAKE_CASE` | `FILA_SERVER__LISTEN_ADDR` |

**Metric Naming:**

| Element | Convention | Example |
|---------|-----------|---------|
| Metric names | `dot.separated` | `fila.messages.enqueued`, `fila.scheduler.drr.rounds` |
| Labels | `snake_case` | `queue_id`, `fairness_key`, `throttle_key` |
| Units | Suffix with unit | `fila.lua.execution_duration_us`, `fila.throttle.tokens_remaining` |

### Structure Patterns

**Module Organization — fila-core:**
- One module per domain concept (message, queue, scheduler, throttle, storage, lua, config, metrics, error)
- Sub-modules for implementations within a concept (`scheduler/drr.rs`, `storage/rocksdb.rs`)
- `mod.rs` files re-export the public API for the module
- Traits define boundaries; implementations are behind the trait

**File Size Guideline:**
- If a file exceeds ~500 lines, consider splitting into sub-modules
- Prefer many small, focused files over few large files

**Test Organization:**
- Unit tests: `#[cfg(test)] mod tests { ... }` at bottom of each source file
- Integration tests: `tests/integration/` with one file per test scenario
- Test helpers: `tests/integration/common/mod.rs`
- Benchmarks: `benches/` with one file per benchmark category

### Format Patterns

**gRPC Response Format:**
- Success: Return the response message directly with `Ok(Response::new(...))`
- Error: Return `Err(tonic::Status::...)` with appropriate code and message
- No custom wrapper — use gRPC's native error model

**Serialization:**
- Wire format: Protobuf (via prost) for all gRPC communication
- Storage format: Protobuf-encoded bytes in RocksDB (same message format as wire)
- Config files: TOML for static config
- Logs: Structured JSON when running in production mode; pretty-print in development

**Timestamp Format:**
- Internal: Nanoseconds since Unix epoch (`u64`) — matches RocksDB key ordering needs
- gRPC API: `google.protobuf.Timestamp` in proto definitions
- Logs: RFC 3339 / ISO 8601

### Communication Patterns

**Channel Communication (IO ↔ Scheduler):**

```rust
// Inbound: IO threads → Scheduler
enum SchedulerCommand {
    Enqueue { message: Message, reply: oneshot::Sender<Result<EnqueueResult>> },
    Ack { queue_id: QueueId, msg_id: MsgId, reply: oneshot::Sender<Result<()>> },
    Nack { queue_id: QueueId, msg_id: MsgId, error: String, reply: oneshot::Sender<Result<NackResult>> },
    RegisterConsumer { queue_id: QueueId, tx: mpsc::Sender<ReadyMessage> },
    UnregisterConsumer { consumer_id: ConsumerId },
    Admin(AdminCommand),
}

// Outbound: Scheduler → IO threads (ready messages for consumers)
// Delivered via per-consumer mpsc::Sender registered during RegisterConsumer
```

**Logging Patterns:**
- Use `tracing` macros: `tracing::info!`, `tracing::warn!`, `tracing::error!`
- Always include structured fields: `tracing::info!(queue_id = %queue_id, msg_id = %msg_id, "message enqueued")`
- Log levels: ERROR for unrecoverable failures, WARN for circuit breaker activations and recovered errors, INFO for lifecycle events (queue created, consumer connected), DEBUG for per-message flow, TRACE for scheduler internals
- Never log message payloads (could contain sensitive data)

### Process Patterns

**Error Recovery:**
- Lua errors → circuit breaker → safe defaults (never crash the scheduler)
- RocksDB errors → log error, return INTERNAL to caller (broker stays running)
- Channel disconnection → remove consumer from registry, leases governed by visibility timeout
- Startup failure → exit with clear error message and non-zero exit code

**Graceful Shutdown:**
1. Stop accepting new gRPC connections
2. Signal scheduler to drain: finish current round, stop accepting new commands
3. Wait for in-flight operations to complete (with timeout)
4. Flush RocksDB WAL
5. Exit cleanly

**Resource Cleanup:**
- Lua VMs: dropped when queue is deleted
- RocksDB: single instance, dropped on shutdown
- Channels: dropped when scheduler thread exits
- Consumers: tracked by scheduler, cleaned up on disconnect

### Enforcement Guidelines

**All AI Agents Implementing Fila MUST:**

1. Follow Rust naming conventions exactly as specified above
2. Use `thiserror` for all error types in `fila-core`, never `anyhow`
3. Send all scheduler-mutating operations through the crossbeam channel — never directly mutate scheduler state from an IO thread
4. Write RocksDB mutations as `WriteBatch` for atomicity — never individual puts for related changes
5. Add `tracing` instrumentation to every public function (at minimum: `#[tracing::instrument]` or manual span)
6. Write unit tests for any non-trivial logic
7. Use the storage trait for all persistence operations in core — never call RocksDB directly from scheduler code
8. Pre-compile Lua scripts at queue creation time — never interpret raw source per message
9. Encode RocksDB keys using big-endian byte encoding — never string formatting of numbers
10. Keep the scheduler core loop free of any async code or blocking IO

**Anti-Patterns to Avoid:**

- Using `Arc<Mutex<...>>` for scheduler state (defeats single-threaded design)
- Passing raw RocksDB handles between threads (all access through scheduler thread)
- Logging message payloads or headers that might contain PII
- Using `unwrap()` in production code paths (use proper error handling)
- Allocating per-message in the scheduler hot path where avoidable
- Hard-coding configuration values (use config file or constants)

## Project Structure & Boundaries

### Complete Project Directory Structure

```
fila/
├── Cargo.toml                          # Workspace root
├── Cargo.lock
├── README.md
├── LICENSE                             # AGPLv3
├── Dockerfile
├── fila.toml                           # Default broker config (example)
├── .github/
│   └── workflows/
│       ├── ci.yml                      # PR checks: fmt, clippy, test, bench
│       └── release.yml                 # Tag: build binaries, Docker, GitHub Release
├── proto/
│   └── fila/
│       └── v1/
│           ├── messages.proto          # Message envelope, metadata, enums
│           ├── service.proto           # FilaService: Enqueue, Lease, Ack, Nack
│           └── admin.proto             # FilaAdmin: CreateQueue, DeleteQueue, SetConfig, GetStats, Redrive
├── crates/
│   ├── fila-proto/                     # Generated protobuf + gRPC code
│   │   ├── Cargo.toml
│   │   ├── build.rs                    # prost-build + tonic-build codegen
│   │   └── src/
│   │       └── lib.rs                  # Re-export generated types and services
│   ├── fila-core/                      # Core library: scheduler, storage, lua, domain types
│   │   ├── Cargo.toml
│   │   └── src/
│   │       ├── lib.rs                  # Public API re-exports
│   │       ├── error.rs                # FilaError enum (thiserror)
│   │       ├── message.rs              # Message domain type, MessageMetadata
│   │       ├── queue.rs                # Queue definition, QueueConfig, DLQ association
│   │       ├── broker.rs               # Broker orchestrator: owns scheduler thread, channels
│   │       ├── scheduler/
│   │       │   ├── mod.rs              # Scheduler trait + core loop
│   │       │   └── drr.rs              # Deficit Round Robin implementation
│   │       ├── throttle/
│   │       │   ├── mod.rs              # Throttle manager: owns all token buckets
│   │       │   └── token_bucket.rs     # Single token bucket implementation
│   │       ├── storage/
│   │       │   ├── mod.rs              # Storage trait re-export
│   │       │   ├── traits.rs           # Storage trait definition
│   │       │   └── rocksdb.rs          # RocksDB implementation with column families
│   │       ├── lua/
│   │       │   ├── mod.rs              # Lua engine public API
│   │       │   ├── sandbox.rs          # Lua VM creation, security limits, fila.* API
│   │       │   └── hooks.rs            # on_enqueue + on_failure hook execution
│   │       ├── config.rs               # BrokerConfig struct (deserialized from TOML)
│   │       └── metrics.rs              # Metric names, recording helpers
│   ├── fila-server/                    # gRPC server binary
│   │   ├── Cargo.toml
│   │   └── src/
│   │       ├── main.rs                 # Entry point: parse args, load config, start server
│   │       ├── server.rs               # Server setup: create broker, bind gRPC, signal handling
│   │       ├── service/
│   │       │   ├── mod.rs              # Service module re-exports
│   │       │   ├── producer.rs         # Enqueue RPC handler
│   │       │   ├── consumer.rs         # Lease, Ack, Nack RPC handlers
│   │       │   └── admin.rs            # Admin RPC handlers (CreateQueue, SetConfig, etc.)
│   │       ├── convert.rs              # Proto ↔ Core type conversion (From/Into impls)
│   │       └── telemetry.rs            # OTel setup: tracing subscriber, metrics exporter
│   └── fila-cli/                       # CLI binary (crate: fila-cli, binary: fila)
│       ├── Cargo.toml
│       └── src/
│           ├── main.rs                 # Entry point, clap app definition (binary name: fila)
│           └── commands/
│               ├── mod.rs              # Command module re-exports
│               ├── queue.rs            # queue create, queue delete, queue list, queue inspect
│               ├── config.rs           # config get, config set
│               ├── stats.rs            # stats (queue stats, per-key metrics)
│               └── redrive.rs          # redrive (DLQ → source queue)
├── tests/
│   └── integration/
│       ├── common/
│       │   └── mod.rs                  # Test helpers: start/stop broker, create client, temp dirs
│       ├── enqueue_ack_test.rs         # Basic message lifecycle: enqueue → lease → ack
│       ├── fairness_test.rs            # DRR correctness: multi-key fair share verification
│       ├── throttle_test.rs            # Token bucket: throttle skip, rate recovery
│       ├── lua_hooks_test.rs           # Lua execution: on_enqueue, on_failure, circuit breaker
│       ├── visibility_test.rs          # Visibility timeout: expiry → redelivery
│       ├── dlq_test.rs                 # Dead letter queue: nack → DLQ → redrive
│       └── crash_recovery_test.rs      # Kill process → restart → verify state intact
├── benches/
│   ├── throughput.rs                   # End-to-end enqueue/lease/ack throughput
│   ├── scheduler.rs                    # DRR scheduling overhead vs raw FIFO
│   └── lua_execution.rs               # Lua hook execution latency (p50, p99)
└── examples/
    ├── fair_scheduling.rs              # Demo: multi-tenant fair scheduling
    └── throttling.rs                   # Demo: per-key rate limiting
```

### Architectural Boundaries

**Crate Dependency Graph:**

```
fila-proto (generated types + gRPC definitions)
     │
     ├───────────────┐
     ▼               ▼
fila-core          fila-cli
(scheduler,        (gRPC client,
 storage,           CLI commands)
 lua, domain)
     │
     ▼
fila-server
(gRPC handlers,
 type conversion,
 telemetry)
```

**Boundary Rules:**

| Boundary | Rule |
|----------|------|
| `fila-core` → `fila-proto` | Core depends on proto for shared message types only. No gRPC dependency. |
| `fila-server` → `fila-core` | Server calls core through `Broker` API. Never reaches into core internals. |
| `fila-server` → `fila-proto` | Server implements gRPC services defined in proto. |
| `fila-cli` → `fila-proto` | CLI creates gRPC client stubs from proto service definitions. |
| `fila-cli` → `fila-core` | CLI does NOT depend on core. Communicates exclusively via gRPC. |
| IO threads → Scheduler | IO threads send `SchedulerCommand` via crossbeam channel. Never call scheduler methods directly. |
| Scheduler → RocksDB | Scheduler accesses storage exclusively through `Storage` trait. |
| Lua → Broker state | Lua reads runtime config via `fila.get()` bridge. Cannot modify broker state directly. |

**Data Flow — Enqueue:**

```
Producer → gRPC IO thread → SchedulerCommand::Enqueue → crossbeam → Scheduler thread
    → run on_enqueue Lua → compute fairness_key, weight, throttle_keys
    → RocksDB WriteBatch (message + metadata)
    → add to active keys set
    → oneshot reply → IO thread → gRPC response
```

**Data Flow — Lease:**

```
Consumer → gRPC IO thread → SchedulerCommand::RegisterConsumer → crossbeam → Scheduler thread
    → scheduler adds consumer to registry with mpsc::Sender
    → (in DRR loop) scheduler finds ready message for consumer's queue
    → check throttle → check deficit → dequeue → create lease → WriteBatch
    → mpsc::Sender.send(ReadyMessage) → IO thread → gRPC stream push
```

**Data Flow — Ack:**

```
Consumer → gRPC IO thread → SchedulerCommand::Ack → crossbeam → Scheduler thread
    → RocksDB WriteBatch (remove message + lease + lease_expiry)
    → oneshot reply → IO thread → gRPC response
```

**Data Flow — Nack:**

```
Consumer → gRPC IO thread → SchedulerCommand::Nack → crossbeam → Scheduler thread
    → run on_failure Lua → decide retry or DLQ
    → if retry: WriteBatch (update message with new attempt count, remove lease)
    → if DLQ: WriteBatch (move message to DLQ queue, remove lease)
    → oneshot reply → IO thread → gRPC response
```

### Requirements to Structure Mapping

**FR Category → Crate/Module Mapping:**

| FR Category | Primary Location | Supporting Locations |
|------------|-----------------|---------------------|
| Message Lifecycle (FR1–7) | `fila-core/src/message.rs`, `broker.rs`, `scheduler/mod.rs` | `fila-server/src/service/producer.rs`, `consumer.rs` |
| Fair Scheduling (FR8–12) | `fila-core/src/scheduler/drr.rs` | `fila-core/src/broker.rs` |
| Throttling (FR13–17) | `fila-core/src/throttle/token_bucket.rs` | `fila-core/src/scheduler/mod.rs` |
| Rules Engine (FR18–25) | `fila-core/src/lua/sandbox.rs`, `hooks.rs` | `fila-core/src/broker.rs` |
| Runtime Config (FR26–29) | `fila-core/src/storage/traits.rs`, `rocksdb.rs` | `fila-server/src/service/admin.rs` |
| Queue Management (FR30–34) | `fila-core/src/queue.rs`, `broker.rs` | `fila-server/src/service/admin.rs`, `fila-cli/src/commands/queue.rs` |
| Observability (FR35–41) | `fila-core/src/metrics.rs` | `fila-server/src/telemetry.rs`, every module via `tracing` |
| Client SDKs (FR42–48) | `proto/fila/v1/*.proto` | External SDK repositories (generated from proto) |
| Deployment (FR49–53) | `Dockerfile`, `.github/workflows/`, `Cargo.toml` | `fila-server/src/main.rs` |
| Documentation (FR54–60) | `README.md`, `examples/`, generated from proto | External docs site (future) |

**Cross-Cutting Concerns → Location:**

| Concern | Primary File(s) | Pattern |
|---------|----------------|---------|
| Observability | `fila-core/src/metrics.rs`, `fila-server/src/telemetry.rs` | Every module uses `tracing::instrument` |
| Error handling | `fila-core/src/error.rs`, `fila-server/src/convert.rs` | Core errors → gRPC status at server boundary |
| Crash recovery | `fila-core/src/storage/rocksdb.rs`, `fila-core/src/broker.rs` | WriteBatch atomicity + startup recovery scan |
| Configuration | `fila-core/src/config.rs`, `fila-server/src/main.rs` | TOML deserialized at startup, env var overrides |

## Architecture Validation Results

### Coherence Validation

**Decision Compatibility:**
- All technology choices are Rust-native and work together: tokio + tonic + prost + rocksdb + mlua + tracing + opentelemetry. No version conflicts.
- Single-threaded scheduler design is compatible with crossbeam channels and RocksDB (RocksDB is thread-safe for concurrent reads, scheduler is the only writer).
- Protobuf types flow cleanly through the stack: proto → core types → storage → gRPC responses.

**Pattern Consistency:**
- Naming conventions follow Rust ecosystem standards throughout — no custom conventions that could confuse agents.
- Error handling is consistent: thiserror in core, tonic Status at boundary, structured logging everywhere.
- All state mutations go through the scheduler thread via channels — pattern is consistent and enforceable.

**Structure Alignment:**
- Workspace crate structure matches the architectural boundary decisions exactly.
- Each crate has a clear, singular responsibility.
- Dependency graph is acyclic and minimal.

### Requirements Coverage Validation

**Functional Requirements Coverage:**

| FR Category | Coverage | Notes |
|------------|---------|-------|
| Message Lifecycle (FR1–7) | Full | Enqueue/Lease/Ack/Nack RPCs, visibility timeout, DLQ, redrive all mapped to specific code locations |
| Fair Scheduling (FR8–12) | Full | DRR scheduler with active keys, deficit tracking, weighted throughput |
| Throttling (FR13–17) | Full | Token buckets per key, multi-key from Lua, skip-throttled in DRR, runtime API |
| Rules Engine (FR18–25) | Full | mlua sandbox, on_enqueue/on_failure, fila.get(), timeout/memory limits, circuit breaker, pre-compiled cache |
| Runtime Config (FR26–29) | Full | RocksDB state CF, gRPC SetConfig/GetConfig, Lua fila.get() bridge |
| Queue Management (FR30–34) | Full | Admin RPCs, auto-DLQ, stats via GetStats, fila-cli binary |
| Observability (FR35–41) | Full | OTel metrics + tracing on every operation, per-key labels, metric naming conventions defined |
| Client SDKs (FR42–48) | Full | Proto-first design enables generated SDKs; proto files are the source of truth |
| Deployment (FR49–53) | Full | Single binary, Dockerfile, cargo install, shell script, WriteBatch crash recovery |
| Documentation (FR54–60) | Partial | Architecture supports proto-generated API ref and examples; docs site is an implementation concern |

**Non-Functional Requirements Coverage:**

| NFR | Architectural Support |
|-----|----------------------|
| NFR1: 100k+ msg/s | Single-threaded core, lock-free channels, RocksDB batch writes, pre-compiled Lua |
| NFR2: <5% scheduling overhead | DRR is O(active_keys) per round; active keys set avoids scanning empty keys |
| NFR3: 5% fairness accuracy | DRR with quantum + deficit tracking; property-based tests verify invariant |
| NFR4-5: <50us Lua p99 | Pre-compiled bytecode, instruction count timeout, memory limits |
| NFR6: <1ms enqueue-to-lease | Lock-free channels; scheduler loop runs continuously |
| NFR7: <1us token bucket | In-memory token buckets; simple arithmetic per check |
| NFR8: Zero message loss recovery | RocksDB WriteBatch atomicity; startup recovery scan |
| NFR9: Atomic enqueue | WriteBatch for all multi-key mutations |
| NFR10: Expired timeouts resolved in one cycle | lease_expiry CF with timestamp-ordered keys; checked every loop iteration |
| NFR11: Circuit breaker in 3 failures | Configurable threshold in Lua sandbox |
| NFR12: At-least-once delivery | Visibility timeout + lease tracking |
| NFR13: Single-engineer operability | Single binary, TOML config, CLI admin, OTel metrics |
| NFR14: <10min onboarding | Docker run + examples/ directory |
| NFR15: No external dependencies | Embedded RocksDB, embedded Lua VM |
| NFR16: All state queryable | GetStats RPC + fila-cli + OTel metrics |
| NFR17: Runtime config without restart | gRPC SetConfig → RocksDB state CF → Lua fila.get() |
| NFR18-21: Integration standards | OTel export, gRPC best practices, protobuf compat, idiomatic SDKs |

### Implementation Readiness Validation

**Decision Completeness:**
- All critical technology choices made with rationale
- Concurrency model fully specified with pseudocode
- Data model (RocksDB column families + key encoding) fully specified
- Error handling strategy complete with mapping table
- Configuration model (static TOML + runtime API) fully specified

**Structure Completeness:**
- Every file and directory in the project tree is specified with its purpose
- All FR categories mapped to specific code locations
- Cross-cutting concerns mapped to implementation patterns

**Pattern Completeness:**
- Naming conventions cover all layers (Rust, Protobuf, RocksDB, CLI, metrics)
- Communication patterns (channels, commands) specified with type signatures
- Error handling, logging, shutdown, and recovery patterns all documented

### Architecture Completeness Checklist

**Requirements Analysis**
- [x] Project context thoroughly analyzed (60 FRs, 21 NFRs)
- [x] Scale and complexity assessed (High — systems infrastructure)
- [x] Technical constraints identified (Rust, solo dev, single-node MVP, no external deps)
- [x] Cross-cutting concerns mapped (observability, crash recovery, Lua safety, performance, testability)

**Architectural Decisions**
- [x] Critical decisions documented: concurrency model, storage, protocol, key encoding, crate organization
- [x] Technology stack fully specified with all crate dependencies
- [x] Integration patterns defined (channel commands, gRPC, storage trait)
- [x] Performance considerations addressed at every level

**Implementation Patterns**
- [x] Naming conventions established for all layers
- [x] Structure patterns defined (modules, files, tests)
- [x] Communication patterns specified (channels, commands, data flow)
- [x] Process patterns documented (error recovery, shutdown, startup)

**Project Structure**
- [x] Complete directory structure with all files and purposes
- [x] Component boundaries established (crate dependency graph)
- [x] Integration points mapped (data flow diagrams for all RPCs)
- [x] Requirements to structure mapping complete

### Architecture Readiness Assessment

**Overall Status:** READY FOR IMPLEMENTATION

**Confidence Level:** High

**Key Strengths:**
- Architecture is deeply informed by prior brainstorming and stress-testing — risks have been identified and mitigated
- Redis-validated concurrency model provides proven performance characteristics
- Clean crate separation enables incremental development matching the M1-M5 milestone plan
- Every functional and non-functional requirement has a clear architectural home
- Patterns are specific enough to prevent agent conflicts while not being over-prescriptive

**Areas for Future Enhancement:**
- SDK repository structure and CI (per-language repos, generated from proto)
- Documentation site architecture (when docs-as-product phase begins)
- Clustering and replication architecture (post-MVP, when single-node capacity is exceeded)
- Authentication layer design (post-MVP, when multi-tenant production deployments emerge)

### Implementation Handoff

**AI Agent Guidelines:**
- Follow all architectural decisions exactly as documented in this file
- Use implementation patterns consistently across all components
- Respect crate boundaries — never violate the dependency graph
- All scheduler state mutations go through the channel — no exceptions
- Refer to this document for all architectural questions

**First Implementation Priority:**
1. Create Cargo workspace with all four crates (`fila-proto`, `fila-core`, `fila-server`, `fila-cli`)
2. Write `.proto` files for `messages.proto`, `service.proto`, `admin.proto`
3. Set up `fila-proto` build.rs with prost-build + tonic-build
4. Verify generated code compiles
5. This establishes the foundation for M1 development
