# Review Notes — Epic 15: Authentication & Security

## PR #71: mTLS Transport Security (Story 15.1)

### Gaps in Dev Process
- `tokio/fs` feature not included in `fila-core`'s Cargo.toml despite `tokio::fs::read` being used in `build_client_tls`/`build_server_tls`. Should have been caught by a local build before PR.
- URL scheme normalization logic was written inline 4 times across `network.rs` and `mod.rs` rather than extracted as a helper from the start.
- Plaintext code path passed `https://` addresses through unchanged (`starts_with("http")` matches both `http://` and `https://`) — subtle correctness bug that Cubic caught (P2).

### Incorrect Decisions During Development
- None requiring reversal. Doc comments that initially referenced `tls_config.enabled` (pre-existing field name) and described CA cert as "required" were corrected in-branch before final review.

### Deferred Work
- None.

### Patterns for Future Stories
- When adding `tokio` async I/O features (`fs`, `io-util`, etc.), update `Cargo.toml` feature list before writing code that uses them.
- When a URL normalization or connection helper is needed in more than one place, extract it immediately rather than copy-pasting. The `normalize_cluster_url` + `connect_channel` pattern in `cluster/network.rs` is now the canonical way to connect to cluster peers — future stories should use it.

## PR #72: API Key Authentication (Story 15.2)

### Gaps in Dev Process
- `AuthConfig` was originally designed with `CreateApiKey` RPC-level bypass (and `RevokeApiKey`/`ListApiKeys` initially also bypassed). Lucas correctly identified this as a security nightmare — replaced with `bootstrap_apikey: String` (required field when `[auth]` is present). The type system now makes it impossible to enable auth without a bootstrap key.
- E2E tests initially passed `[auth]` config with no `bootstrap_apikey` — server crashed on startup (missing required field). Tests needed to provide `bootstrap_apikey` in config and use it for first key creation.
- `cargo fmt` issue in `main.rs` inline struct caused Format CI failure on first push.
- Story doc had inconsistencies: claimed StorageEngine trait methods were added (they weren't), and that admin RPCs bypass auth (they don't).

### Incorrect Decisions During Development
- `validate_api_key` originally returned `bool`; the 15.3 story changed it to `Option<String>` (returning the key_id). This caused an auto-merge conflict where `Ok(true)` appeared in the `Option<String>` return path. Bootstrap key now returns `Some("__bootstrap__")` as a synthetic key_id, and `check_permission` recognizes `"__bootstrap__"` as superadmin.

### Deferred Work
- None.

### Patterns for Future Stories
- When a config field is required (cannot be `None`) when a section is present, use `String` not `Option<String>`. The type system enforces the constraint.
- `FILA_BOOTSTRAP_APIKEY` env var enables auth AND sets the bootstrap key — use it in CI/tests to avoid committing credentials.

## PR #73: Per-Queue Access Control (Story 15.3)

### Gaps in Dev Process
- 15.3 branch was built on old 15.2 base (with `CreateApiKey` bypass). After 15.2 was redesigned to remove the bypass, the rebase required resolving conflicts in `auth.rs`, `broker/mod.rs`, and all e2e auth/ACL tests.
- `cli_create_regular_key` in `acl.rs` called `auth create` without a key — failed with "invalid or missing api key" in CI. Same pattern as `auth.rs` `cli_create_key` which was already fixed on 15.2.
- Rebase auto-merged `broker/mod.rs` with `Ok(true)` type mismatch (returning `bool` where `Option<String>` expected). Required manual fix to use `Ok(Some("__bootstrap__"))`.

### Incorrect Decisions During Development
- 15.3 added `AUTH_BYPASS_PATHS = ["/fila.v1.FilaAdmin/CreateApiKey"]` — removed during rebase since Lucas's 15.2 decision eliminated all bypasses. Bootstrap key is the mechanism.

### Deferred Work
- None.

### Patterns for Future Stories
- When a story that introduces an auth bypass is later superseded, explicitly search and remove all traces (test helpers, config strings, doc comments).
- When a stacked branch is rebased after significant changes to the parent, check all test helpers for auth patterns — they may still reference the old design.
