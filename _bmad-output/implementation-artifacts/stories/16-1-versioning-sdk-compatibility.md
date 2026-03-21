# Story 16.1: Versioning Scheme & SDK Compatibility Matrix

Status: review

## Story

As a developer,
I want to consult an SDK-server compatibility matrix with documented guarantees,
so that I know which SDK versions work with which server versions.

## Acceptance Criteria

1. **Given** Fila server and 6 SDKs are independently versioned, **when** the versioning scheme is formalized, **then** the server adopts semantic versioning (semver): MAJOR.MINOR.PATCH.

2. **Given** semver is adopted, **then** MAJOR bumps indicate breaking proto/API changes, MINOR bumps indicate new features (backward compatible), PATCH bumps indicate bug fixes.

3. **Given** the need for version visibility, **then** a `docs/compatibility.md` documents the SDK-server compatibility matrix: minimum server version per SDK version, supported proto versions, deprecation policy.

4. **Given** any node in a deployment, **when** a client calls `GetServerInfo`, **then** the server returns: server version, proto version, and supported features list.

5. **Given** any SDK, **then** it can query `GetServerInfo` at connection time for optional compatibility verification.

6. **Given** the compatibility document, **then** it is publishable alongside release notes.

7. **Given** protobuf backward compatibility, **then** the policy is formalized: field additions only within a MAJOR version, no field removals or type changes.

8. **Given** the server binary and CLI, **when** `--version` is passed, **then** the binary prints its semver version and exits.

## Tasks / Subtasks

- [x] Task 1: Embed version in server binary (AC: 1, 8)
  - [x] 1.1 Set workspace `Cargo.toml` version to `0.1.0` (already is — confirmed all crates pin)
  - [x] 1.2 In `fila-server/src/main.rs`, use `env!("CARGO_PKG_VERSION")` via clap `#[command(version)]`
  - [x] 1.3 Add `--version` flag to `fila-server` clap config
  - [x] 1.4 In `fila-cli/src/main.rs`, add `--version` flag

- [x] Task 2: Proto — GetServerInfo RPC (AC: 4, 5)
  - [x] 2.1 Add `GetServerInfo` RPC to `FilaAdmin` service in `admin.proto`
  - [x] 2.2 Define `GetServerInfoRequest` (empty) and `GetServerInfoResponse { server_version, proto_version, features }`
  - [x] 2.3 Proto version string: `"v1"` (matches `fila.v1` package)
  - [x] 2.4 Features list: 9 features (fair_scheduling, throttling, lua_scripting, tls, api_key_auth, acl, clustering, dlq, redrive)

- [x] Task 3: Server — GetServerInfo handler (AC: 4)
  - [x] 3.1 Implement `get_server_info` in `AdminService` — returns version, proto version, features
  - [x] 3.2 `GetServerInfo` does NOT require authentication
  - [x] 3.3 Auth bypass: added `/fila.v1.FilaAdmin/GetServerInfo` to `AUTH_BYPASS_PATHS`

- [x] Task 4: Rust SDK — version awareness (AC: 5)
  - [x] 4.1 Add `get_server_info()` method to `FilaClient` (creates admin client on same channel)
  - [x] 4.2 SDK embeds its own version via `FilaClient::sdk_version()` using `env!("CARGO_PKG_VERSION")`
  - [x] 4.3 Added `ServerInfo` struct and `ServerInfoError` per-operation error type

- [x] Task 5: CLI — server-info subcommand (AC: 4)
  - [x] 5.1 Add `fila server-info` subcommand printing version, proto version, features

- [x] Task 6: Documentation — compatibility matrix (AC: 3, 6, 7)
  - [x] 6.1 Create `docs/compatibility.md` with versioning policy, proto compatibility policy, SDK-server matrix
  - [x] 6.2 Matrix table with all 6 SDKs, TLS/auth feature support columns
  - [x] 6.3 Document current state: all at 0.1.0, external SDKs lack auth
  - [x] 6.4 Include deprecation policy: 1 MINOR version window
  - [x] 6.5 Link from README to `docs/compatibility.md`

- [x] Task 7: Integration tests (AC: 4, 8)
  - [x] 7.1 Unit test: `GetServerInfo` returns valid version string matching semver pattern
  - [x] 7.2 Unit test: `GetServerInfo` returns proto_version `"v1"`
  - [x] 7.3 Unit test: `GetServerInfo` returns non-empty features list with all expected features
  - [x] 7.4 E2E test: `GetServerInfo` works without authentication (auth-enabled server)
  - [x] 7.5 E2E test: `GetServerInfo` returns valid semver and features on plain server

## Dev Notes

### Architecture Patterns

- **Proto location**: `crates/fila-proto/proto/fila/v1/admin.proto` — added `GetServerInfo` to `FilaAdmin` service
- **Admin handler**: `crates/fila-server/src/admin_service.rs` — implemented handler
- **Auth bypass**: `crates/fila-server/src/auth.rs` — added path to `AUTH_BYPASS_PATHS`
- **CLI structure**: `crates/fila-cli/src/main.rs` — added `ServerInfo` variant to `Commands` enum
- **SDK client**: `crates/fila-sdk/src/client.rs` — added `Channel` field, `get_server_info()`, `sdk_version()`

### References

- [Source: epics.md — Epic 16, Story 16.1]
- [Source: architecture.md — Wire protocol, gRPC, tonic]
- [Source: Story 15.2 — Auth middleware, CallerKey enum, auth bypass pattern]
- [Source: CLAUDE.md — Error handling patterns, CI requirements]

## Dev Agent Record

### Agent Model Used

Claude Opus 4.6 (1M context)

### Debug Log References

None.

### Completion Notes List

- All 7 tasks implemented in single pass, no debug iterations needed
- Added `clap` dependency to fila-server for `--version` support
- SDK stores `Channel` separately to create admin client for `GetServerInfo` without requiring second connection
- `ServerInfoError` follows per-operation error type pattern (wraps `StatusError`)
- Auth bypass uses path-matching pattern (same as the existing bypass list, which was previously empty)
- 5 new tests: 3 unit tests in admin_service.rs, 2 e2e tests in auth.rs

### File List

- `crates/fila-proto/proto/fila/v1/admin.proto` — added GetServerInfo RPC + messages
- `crates/fila-server/Cargo.toml` — added clap dependency
- `crates/fila-server/src/main.rs` — added clap Args struct with version
- `crates/fila-server/src/admin_service.rs` — added get_server_info handler + 3 unit tests
- `crates/fila-server/src/auth.rs` — added GetServerInfo to AUTH_BYPASS_PATHS
- `crates/fila-cli/src/main.rs` — added version flag, ServerInfo command, cmd_server_info handler
- `crates/fila-sdk/src/client.rs` — added Channel field, ServerInfo struct, get_server_info(), sdk_version()
- `crates/fila-sdk/src/error.rs` — added ServerInfoError
- `crates/fila-sdk/src/lib.rs` — re-export ServerInfo, ServerInfoError
- `crates/fila-e2e/tests/auth.rs` — 2 new e2e tests for GetServerInfo
- `docs/compatibility.md` — new: versioning policy + SDK-server matrix
- `README.md` — added link to docs/compatibility.md
