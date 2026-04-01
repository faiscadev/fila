# Epic 20 Review Notes

## PR #158: Binary Protocol Server — Hot-Path Operations

### Gaps in Dev Process
- No continuation frame reassembly implemented despite being specced in docs/protocol.md — codec had the constants but no actual logic
- rustls CryptoProvider conflict not caught: adding tokio-rustls alongside tonic's tls-ring caused dual-provider panic, breaking all TLS e2e tests
- TLS e2e test helper (`start_tls_server`) missing stdout/stderr pipe draining and telemetry config — pre-existing but exposed by this PR

### Incorrect Decisions During Development
- Admin opcodes assigned 0x20-0x39 (ascending) instead of growing downward from 0xFD — corrected during review to avoid future range collisions with hot-path opcodes
- Stream enum used manual method delegation instead of implementing AsyncRead/AsyncWrite traits
- Spawned connection tasks were fire-and-forget with no JoinSet tracking for graceful shutdown

### Deferred Work
- None — all review findings addressed

### Patterns for Future Stories
- When adding a new rustls-dependent crate alongside tonic, always install a CryptoProvider explicitly at startup
- Binary protocol server patterns (DynStream, JoinSet, ContinuationAssembler) established here should be reused in stories 20.2-20.5
- Admin opcodes grow downward from 0xFD — new admin opcodes get the next lower value

## PR #159: Admin Operations & Auth on Binary Protocol

### Gaps in Dev Process
- Multiple `as u16` truncation sites in encode methods would silently corrupt frames for large lists — same class of bug Cubic caught in stats, repeated across queues list, config entries, API keys, and ACL permissions
- No encode/decode trait formalized — each type had identical method signatures as inherent impls with no shared contract
- All 37 protocol types in a single 1919-line types.rs file

### Incorrect Decisions During Development
- `assert!` used for stats list length check instead of truncation — would panic the server in production
- types.rs not split into domain-specific modules

### Deferred Work
- u16 count limitation across all list types tracked in #164 — needs wire format change to u32

### Patterns for Future Stories
- All new protocol types must implement `ProtocolMessage` trait
- All u16 list counts must use `.min(u16::MAX)` + `tracing::warn!` pattern, never `as u16` directly
- Protocol types organized by domain: handshake.rs, hotpath.rs, admin.rs, auth.rs, error.rs
