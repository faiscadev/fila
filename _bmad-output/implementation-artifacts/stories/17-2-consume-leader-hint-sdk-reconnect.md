# Story 17.2: Consume Leader Hint & SDK Reconnect

Status: review

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

- [x] Task 1: Add client address tracking infrastructure (AC: 1)
  - [x] 1.1: GetNodeInfo cluster RPC returns node's client-facing gRPC address
  - [x] 1.2: client_addr field added to AddNodeRequest for join-time registration
  - [x] 1.3: node_id → client_addr mapping tracked in MultiRaftManager
  - [x] 1.4: ClusterHandle.get_queue_leader_client_addr() resolves leader's client addr
  - [x] 1.5: ClusterManager.start() takes client_listen_addr, registers own address

- [x] Task 2: Server-side leader hint on Consume (AC: 1)
  - [x] 2.1: In `service.rs` consume handler, when not leader: get leader client address
  - [x] 2.2: Return `Status::unavailable` with gRPC metadata `x-fila-leader-addr: <addr>`
  - [x] 2.3: If leader address unknown, UNAVAILABLE without hint (graceful degradation)

- [x] Task 3: Rust SDK transparent reconnect (AC: 2, 4)
  - [x] 3.1: extract_leader_hint() extracts `x-fila-leader-addr` from status metadata
  - [x] 3.2: If present, create new gRPC connection to leader addr and retry consume once
  - [x] 3.3: If redirect fails, return original error (no infinite loops)

- [x] Task 4: External SDK updates — Go (AC: 3)
  - [x] 4.1-4.3: PR fila-go#2

- [x] Task 5: External SDK updates — Python (AC: 3)
  - [x] 5.1-5.3: PR fila-python#2

- [x] Task 6: External SDK updates — JavaScript (AC: 3)
  - [x] 6.1-6.3: PR fila-js#2

- [x] Task 7: External SDK updates — Ruby (AC: 3)
  - [x] 7.1-7.3: PR fila-ruby#2

- [x] Task 8: External SDK updates — Java (AC: 3)
  - [x] 8.1-8.3: PR fila-java#2

- [x] Task 9: E2E test for consume leader redirect (AC: 5)
  - [x] 9.1: New test `cluster_consume_leader_redirect` in `crates/fila-e2e/tests/cluster.rs`
  - [x] 9.2: Connects consumer to non-leader, verifies SDK redirect, receives messages

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

None.

### Completion Notes List

- Used gRPC metadata (`x-fila-leader-addr`) on UNAVAILABLE status rather than modifying the ConsumeResponse proto. Standard gRPC pattern — no proto changes needed.
- Client address mapping uses a `HashMap<NodeId, String>` on `MultiRaftManager`. Populated via AddNodeRequest.client_addr on join, and register_client_addr at startup. Added GetNodeInfo RPC as fallback discovery.
- ClusterManager.start() now takes `client_listen_addr` parameter — breaking API change for callers. Updated main.rs and all test call sites.
- SDK reconnect uses http:// by default for leader URL. TLS clusters would need the SDK to preserve and reuse TLS config for the redirect connection — the Rust SDK currently doesn't (future improvement for TLS clusters).

### File List

- `crates/fila-proto/proto/fila/v1/cluster.proto` — MODIFIED: GetNodeInfo RPC, client_addr field on AddNodeRequest, GetNodeInfoRequest/Response messages
- `crates/fila-core/src/cluster/mod.rs` — MODIFIED: ClusterManager.start() takes client_listen_addr, get_queue_leader_client_addr(), register own client addr
- `crates/fila-core/src/cluster/multi_raft.rs` — MODIFIED: client_addrs HashMap, register_client_addr/get_client_addr methods
- `crates/fila-core/src/cluster/grpc_service.rs` — MODIFIED: GetNodeInfo handler, store client_addr from AddNode, node_id/client_addr fields
- `crates/fila-core/src/cluster/tests.rs` — MODIFIED: updated ClusterGrpcService::new calls
- `crates/fila-server/src/service.rs` — MODIFIED: x-fila-leader-addr metadata on consume non-leader error
- `crates/fila-server/src/main.rs` — MODIFIED: pass client_listen_addr to ClusterManager
- `crates/fila-sdk/src/client.rs` — MODIFIED: extract_leader_hint(), transparent reconnect in consume()
- `crates/fila-e2e/tests/cluster.rs` — MODIFIED: cluster_consume_leader_redirect test
- External PRs: fila-go#2, fila-python#2, fila-js#2, fila-ruby#2, fila-java#2
