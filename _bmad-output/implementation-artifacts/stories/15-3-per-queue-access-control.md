# Story 15.3: Per-Queue Access Control

Status: done

## Story

As an operator,
I want to define access control policies per queue,
So that I can restrict which clients can produce to or consume from specific queues.

## Acceptance Criteria

1. **Given** API key authentication is enabled (from Story 15.2), **when** ACL policies are configured, **then** each API key can be associated with a set of permissions: `produce:<queue_pattern>`, `consume:<queue_pattern>`, `admin:<queue_pattern>`.

2. **Given** queue patterns, **then** `*` matches any queue name, and `orders.*` matches any queue whose name starts with `orders.` (glob-style prefix wildcard).

3. **Given** an authenticated RPC, **when** the API key lacks the required permission, **then** the server returns `PERMISSION_DENIED` with a descriptive message.

4. **Given** an RPC with a valid key:
   - `Enqueue` requires `produce:<queue>`
   - `Consume` requires `consume:<queue>`
   - All `FilaAdmin` RPCs (queue management, config, stats, redrive) require `admin:*`
   - Key-management RPCs (`CreateApiKey`, `RevokeApiKey`, `ListApiKeys`) and ACL RPCs (`SetAcl`, `GetAcl`) require `admin:*`

5. **Given** a superadmin key (created with `is_superadmin: true`), **when** any RPC is made, **then** all ACL checks are bypassed — the key has full access.

6. **Given** `SetAcl` and `GetAcl` admin RPCs, **then** operators can associate permissions with a key_id and retrieve the current ACL for a key.

7. **Given** auth is enabled, **when** a new key is created, **then** it has no permissions until `SetAcl` is called (secure-by-default). Superadmin keys are the exception.

8. **Given** ACL changes via `SetAcl`, **when** the operation completes, **then** subsequent RPCs use the new permissions immediately — no restart required.

9. **Given** auth is disabled, **then** all ACL checks are skipped — behavior identical to pre-auth.

10. **Given** integration tests, **then** verify: key with produce-only can enqueue but not consume; key with consume-only can consume but not enqueue; superadmin bypasses all checks.

## Tasks / Subtasks

- [x] Task 1: Domain types — ACL model (AC: 1, 2, 5)
  - [x] 1.1 Define `Permission` enum: `Produce`, `Consume`, `Admin`
  - [x] 1.2 Add `permissions: Vec<(String, String)>` and `is_superadmin: bool` to `ApiKeyEntry` in `broker/auth.rs`
  - [x] 1.3 Implement `pattern_matches(pattern: &str, queue: &str)` — `*` = any, `foo.*` = prefix
  - [x] 1.4 Implement `has_permission(entry: &ApiKeyEntry, perm: Permission, queue: &str) -> bool`

- [x] Task 2: Proto changes (AC: 6)
  - [x] 2.1 Add `SetAcl`, `GetAcl` RPCs to `FilaAdmin` service in `admin.proto`
  - [x] 2.2 Add messages: `AclPermission`, `SetAclRequest/Response`, `GetAclRequest/Response`
  - [x] 2.3 Update `CreateApiKeyRequest` to include `is_superadmin: bool`
  - [x] 2.4 Update `CreateApiKeyResponse` to reflect superadmin status

- [x] Task 3: Storage (AC: 7, 8)
  - [x] 3.1 Store permissions + is_superadmin in existing `ApiKeyEntry` JSON (backward compat via `#[serde(default)]`)
  - [x] 3.2 Add `set_acl`, `get_acl`, `check_permission` to `Broker`

- [x] Task 4: Auth middleware — inject key_id as request extension (AC: 3, 4, 9)
  - [x] 4.1 `ValidatedKeyId(String)` already added in Story 15.2
  - [x] 4.2 `AuthService::call` already injects `ValidatedKeyId` in Story 15.2
  - [x] 4.3 Auth disabled path does not inject (handlers see `None`)

- [x] Task 5: Permission enforcement in service handlers (AC: 3, 4)
  - [x] 5.1 `HotPathService::enqueue` checks `Produce` permission
  - [x] 5.2 `HotPathService::consume` checks `Consume` permission
  - [x] 5.3 All `AdminService` handlers call `check_admin(&request)?` requiring `Admin:*`
  - [x] 5.4 `RevokeApiKey`, `ListApiKeys`, `SetAcl`, `GetAcl` all gated by `check_admin`
  - [x] 5.5 `CreateApiKey` bypasses ACL (already bypasses auth in middleware)

- [x] Task 6: Admin service — SetAcl/GetAcl handlers (AC: 6)
  - [x] 6.1 Implemented `set_acl` handler
  - [x] 6.2 Implemented `get_acl` handler

- [x] Task 7: CLI — acl subcommands (AC: 6)
  - [x] 7.1 Added `fila auth acl set <key-id> --perm <kind:pattern>` subcommand
  - [x] 7.2 Added `fila auth acl get <key-id>` subcommand
  - [x] 7.3 Updated `fila auth create` to accept `--superadmin` flag

