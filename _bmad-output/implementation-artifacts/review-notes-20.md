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

## PR #160: Rust SDK Binary Protocol Client

### Gaps in Dev Process
- SDK consume() blocked forever waiting for initial response — protocol spec had no ConsumeOk confirmation opcode
- Background reader task leaked on client drop — no cancellation mechanism
- Dropping consume stream didn't send CancelConsume to server, leaving dead consumers registered
- Delivery channel registered after ConsumeOk created race with early deliveries
- redirect_consume dropped temporary client, killing background reader before stream could receive deliveries
- SDK integration test server didn't drain stdout pipe (same bug as e2e TLS tests)
- Server advertised gRPC address for cluster leader redirects instead of binary protocol address

### Incorrect Decisions During Development
- No ConsumeOk opcode in protocol spec — consume was fire-and-forget with no confirmation
- IoStream/ReadStream/WriteStream used manual method delegation instead of AsyncRead/AsyncWrite traits
- FilaClient stored fields in individual Arcs instead of a single Arc<Inner>
- reader_task stored as Arc<JoinHandle> with _ prefix — served no purpose

### Deferred Work
- 3 cluster e2e tests fail: binary server lacks leader forwarding for writes (story 20.4 scope)

### Patterns for Future Stories
- ConsumeOk (0x13) must be sent before any Delivery frames
- ConsumeStream must hold Arc<FilaClientInner> to keep connection alive
- ConsumeStream Drop sends CancelConsume to clean up server-side consumer
- Delivery channel registered before Consume request with save/restore on failure
- Every server spawn in tests must drain BOTH stdout and stderr pipes

## PR #161: CLI & Cluster Inter-Node Migration

### Gaps in Dev Process
- Binary server admin handlers had no cluster awareness — create/delete queue bypassed Raft entirely
- Binary server hot-path handlers had no write forwarding — enqueue/ack/nack went to local scheduler only
- CLI migrated to binary protocol but all test helpers still used gRPC addresses
- SDK delivery channel deadlock: background reader blocked on send.await, starving response frames
- Double-consume hang: dropping a stream didn't cancel server-side consumer, second consume saw stale deliveries
- Benchmark fairness crash was actually the delivery deadlock manifesting under load

### Incorrect Decisions During Development
- Stream enum in CLI used manual method delegation (same mistake as server and SDK — third time)
- Bare .unwrap() in mTLS cert loading path
- Error description leaked into message_id field in cluster enqueue error path
- ACL permission kinds not validated in CLI

### Deferred Work
- Benchmark CI issue tracked in #166 (was actually fixed by the delivery overflow buffer)

### Patterns for Future Stories
- SDK delivery overflow buffer: try_send to channel, overflow to VecDeque, TCP reads always continue
- Response frames (oneshots) and delivery frames (mpsc) must never share blocking paths
- Server drains all existing consumers when new Consume arrives on same connection
- consume() cancels existing subscription inline before starting new one
- All CLI/test code must use binary_addr, not gRPC addr
