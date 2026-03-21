# Story 16.1: Versioning Scheme & SDK Compatibility Matrix

Status: ready-for-dev

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

- [ ] Task 1: Embed version in server binary (AC: 1, 8)
  - [ ] 1.1 Set workspace `Cargo.toml` version to `0.1.0` (already is — confirm all crates inherit or pin)
  - [ ] 1.2 In `fila-server/src/main.rs`, use `env!("CARGO_PKG_VERSION")` to capture version at compile time
  - [ ] 1.3 Add `--version` flag to `fila-server` clap config (clap's `#[command(version)]`)
  - [ ] 1.4 In `fila-cli/src/main.rs`, add `--version` flag similarly

- [ ] Task 2: Proto — GetServerInfo RPC (AC: 4, 5)
  - [ ] 2.1 Add `GetServerInfo` RPC to `FilaAdmin` service in `admin.proto`
  - [ ] 2.2 Define `GetServerInfoRequest` (empty) and `GetServerInfoResponse { string server_version = 1; string proto_version = 2; repeated string features = 3; }`
  - [ ] 2.3 Proto version string: `"v1"` (matches `fila.v1` package)
  - [ ] 2.4 Features list: enumerate currently supported features (e.g., `"fair_scheduling"`, `"throttling"`, `"lua_scripting"`, `"tls"`, `"api_key_auth"`, `"acl"`, `"clustering"`, `"dlq"`, `"redrive"`)

- [ ] Task 3: Server — GetServerInfo handler (AC: 4)
  - [ ] 3.1 Implement `get_server_info` in `AdminService` — return version from `env!("CARGO_PKG_VERSION")`, proto version `"v1"`, features list
  - [ ] 3.2 `GetServerInfo` should NOT require authentication (unauthenticated clients need version info to negotiate)
  - [ ] 3.3 Auth bypass: add `GetServerInfo` to the auth middleware's exempt list (same pattern as `CreateApiKey` bootstrap path)

- [ ] Task 4: Rust SDK — version awareness (AC: 5)
  - [ ] 4.1 Add `get_server_info()` method to `FilaClient` in `fila-sdk`
  - [ ] 4.2 SDK embeds its own version via `env!("CARGO_PKG_VERSION")`
  - [ ] 4.3 Optional: log warning if server version is newer MAJOR than SDK version

- [ ] Task 5: CLI — server-info subcommand (AC: 4)
  - [ ] 5.1 Add `fila server-info` subcommand that calls `GetServerInfo` and prints version, proto version, features

- [ ] Task 6: Documentation — compatibility matrix (AC: 3, 6, 7)
  - [ ] 6.1 Create `docs/compatibility.md` with: versioning policy (semver rules), proto compatibility policy (additive-only within MAJOR), SDK-server compatibility matrix table
  - [ ] 6.2 Matrix: columns = Server version, Rust SDK, Go SDK, Python SDK, JS SDK, Ruby SDK, Java SDK. Rows = version ranges with feature support
  - [ ] 6.3 Document current state: all at 0.1.0, external SDKs lack auth features (TLS, API keys — to be added in Story 16.2)
  - [ ] 6.4 Include deprecation policy: minimum 1 MINOR version deprecation window before removal
  - [ ] 6.5 Link from main README to `docs/compatibility.md`

- [ ] Task 7: Integration tests (AC: 4, 8)
  - [ ] 7.1 Test: `GetServerInfo` returns valid version string matching semver pattern
  - [ ] 7.2 Test: `GetServerInfo` returns proto_version `"v1"`
  - [ ] 7.3 Test: `GetServerInfo` returns non-empty features list containing expected features
  - [ ] 7.4 Test: `GetServerInfo` works without authentication (when auth is enabled)
  - [ ] 7.5 Test: `--version` flag on server and CLI binaries prints version and exits

## Dev Notes

### Architecture Patterns

- **Proto location**: `crates/fila-proto/proto/fila/v1/admin.proto` — add `GetServerInfo` to existing `FilaAdmin` service
- **Admin handler**: `crates/fila-server/src/grpc/admin.rs` — implement handler
- **Auth bypass pattern**: In `crates/fila-server/src/broker/auth.rs`, the `AuthService` middleware has an exempt path check. `GetServerInfo` needs the same treatment as the bootstrap path — check `CallerKey` enum usage from Story 15.2/15.3
- **CLI structure**: `crates/fila-cli/src/main.rs` uses clap derive macros with subcommand enums
- **SDK client**: `crates/fila-sdk/src/lib.rs` — `FilaClient` wraps tonic generated client

### Error Handling

Follow CLAUDE.md: explicit error variant matching, no catch-all `.to_string()`. `GetServerInfo` is simple (no error states beyond gRPC transport), so error handling is minimal for this RPC.

### Per-Command Error Types

`GetServerInfo` has no domain errors — it always succeeds. No new error type needed. The SDK method can return `ConnectError` (already exists from Epic 7).

### Current Version State

- All Cargo.toml versions are `0.1.0`
- No `env!("CARGO_PKG_VERSION")` usage anywhere currently
- No `--version` flags exist on any binary
- Release workflows use Git tags (`v*`) for versioning — this is correct and continues unchanged
- Bleeding-edge uses commit SHA — also continues unchanged

### Proto Backward Compatibility

The `fila.v1` package has only ever added fields/RPCs, never removed. This story formalizes that as policy. Adding `GetServerInfo` RPC is itself an additive change (backward compatible).

### External SDKs — Out of Scope

This story does NOT update the 5 external SDKs (Go, Python, JS, Ruby, Java). They remain at 0.1.0 without `GetServerInfo`. Story 16.2 will update them with auth + proto changes including this new RPC.

### Testing

- Use existing `TestServer` helper from `fila-sdk` for integration tests
- Auth bypass test: start server with auth enabled, call `GetServerInfo` without a key — should succeed
- Binary version test: use `std::process::Command` to run `fila-server --version` and verify output

### Project Structure Notes

- No new crates or modules — all changes in existing files
- New file: `docs/compatibility.md` only
- Proto changes regenerate via `build.rs` in `fila-proto`

### References

- [Source: epics.md — Epic 16, Story 16.1]
- [Source: architecture.md — Wire protocol, gRPC, tonic]
- [Source: Story 15.2 — Auth middleware, CallerKey enum, auth bypass pattern]
- [Source: Story 15.3 — ACL enforcement, admin RPC gating]
- [Source: CLAUDE.md — Error handling patterns, CI requirements]

## Dev Agent Record

### Agent Model Used

### Debug Log References

### Completion Notes List

### File List