- [x] Task 8: Integration tests (AC: 1–10)
  - [x] 8.1 Test: key without permissions cannot enqueue (PERMISSION_DENIED)
  - [x] 8.2 Test: produce-only key can enqueue to granted queue but not other queues
  - [x] 8.3 Test: wildcard `produce:*` allows enqueue to any queue
  - [x] 8.4 Test: superadmin key bypasses all ACL checks
  - [x] 8.5 Test: admin operations require admin permission
  - [x] 8.6 CLI test: `auth acl get` shows permissions
  - [x] 8.7 CLI test: `auth acl get` shows superadmin status

## Dev Notes

### Permission Format

Permissions are stored as `(kind, pattern)` pairs:
- `produce:orders` — can enqueue to queue `orders`
- `produce:*` — can enqueue to any queue
- `consume:orders.*` — can consume from `orders.us`, `orders.eu`, etc.
- `admin:*` — can make any admin RPC

### Pattern Matching

```rust
fn pattern_matches(pattern: &str, queue: &str) -> bool {
    if pattern == "*" { return true; }
    if let Some(prefix) = pattern.strip_suffix(".*") {
        return queue.starts_with(&format!("{prefix}."));
    }
    pattern == queue
}
```

### Key Extension Injection

Auth middleware injects key_id for handlers:
```rust
// In auth.rs (after successful validation):
req.extensions_mut().insert(ValidatedKeyId(key_id));

// In service handlers (via tonic::Request):
let key_id = req.extensions().get::<ValidatedKeyId>().map(|k| k.0.as_str());
```

tonic wraps `http::Request` so extensions set on the HTTP request are accessible via `tonic::Request::extensions()`.

### Superadmin Design

Superadmin is a flag on the key entry. A superadmin key bypasses `check_permission` — useful for:
- Initial bootstrap (create queues, set ACLs for other keys)
- Operator emergency access

### Storage Layout

ACL data is stored within the existing `ApiKeyEntry`:
```rust
struct ApiKeyEntry {
    // ... existing fields ...
    permissions: Vec<(String, String)>, // (kind, pattern), e.g. ("produce", "orders.*")
    is_superadmin: bool,
}
```

No separate storage key needed — permissions are co-located with the key entry.

### Key Files to Modify

| File | Change |
|------|--------|
| `crates/fila-proto/proto/fila/v1/admin.proto` | `SetAcl`, `GetAcl` RPCs + messages, update `CreateApiKeyRequest` |
| `crates/fila-core/src/broker/auth.rs` | `Permission`, `AclEntry`, pattern matching |
| `crates/fila-core/src/broker/mod.rs` | `set_acl`, `get_acl`, `check_permission` methods |
| `crates/fila-server/src/auth.rs` | `ValidatedKeyId` extension, inject on valid key |
| `crates/fila-server/src/service.rs` | ACL check in `enqueue`, `consume` |
| `crates/fila-server/src/admin_service.rs` | ACL check in admin handlers, `set_acl`/`get_acl` impl |
| `crates/fila-cli/src/main.rs` | `auth acl set/get`, `--superadmin` flag |
| `crates/fila-e2e/tests/acl.rs` | Integration tests |

## Dev Agent Record

### Agent Model Used

claude-sonnet-4-6

### Debug Log References

None.

### Completion Notes List

- `check_admin(&request)?` gating added to all 13 `AdminService` handlers (create_queue, delete_queue, set_config, get_config, list_config, get_stats, redrive, list_queues, revoke_api_key, list_api_keys, set_acl, get_acl). `CreateApiKey` is exempt (bootstrap path, already bypasses auth middleware).
- ACL permissions stored as `Vec<(String, String)>` (kind, pattern) in existing `ApiKeyEntry` JSON with `#[serde(default)]` for backward compat.
- Auth tests updated to use `--superadmin` keys since admin ops now require admin permission.
- `start_auth_server` and `cli_create_superadmin_key` moved to `helpers/mod.rs` so both `auth.rs` and `acl.rs` test modules can use them.

### File List

| File | Change |
|------|--------|
| `crates/fila-proto/proto/fila/v1/admin.proto` | `SetAcl`, `GetAcl` RPCs + messages, `is_superadmin` in create key |
| `crates/fila-core/src/broker/auth.rs` | `Permission`, `has_permission`, `pattern_matches`, updated `ApiKeyEntry` |
| `crates/fila-core/src/broker/mod.rs` | `check_permission`, `set_acl`, `get_acl`, `create_api_key` with `is_superadmin` |
| `crates/fila-server/src/service.rs` | `Produce`/`Consume` ACL checks in `enqueue`/`consume` |
| `crates/fila-server/src/admin_service.rs` | `check_admin` helper + gating in all handlers, `set_acl`/`get_acl` impl |
| `crates/fila-cli/src/main.rs` | `auth acl set/get`, `--superadmin` flag |
| `crates/fila-e2e/tests/acl.rs` | 8 new integration tests |
| `crates/fila-e2e/tests/auth.rs` | Updated to use superadmin keys for admin ops |
| `crates/fila-e2e/tests/helpers/mod.rs` | `start_auth_server`, `cli_create_superadmin_key` helpers |
