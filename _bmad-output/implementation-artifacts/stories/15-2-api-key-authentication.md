# Story 15.2: API Key Authentication

Status: done

## Story

As an operator,
I want to authenticate clients using API keys,
so that I can control which clients can access the broker.

## Acceptance Criteria

1. **Given** a `[auth]` section is present in `fila.toml`, **when** the server starts, **then** API key authentication is enabled; **when** the section is absent, authentication is disabled (backward compatible — all existing clients work unchanged).

2. **Given** auth is enabled, **when** a client sends an RPC without the `authorization: Bearer <key>` metadata header, **then** the server returns `UNAUTHENTICATED` status.

3. **Given** auth is enabled, **when** a client sends an RPC with an invalid or revoked key, **then** the server returns `UNAUTHENTICATED` status.

4. **Given** auth is enabled, **when** a client sends an RPC with a valid, non-expired key, **then** the RPC proceeds normally.

5. **Given** the `FilaAdmin` gRPC service, **then** three new RPCs are available: `CreateApiKey` (returns key_id + plaintext key, stored as SHA-256 hash), `RevokeApiKey` (by key_id), `ListApiKeys` (returns key_id, name, created_at, expires_at — never the plaintext key).

6. **Given** API key creation/revocation, **when** the operation completes, **then** an audit log entry is emitted via `tracing::info!` with key_id, action, and timestamp fields.

7. **Given** the Rust SDK `ConnectOptions`, **when** `.with_api_key(key: String)` is called, **then** the SDK attaches the `authorization: Bearer <key>` metadata to every outgoing RPC.

8. **Given** the `fila` CLI, **when** `--api-key <key>` is provided, **then** the CLI attaches the header to all RPCs.

9. **Given** API key validation overhead, **when** benchmarked against auth-disabled baseline, **then** overhead is < 100µs per request (SHA-256 hash + hash map lookup).

10. **Given** the default (no `[auth]` section), **when** the server starts, **then** behavior is identical to before — no authentication required, all requests succeed.

## Tasks / Subtasks

- [x] Task 1: Config (AC: 1, 10)
  - [x] 1.1 Add `AuthConfig` struct and `Option<AuthConfig>` to `BrokerConfig` in `config.rs`
  - [x] 1.2 Re-export from `broker/mod.rs` and `lib.rs`
  - [x] 1.3 Add TOML parsing tests (absent → disabled, present → enabled)

- [x] Task 2: Proto changes (AC: 5)
  - [x] 2.1 Add `ApiKeyEntry` message and `CreateApiKey`, `RevokeApiKey`, `ListApiKeys` RPCs to `fila_admin.proto`
  - [x] 2.2 Re-generate proto code

- [x] Task 3: Domain types + storage (AC: 4, 5)
  - [x] 3.1 Add `sha2 = "0.10"` to workspace `Cargo.toml` (dev dep for fila-core)
  - [x] 3.2 Define `ApiKeyId` (UUID) and `ApiKeyEntry` (key_id, name, hashed_key, created_at, expires_at) in `broker/auth.rs`
  - [x] 3.3 Add `create_api_key`, `revoke_api_key`, `list_api_keys`, `validate_api_key_hash` methods to `StorageEngine` trait
  - [x] 3.4 Implement in `RocksDbEngine` using column family `auth` with key `api_key:<key_id>` → JSON `ApiKeyEntry`
  - [x] 3.5 Add validation helper: SHA-256 hash token, scan all keys to find matching hash

- [x] Task 4: Broker methods (AC: 4, 5, 6)
  - [x] 4.1 Add `create_api_key`, `revoke_api_key`, `list_api_keys`, `validate_api_key` to `Broker`
  - [x] 4.2 `create_api_key`: generate random token (UUID v4 hex), hash it, store entry, return (key_id, token)
  - [x] 4.3 Audit log via `tracing::info!` on create and revoke

