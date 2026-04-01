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
