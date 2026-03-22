# Story 17.2: Consume Leader Hint & SDK Reconnect

Status: ready-for-dev

## Story

As a consumer,
I want to connect to any cluster node and be transparently routed to the queue's leader,
so that I don't need to know which node leads which queue.

## Acceptance Criteria

1. **Given** a client calls `Consume` on a node that is not the Raft leader for the requested queue
   **When** the server detects the non-leader condition
   **Then** the server returns an error with the leader's client address (e.g., `NOT_LEADER { leader_addr: "node2:5555" }`)
   **And** the leader address is conveyed via gRPC metadata key `x-fila-leader-addr` on the UNAVAILABLE status

2. **Given** the Rust SDK (`fila-sdk`) receives a consume UNAVAILABLE error with leader hint metadata
   **When** the SDK detects the `x-fila-leader-addr` metadata
   **Then** the SDK reconnects to the hinted leader and retries the consume transparently
   **And** the consumer receives messages after redirect

3. **Given** all 5 external SDKs (Go, Python, JS, Ruby, Java) receive a consume UNAVAILABLE error with leader hint
   **When** each SDK detects the `x-fila-leader-addr` metadata
   **Then** each SDK reconnects to the hinted leader and retries the consume transparently

4. **Given** the hinted leader is unavailable
   **When** the SDK attempts to connect to the hinted address
   **Then** the SDK falls back to the original error (no infinite redirect loops)
   **And** a maximum of 1 redirect attempt is made per consume call

5. **Given** the cluster e2e test suite
   **When** a consume-on-non-leader test executes
   **Then** the test verifies: consumer connects to non-leader, gets redirected to leader, receives messages successfully

## Tasks / Subtasks

- [ ] Task 1: Add `get_queue_leader_client_addr` to ClusterHandle (AC: 1)
  - [ ] 1.1: New method on ClusterHandle that returns the leader's client address (not cluster address) for a queue
  - [ ] 1.2: Use `raft.metrics()` membership to look up leader node → resolve to client listen address
  - [ ] 1.3: Need a mapping from node_id → client address (cluster address != client address)

- [ ] Task 2: Server-side leader hint on Consume (AC: 1)
  - [ ] 2.1: In `service.rs` consume handler, when not leader: get leader client address
  - [ ] 2.2: Return `Status::unavailable` with gRPC metadata `x-fila-leader-addr: <addr>`
  - [ ] 2.3: Handle case where leader address is unknown (no hint, just UNAVAILABLE)

- [ ] Task 3: Rust SDK transparent reconnect (AC: 2, 4)
  - [ ] 3.1: In `fila-sdk` consume error handling, extract `x-fila-leader-addr` from status metadata
  - [ ] 3.2: If present, create new connection to leader addr and retry consume once
  - [ ] 3.3: If redirect fails, return original error (no infinite loops)
  - [ ] 3.4: Add `ConsumeError::LeaderRedirect` variant for visibility

- [ ] Task 4: External SDK updates — Go (AC: 3)
  - [ ] 4.1: Extract leader hint from gRPC status metadata in consume path
  - [ ] 4.2: Reconnect and retry transparently (max 1 redirect)
  - [ ] 4.3: Open PR in fila-go repo

- [ ] Task 5: External SDK updates — Python (AC: 3)
  - [ ] 5.1: Extract leader hint from gRPC status trailing metadata
  - [ ] 5.2: Reconnect and retry transparently
  - [ ] 5.3: Open PR in fila-python repo

- [ ] Task 6: External SDK updates — JavaScript (AC: 3)
  - [ ] 6.1: Extract leader hint from gRPC metadata
  - [ ] 6.2: Reconnect and retry transparently
  - [ ] 6.3: Open PR in fila-js repo

- [ ] Task 7: External SDK updates — Ruby (AC: 3)
  - [ ] 7.1: Extract leader hint from gRPC metadata
  - [ ] 7.2: Reconnect and retry transparently
  - [ ] 7.3: Open PR in fila-ruby repo

