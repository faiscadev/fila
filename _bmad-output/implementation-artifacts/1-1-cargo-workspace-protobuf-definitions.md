# Story 1.1: Cargo Workspace & Protobuf Definitions

Status: done

## Story

As a developer,
I want the project workspace and protobuf API definitions to be established,
So that all crates have a shared foundation and generated types to build upon.

## Acceptance Criteria

1. A Cargo workspace root exists with four member crates: `fila-proto`, `fila-core`, `fila-server`, `fila-cli`
2. `proto/fila/v1/messages.proto` defines the `Message` envelope with id, headers, payload, timestamps, and metadata fields
3. `proto/fila/v1/service.proto` defines `FilaService` with `Enqueue`, `Lease`, `Ack`, and `Nack` RPCs
4. `proto/fila/v1/admin.proto` defines `FilaAdmin` with `CreateQueue`, `DeleteQueue`, `SetConfig`, `GetConfig`, `GetStats`, and `Redrive` RPCs
5. `fila-proto` has a `build.rs` that runs protobuf and gRPC code generation to produce Rust types and service/client code
6. `fila-proto/src/lib.rs` re-exports all generated types and services
7. All four crates compile successfully with `cargo build`
8. The workspace `Cargo.toml` uses workspace-level dependency management
9. A `LICENSE` file with AGPLv3 text is present at the workspace root

## Tasks / Subtasks

