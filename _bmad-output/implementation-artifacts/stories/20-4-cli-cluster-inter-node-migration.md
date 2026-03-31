# Story 20.4: CLI & Cluster Inter-Node Migration

Status: ready-for-dev

## Story

As an operator,
I want the CLI and cluster inter-node communication to use the binary protocol,
so that all Fila communication uses a single transport.

## Acceptance Criteria

1. **Given** fila-cli, **when** updated to use fila-sdk, **then** all CLI commands work over binary protocol.
2. **Given** fila-cli, **when** `--tls` and `--api-key` flags are used, **then** they work the same as before.
3. **Given** cluster nodes, **when** inter-node communication is migrated, **then** Raft log replication, leader forwarding, and queue group management use the binary protocol.
4. **Given** cluster nodes with TLS, **when** inter-node mTLS is configured, **then** it works over binary protocol.
5. **Given** cluster e2e tests, **when** they run, **then** they pass.
6. **Given** single-node e2e tests, **when** they run over binary protocol, **then** they pass.

## Tasks / Subtasks

- [ ] Task 1: Migrate CLI to use fila-sdk (AC: #1, #2)
  - [ ] 1.1: Replace tonic/fila-proto deps with fila-sdk + fila-fibp
  - [ ] 1.2: Rewrite connect logic to use ConnectOptions (binary protocol)
  - [ ] 1.3: Rewrite each command to use SDK or direct binary protocol frames
  - [ ] 1.4: Preserve --tls, --api-key, --tls-ca-cert, --tls-cert, --tls-key flags

- [ ] Task 2: Add cluster opcodes to fila-fibp (AC: #3)
  - [ ] 2.1: Define cluster opcodes 0x40-0x4F for Raft RPCs
  - [ ] 2.2: Cluster frame types — carry protobuf-serialized Raft data in binary frames

- [ ] Task 3: Cluster network layer migration (AC: #3, #4)
  - [ ] 3.1: Replace FilaNetwork tonic client with binary protocol TCP client
  - [ ] 3.2: Replace ClusterGrpcService with binary protocol handler
  - [ ] 3.3: Update ClusterHandle forward_client_write to use binary protocol
  - [ ] 3.4: Update ClusterManager to start binary protocol listener for cluster port

- [ ] Task 4: Verify tests pass (AC: #5, #6)
  - [ ] 4.1: All single-node e2e tests pass
  - [ ] 4.2: All cluster e2e tests pass

## Dev Notes

### CLI Approach
Replace direct gRPC calls with a thin wrapper around fila-fibp binary protocol frames. The CLI sends admin frames directly — no need to use the full SDK since the CLI only does admin operations (no consume streaming).

### Cluster Approach
Transport Raft RPCs over binary protocol frames. The Raft entry payloads stay protobuf-serialized (openraft uses serde for entries, proto_convert.rs converts). The frames carry protobuf bytes as opaque payloads.

### References
- [Source: crates/fila-cli/src/main.rs] — Current CLI
- [Source: crates/fila-core/src/cluster/network.rs] — Raft network layer
- [Source: crates/fila-core/src/cluster/grpc_service.rs] — Cluster service
- [Source: crates/fila-core/src/cluster/mod.rs] — ClusterManager, ClusterHandle

## Dev Agent Record
### Agent Model Used
### Completion Notes List
### File List