- [ ] Task 8: External SDK updates — Java (AC: 3)
  - [ ] 8.1: Extract leader hint from gRPC metadata
  - [ ] 8.2: Reconnect and retry transparently
  - [ ] 8.3: Open PR in fila-java repo

- [ ] Task 9: E2E test for consume leader redirect (AC: 5)
  - [ ] 9.1: New test in `crates/fila-e2e/tests/cluster.rs` using TestCluster
  - [ ] 9.2: Connect consumer to non-leader, verify redirect, verify messages received

## Dev Notes

### Architecture Context

**Current consume behavior in cluster mode** (`crates/fila-server/src/service.rs` lines 218-230):
- Checks `cluster.is_queue_leader(queue_id)` before registering consumer
- Non-leader: returns `Status::unavailable("this node is not the leader for this queue; reconnect to the leader")`
- No leader address included — client has to guess

**Leader address resolution challenge:**
- `ClusterHandle.is_queue_leader()` returns `Option<bool>` but not the leader address
- Cluster membership stores node addresses in cluster gRPC format (port 5556), not client format (port 5555)
- Need a node_id → client_listen_addr mapping. Options:
  1. Store client listen addr in the Raft membership's `BasicNode.addr` alongside cluster addr
  2. Add a separate mapping (node_id → client_addr) in ClusterConfig or shared state
  3. Derive client addr from cluster addr by port offset (fragile, don't do this)

**Recommended approach**: Store client addresses in a shared map populated at startup. Each node knows its own client address from config. On cluster join, include client address in the AddNode request or membership metadata.

### gRPC Metadata Pattern

Use trailing metadata on the Status error (standard gRPC pattern for service mesh routing):
```rust
let mut status = Status::unavailable("not the leader for this queue");
if let Some(addr) = leader_client_addr {
    status.metadata_mut().insert("x-fila-leader-addr", addr.parse().unwrap());
}
return Err(status);
```

SDKs extract via:
- Rust: `status.metadata().get("x-fila-leader-addr")`
- Go: `status.Trailer()["x-fila-leader-addr"]` or `grpc.Header()`
- Python: `error.trailing_metadata()`
- JS: `error.metadata.get("x-fila-leader-addr")`

### SDK Reconnect Pattern

Each SDK should:
1. Catch UNAVAILABLE error from consume
2. Check for `x-fila-leader-addr` metadata
3. If present: create new gRPC channel to that address, retry consume once
4. If retry fails: return the retry error (not the original)
5. Max 1 redirect per consume call — no loops

### Existing Patterns to Follow

- Rust SDK consume: `crates/fila-sdk/src/client.rs` lines 212-245
- SDK error types: `crates/fila-sdk/src/error.rs` — per-operation errors
- ClusterHandle methods: `crates/fila-core/src/cluster/mod.rs` — `is_queue_leader`, `meta_members`
- Cluster e2e tests: `crates/fila-e2e/tests/cluster.rs` — `TestCluster` helper
- External SDK patterns: each has a `client.{lang}` with consume method + error handling

### External SDK Repos (per CLAUDE.md: must use PRs, not direct push)

- Go: fila-go
- Python: fila-python
- JavaScript: fila-js
- Ruby: fila-ruby
- Java: fila-java

### References

- [Source: crates/fila-server/src/service.rs:218-230] — current consume non-leader handling
- [Source: crates/fila-core/src/cluster/mod.rs:169-176] — is_queue_leader, meta_members
- [Source: crates/fila-sdk/src/client.rs:212-245] — SDK consume method
- [Source: crates/fila-sdk/src/error.rs:89-95] — consume_status_error mapping
- [Source: crates/fila-e2e/tests/helpers/cluster.rs] — TestCluster helper
- [Source: crates/fila-e2e/tests/cluster.rs] — existing cluster e2e tests
- [GitHub: #65] — Consume on non-leader returns leader hint

## Dev Agent Record

### Agent Model Used

Claude Opus 4.6 (1M context)

### Debug Log References

### Completion Notes List

### File List