- [x] Task 1: Create Cargo workspace root (AC: #1, #8)
  - [x] 1.1 Create root `Cargo.toml` with `[workspace]` members: `crates/fila-proto`, `crates/fila-core`, `crates/fila-server`, `crates/fila-cli`
  - [x] 1.2 Define `[workspace.dependencies]` for all shared dependencies (see Dev Notes for versions)
  - [x] 1.3 Set workspace-level `edition = "2021"`, `license = "AGPL-3.0-or-later"`, `repository`, `rust-version`
- [x] Task 2: Create protobuf definitions (AC: #2, #3, #4)
  - [x] 2.1 Create `proto/fila/v1/messages.proto` — `Message` envelope, `MessageMetadata`, `Headers`
  - [x] 2.2 Create `proto/fila/v1/service.proto` — `FilaService` with `Enqueue`, `Lease` (server-streaming), `Ack`, `Nack`
  - [x] 2.3 Create `proto/fila/v1/admin.proto` — `FilaAdmin` with `CreateQueue`, `DeleteQueue`, `SetConfig`, `GetConfig`, `GetStats`, `Redrive`
  - [x] 2.4 Use `google.protobuf.Timestamp` for all timestamp fields
  - [x] 2.5 Follow protobuf naming: `PascalCase` messages, `snake_case` fields, `SCREAMING_SNAKE_CASE` enum values prefixed with enum name
- [x] Task 3: Set up fila-proto crate (AC: #5, #6)
  - [x] 3.1 Create `crates/fila-proto/Cargo.toml` with dependencies on `tonic`, `prost`, `tonic-prost`, `prost-types`, and build-dependency on `tonic-prost-build`
  - [x] 3.2 Create `crates/fila-proto/build.rs` using `tonic_prost_build::configure().compile_protos(...)` to generate code from all 3 proto files
  - [x] 3.3 Create `crates/fila-proto/src/lib.rs` with `tonic::include_proto!("fila.v1")` re-exporting all generated types and services
- [x] Task 4: Create stub crates (AC: #1, #7)
  - [x] 4.1 Create `crates/fila-core/Cargo.toml` with dependency on `fila-proto`; `src/lib.rs` as empty module
  - [x] 4.2 Create `crates/fila-server/Cargo.toml` with dependencies on `fila-proto` and `fila-core`; `src/main.rs` as minimal binary
  - [x] 4.3 Create `crates/fila-cli/Cargo.toml` with dependency on `fila-proto`; `src/main.rs` as minimal binary; set `[[bin]] name = "fila"`
- [x] Task 5: Add LICENSE and project files (AC: #9)
  - [x] 5.1 Add `LICENSE` with full AGPLv3 text at workspace root
  - [x] 5.2 Update `.gitignore` with Rust patterns (`/target`, `*.swp`, etc.)
- [x] Task 6: Verify build (AC: #7)
  - [x] 6.1 Run `cargo build` — all four crates compile successfully
  - [x] 6.2 Run `cargo clippy` — no warnings
  - [x] 6.3 Run `cargo fmt --check` — formatted

## Dev Notes

### Critical: tonic 0.14 Build Pipeline Change

As of tonic 0.14 (January 2026), the protobuf build pipeline has changed:
- **Use `tonic-prost-build`** (NOT the old `tonic-build` + `prost-build` combo)
- `tonic-prost-build` replaces both and bridges prost codegen with tonic's service generation
- **`protoc` must be installed** on the build machine (CI needs `protoc` in PATH)

**build.rs pattern:**
```rust
fn main() -> Result<(), Box<dyn std::error::Error>> {
    // Cargo runs build.rs from the crate root (crates/fila-proto/),
    // so paths must be relative to there, not the workspace root.
    let proto_root = "../../proto";
    tonic_prost_build::configure()
        .compile_protos(
            &[
                &format!("{proto_root}/fila/v1/messages.proto"),
                &format!("{proto_root}/fila/v1/service.proto"),
                &format!("{proto_root}/fila/v1/admin.proto"),
            ],
            &[proto_root],
        )?;
    Ok(())
}
```

### Workspace Dependency Versions

Use these versions in `[workspace.dependencies]`:

| Crate | Version | Notes |
|-------|---------|-------|
| `tonic` | `"0.14"` | gRPC framework; must match prost 0.14.x |
| `prost` | `"0.14"` | Protobuf codegen; must match tonic 0.14.x |
| `tonic-prost-build` | `"0.14"` | Build dependency; replaces old tonic-build |
| `tokio` | `"1"` (features: `full`) | Async runtime |
| `serde` | `"1"` (features: `derive`) | Serialization |
| `thiserror` | `"1"` | Error derivation (fila-core) |
| `clap` | `"4"` (features: `derive`) | CLI parsing |
| `uuid` | `"1"` (features: `v7`) | Message IDs |
| `bytes` | `"1"` | Byte buffers |
| `crossbeam-channel` | `"0.5"` | Lock-free channels |
| `tracing` | `"0.1"` | Structured logging |
| `rocksdb` | `"0.24"` | Embedded KV store |

**Version compatibility rule:** tonic and prost MUST use matching 0.14.x versions. Mismatched versions will cause build failures.

Only add dependencies to each crate's `Cargo.toml` that the crate actually needs for this story. Stub crates (fila-core, fila-server, fila-cli) should only declare their workspace member dependency on fila-proto where needed. Other dependencies will be added in later stories.

### Protobuf Design Specifications

**messages.proto** must define:
```protobuf
syntax = "proto3";
package fila.v1;

import "google/protobuf/timestamp.proto";

message Message {
  string id = 1;                              // UUIDv7, broker-assigned
  map<string, string> headers = 2;            // User-provided key-value pairs
  bytes payload = 3;                          // Opaque byte payload
  MessageMetadata metadata = 4;
  MessageTimestamps timestamps = 5;
}

message MessageMetadata {
  string fairness_key = 1;
  uint32 weight = 2;
  repeated string throttle_keys = 3;
  uint32 attempt_count = 4;
  string queue_id = 5;
}

message MessageTimestamps {
  google.protobuf.Timestamp enqueued_at = 1;
  google.protobuf.Timestamp leased_at = 2;
}
```

**service.proto** must define:
```protobuf
syntax = "proto3";
package fila.v1;

import "fila/v1/messages.proto";

service FilaService {
  rpc Enqueue(EnqueueRequest) returns (EnqueueResponse);
  rpc Lease(LeaseRequest) returns (stream LeaseResponse);  // server-streaming
  rpc Ack(AckRequest) returns (AckResponse);
  rpc Nack(NackRequest) returns (NackResponse);
}

message EnqueueRequest {
  string queue = 1;
  map<string, string> headers = 2;
  bytes payload = 3;
}

message EnqueueResponse {
  string message_id = 1;
}

message LeaseRequest {
  string queue = 1;
}

message LeaseResponse {
  Message message = 1;
}

message AckRequest {
  string queue = 1;
  string message_id = 2;
}

message AckResponse {}

message NackRequest {
  string queue = 1;
  string message_id = 2;
  string error = 3;
}

message NackResponse {}
```

**admin.proto** must define:
```protobuf
syntax = "proto3";
package fila.v1;

service FilaAdmin {
  rpc CreateQueue(CreateQueueRequest) returns (CreateQueueResponse);
  rpc DeleteQueue(DeleteQueueRequest) returns (DeleteQueueResponse);
  rpc SetConfig(SetConfigRequest) returns (SetConfigResponse);
  rpc GetConfig(GetConfigRequest) returns (GetConfigResponse);
  rpc GetStats(GetStatsRequest) returns (GetStatsResponse);
  rpc Redrive(RedriveRequest) returns (RedriveResponse);
}

message CreateQueueRequest {
  string name = 1;
  QueueConfig config = 2;
}

message QueueConfig {
  string on_enqueue_script = 1;
  string on_failure_script = 2;
  uint64 visibility_timeout_ms = 3;
}

message CreateQueueResponse {
  string queue_id = 1;
}

message DeleteQueueRequest {
  string queue = 1;
}

message DeleteQueueResponse {}

message SetConfigRequest {
  string key = 1;
  string value = 2;
}

message SetConfigResponse {}

message GetConfigRequest {
  string key = 1;
}

message GetConfigResponse {
  string value = 1;
}

message GetStatsRequest {
  string queue = 1;
}

message GetStatsResponse {
  uint64 depth = 1;
  uint64 in_flight = 2;
  uint64 active_fairness_keys = 3;
}

message RedriveRequest {
  string dlq_queue = 1;
  uint64 count = 2;
}

message RedriveResponse {
  uint64 redriven = 1;
}
```

**Protobuf naming rules:**
- Package: `fila.v1`
- Messages: `PascalCase` (e.g., `EnqueueRequest`)
- Fields: `snake_case` (e.g., `message_id`, `fairness_key`)
- Enums: `PascalCase` name, `SCREAMING_SNAKE_CASE` values prefixed with enum name
- RPCs: `PascalCase` (e.g., `CreateQueue`)
- Services: `PascalCase` (e.g., `FilaService`, `FilaAdmin`)

**Backward compatibility rule:** Field addition only. Never remove or renumber fields.

### Project Structure (This Story)

After this story, the project tree should be:

```
fila/
├── Cargo.toml                    # Workspace root
├── Cargo.lock
├── LICENSE                       # AGPLv3
├── .gitignore
├── proto/
│   └── fila/
│       └── v1/
│           ├── messages.proto
│           ├── service.proto
│           └── admin.proto
└── crates/
    ├── fila-proto/
    │   ├── Cargo.toml
    │   ├── build.rs
    │   └── src/
    │       └── lib.rs
    ├── fila-core/
    │   ├── Cargo.toml
    │   └── src/
    │       └── lib.rs
    ├── fila-server/
    │   ├── Cargo.toml
    │   └── src/
    │       └── main.rs
    └── fila-cli/
        ├── Cargo.toml
        └── src/
            └── main.rs
```

### Anti-Patterns to Avoid

- Do NOT use `tonic-build` or `prost-build` directly — use `tonic-prost-build` (tonic 0.14 change)
- Do NOT add dependencies to stub crates that aren't needed yet (e.g., don't add rocksdb to fila-core in this story)
- Do NOT create any `mod.rs` files or module structure beyond the minimal `lib.rs`/`main.rs` — later stories will add modules
- Do NOT add `anyhow` — project uses `thiserror` for error handling
- Do NOT use `unwrap()` in any production code paths
- Do NOT create README.md, fila.toml, or Dockerfile — those come in later stories

### fila-cli Binary Name

The `fila-cli` crate must produce a binary named `fila` (not `fila-cli`). Set this in `Cargo.toml`:
```toml
[[bin]]
name = "fila"
path = "src/main.rs"
```

### Crate Dependency Graph (This Story)

```
fila-proto  (standalone — no internal deps)
     │
     ├─────────────┐
     ▼             ▼
fila-core      fila-cli (depends on fila-proto)
     │
     ▼
fila-server (depends on fila-proto + fila-core)
```

Note: `fila-cli` does NOT depend on `fila-core`. It communicates exclusively via gRPC using types from `fila-proto`.

### References

- [Source: _bmad-output/planning-artifacts/architecture.md#Project Structure & Boundaries]
- [Source: _bmad-output/planning-artifacts/architecture.md#Wire Protocol — gRPC]
- [Source: _bmad-output/planning-artifacts/architecture.md#Implementation Patterns & Consistency Rules]
- [Source: _bmad-output/planning-artifacts/architecture.md#Naming Patterns]
- [Source: _bmad-output/planning-artifacts/epics.md#Story 1.1: Cargo Workspace & Protobuf Definitions]
- [Source: _bmad-output/planning-artifacts/prd.md#API Specification]

## Dev Agent Record

### Agent Model Used

Claude Opus 4.6

### Debug Log References

- Build initially failed: `compile_protos` include paths required `String` not `&str` — fixed with `.to_string()`
- Build failed again: tonic 0.14 generated code references `tonic_prost::ProstCodec` and `prost_types::Timestamp` — added `tonic-prost` and `prost-types` as runtime dependencies

### Completion Notes List

- Workspace created with 4 crates, workspace-level dependency management, resolver v2
- Three protobuf files define complete API surface: messages, hot-path service, admin service
- fila-proto build pipeline uses tonic-prost-build (tonic 0.14 pattern) with proto paths relative to crate root
- Stub crates compile cleanly; fila-cli produces binary named `fila`
- AGPLv3 LICENSE downloaded from GNU official source
- All checks pass: cargo build, cargo clippy -D warnings, cargo fmt --check

### File List

- Cargo.toml (new — workspace root)
- Cargo.lock (new — generated)
- LICENSE (new — AGPLv3)
- proto/fila/v1/messages.proto (new)
- proto/fila/v1/service.proto (new)
- proto/fila/v1/admin.proto (new)
- crates/fila-proto/Cargo.toml (new)
- crates/fila-proto/build.rs (new)
- crates/fila-proto/src/lib.rs (new)
- crates/fila-core/Cargo.toml (new)
- crates/fila-core/src/lib.rs (new)
- crates/fila-server/Cargo.toml (new)
- crates/fila-server/src/main.rs (new)
- crates/fila-cli/Cargo.toml (new)
- crates/fila-cli/src/main.rs (new)