- [x] Task 5: Auth middleware (AC: 2, 3, 4, 9)
  - [x] 5.1 Add `sha2` as dependency to `fila-core`
  - [x] 5.2 Write `AuthLayer` / `AuthService` tower middleware in `crates/fila-server/src/auth.rs`
  - [x] 5.3 Middleware extracts `authorization` header, parses `Bearer <token>`, hashes token, validates via broker
  - [x] 5.4 When auth disabled: pass through unconditionally
  - [x] 5.5 Apply layer in `fila-server/src/main.rs` between `TraceContextLayer` and the services
  - [x] 5.6 Admin RPCs for key management (`CreateApiKey`, `RevokeApiKey`, `ListApiKeys`) bypass auth (they're the mechanism for key issuance)

- [x] Task 6: Admin service wire-up (AC: 5)
  - [x] 6.1 Implement `CreateApiKey`, `RevokeApiKey`, `ListApiKeys` handlers in `admin_service.rs`

- [x] Task 7: SDK + CLI (AC: 7, 8)
  - [x] 7.1 Add `with_api_key(key: String)` to `ConnectOptions` in `fila-sdk/src/client.rs`
  - [x] 7.2 Apply `authorization: Bearer <key>` metadata interceptor via tonic `Interceptor`
  - [x] 7.3 Add `--api-key` flag to `fila` CLI; pass header via `tonic::metadata::MetadataValue`

- [x] Task 8: Integration tests (AC: 1–4, 10)
  - [x] 8.1 Test: request succeeds without key when auth disabled
  - [x] 8.2 Test: request rejected without key when auth enabled
  - [x] 8.3 Test: request rejected with invalid key
  - [x] 8.4 Test: request succeeds with valid key
  - [x] 8.5 Test: revoked key is rejected

## Dev Notes

### Config Design

```toml
[auth]
# type is optional — only "api_key" is supported (and is the default when section present)
type = "api_key"
```

```rust
#[derive(Debug, Clone, Deserialize)]
pub struct AuthConfig {
    // In the future, other auth types (JWT, etc.) could be added here.
}
```

Presence of `[auth]` section in `fila.toml` enables auth. `Option<AuthConfig>` on `BrokerConfig`.

### Key Format

- **Token**: random UUID v4 rendered as hex (32 chars) — opaque to clients
- **Stored**: SHA-256 hash of token (32 bytes as hex)
- **Returned**: plaintext token once on creation, never again

### Storage Layout

Column family `auth` in RocksDB:
- Key: `api_key:<key_id_uuid>` → JSON `ApiKeyEntry`

```rust
struct ApiKeyEntry {
    key_id: String,       // UUIDv4 as string
    name: String,         // human-readable label
    hashed_key: String,   // hex(SHA-256(token))
    created_at: u64,      // Unix timestamp ms
    expires_at: Option<u64>, // None = never expires
}
```

### Validation Flow

1. Extract `authorization` header from gRPC metadata
2. Parse `Bearer <token>`
3. Compute `SHA-256(token)` → hex string
4. Scan all `api_key:*` entries in storage, find matching `hashed_key`
5. If found: check expiry
6. If not found or expired: return `UNAUTHENTICATED`

Note: O(n) scan is acceptable for small key counts. If key counts grow large, add a reverse index (`hash → key_id`). NFR28 requires < 100µs total — SHA-256 is ~1µs for a UUID-length input, and RocksDB prefix scan over a handful of keys is < 10µs.

### Auth Middleware

Follow the pattern in `crates/fila-server/src/trace_context.rs`:

```rust
pub struct AuthLayer {
    broker: Option<Arc<Broker>>,  // None when auth disabled
}
```

The middleware must:
- Let through requests when `broker` is `None` (auth disabled)
- Let through `CreateApiKey`, `RevokeApiKey`, `ListApiKeys` admin RPCs regardless (bootstrap problem)
- Reject everything else without valid key

### RPC Bypass List

Key management RPCs bypass auth to solve the bootstrap problem (how to create the first key?). In production, operators create the first key directly via CLI/admin while network access is restricted, then enable auth in config.

RPCs that bypass auth:
- `FilaAdmin/CreateApiKey`
- `FilaAdmin/RevokeApiKey`
- `FilaAdmin/ListApiKeys`

### External SDKs

The other 5 SDK repos (fila-go, fila-python, fila-js, fila-ruby, fila-java) need `api_key` support too, but they are separate repositories. This story covers only the Rust SDK and CLI within this workspace. External SDK updates are tracked separately.

### Error Handling

Use `tonic::Status::unauthenticated("invalid or missing api key")` — no details that reveal why it failed (timing/enumeration resistance).

### Key Files to Modify

| File | Change |
|------|--------|
| `Cargo.toml` (workspace) | Add `sha2 = "0.10"` |
| `crates/fila-proto/proto/fila_admin.proto` | New messages + RPCs |
| `crates/fila-core/src/broker/config.rs` | `AuthConfig`, `Option<AuthConfig>` in `BrokerConfig` |
| `crates/fila-core/src/storage/mod.rs` | API key storage methods on trait |
| `crates/fila-core/src/storage/rocksdb.rs` | Implement storage methods |
| `crates/fila-core/src/broker/mod.rs` | `create_api_key`, `revoke_api_key`, etc. on `Broker` |
| `crates/fila-server/src/auth.rs` | Auth middleware (new file) |
| `crates/fila-server/src/main.rs` | Apply auth layer |
| `crates/fila-server/src/admin_service.rs` | Implement new RPCs |
| `crates/fila-sdk/src/client.rs` | `with_api_key`, metadata interceptor |
| `crates/fila-cli/src/main.rs` | `--api-key` flag |
| `crates/fila-e2e/tests/auth.rs` | Integration tests (new file) |

## Dev Agent Record

### Agent Model Used

claude-sonnet-4-6

### Debug Log References

### Completion Notes List

- Implemented auth key storage via existing `put_state`/`list_state_by_prefix` — no new StorageEngine trait methods needed
- Only `CreateApiKey` bypasses auth (bootstrap); `RevokeApiKey`/`ListApiKeys` require a valid key
- Cubic caught 8 issues across 2 review cycles: security bypass scope, poll_ready contract, layer ordering, silent MetadataValue failure, missing name validation, silent deserialization, test specificity, stale doc comment — all fixed
- `fila auth create/revoke/list` CLI subcommands added (not in original ACs but implied)
- PR: #72

### File List

- `Cargo.toml` — added `sha2`, `rcgen`, updated `uuid` features
- `crates/fila-core/Cargo.toml` — added `sha2`, `"fs"` tokio feature
- `crates/fila-core/src/broker/auth.rs` — new: `ApiKeyEntry`, `hash_key`, `storage_key`, `now_ms`
- `crates/fila-core/src/broker/config.rs` — added `AuthConfig`, `Option<AuthConfig>` to `BrokerConfig`
- `crates/fila-core/src/broker/mod.rs` — added `create_api_key`, `revoke_api_key`, `list_api_keys`, `validate_api_key`
- `crates/fila-core/src/lib.rs` — re-exported `AuthConfig`
- `crates/fila-proto/proto/fila/v1/admin.proto` — added `CreateApiKey`, `RevokeApiKey`, `ListApiKeys` RPCs
- `crates/fila-server/src/auth.rs` — new: Tower `AuthLayer`/`AuthService` middleware
- `crates/fila-server/src/admin_service.rs` — added `create_api_key`, `revoke_api_key`, `list_api_keys` handlers
- `crates/fila-server/src/main.rs` — added `mod auth`, applied `AuthLayer`
- `crates/fila-server/Cargo.toml` — added `futures-core`
- `crates/fila-sdk/src/client.rs` — added `with_api_key`, `request<T>` helper, `api_key` on `FilaClient`
- `crates/fila-cli/src/main.rs` — added `--api-key` flag, `auth create/revoke/list` subcommands
- `crates/fila-e2e/tests/auth.rs` — new: 5 integration tests
