# Story 20.2: Admin Operations & Auth on Binary Protocol

Status: ready-for-dev

## Story

As an operator,
I want admin operations and authentication to work over the binary protocol,
so that all Fila functionality is available on a single transport.

## Acceptance Criteria

1. **Given** the binary protocol server from Story 20.1, **when** admin opcodes are implemented (CreateQueue, DeleteQueue, SetConfig, GetConfig, GetStats, ListQueues, ListConfig, Redrive), **then** all admin operations work over the binary protocol with the same semantics as gRPC.

2. **Given** the binary protocol server, **when** API key authentication is enforced, **then** the key sent during handshake is validated per-request.

3. **Given** the binary protocol server, **when** per-queue ACLs are configured, **then** they are enforced identically to gRPC (Produce/Consume/Admin permissions, glob patterns, superadmin bypass).

4. **Given** the binary protocol server, **when** auth-management opcodes are implemented (CreateApiKey, RevokeApiKey, ListApiKeys, SetAcl, GetAcl), **then** API key and ACL management works over the binary protocol.

5. **Given** the binary protocol server, **when** mTLS mutual authentication is configured, **then** it works on binary protocol connections (already implemented in 20.1 via tokio-rustls).

6. **Given** the binary protocol server, **when** integration tests run, **then** admin ops, auth rejection (bad key, missing key, insufficient permissions), and mTLS are verified.

## Tasks / Subtasks

- [ ] Task 1: Add admin operation handlers to binary_server (AC: #1)
  - [ ] 1.1: Add admin opcode dispatch in binary_server.rs dispatch_frame
  - [ ] 1.2: CreateQueue handler — decode, send SchedulerCommand::CreateQueue, encode result
  - [ ] 1.3: DeleteQueue handler
  - [ ] 1.4: GetStats handler
  - [ ] 1.5: ListQueues handler
  - [ ] 1.6: SetConfig handler
  - [ ] 1.7: GetConfig handler
  - [ ] 1.8: ListConfig handler
  - [ ] 1.9: Redrive handler

- [ ] Task 2: Add admin typed request/response structs to fila-fibp (AC: #1)
  - [ ] 2.1: CreateQueue/CreateQueueResult structs + encode/decode
  - [ ] 2.2: DeleteQueue/DeleteQueueResult
  - [ ] 2.3: GetStats/GetStatsResult (including per-key stats)
  - [ ] 2.4: ListQueues/ListQueuesResult
  - [ ] 2.5: SetConfig/SetConfigResult
  - [ ] 2.6: GetConfig/GetConfigResult
  - [ ] 2.7: ListConfig/ListConfigResult
  - [ ] 2.8: Redrive/RedriveResult

- [ ] Task 3: Add auth/ACL operation handlers (AC: #4)
  - [ ] 3.1: CreateApiKey handler
  - [ ] 3.2: RevokeApiKey handler
  - [ ] 3.3: ListApiKeys handler
  - [ ] 3.4: SetAcl handler
  - [ ] 3.5: GetAcl handler
  - [ ] 3.6: Auth/ACL typed structs in fila-fibp + encode/decode

- [ ] Task 4: ACL enforcement on admin operations (AC: #2, #3)
  - [ ] 4.1: Admin permission check for admin operations (CreateQueue, DeleteQueue, etc.)
  - [ ] 4.2: Superadmin bypass for auth management operations

- [ ] Task 5: Integration tests (AC: #6)
  - [ ] 5.1: Test CreateQueue + DeleteQueue round-trip
  - [ ] 5.2: Test GetStats, ListQueues
  - [ ] 5.3: Test SetConfig/GetConfig/ListConfig
  - [ ] 5.4: Test Redrive
  - [ ] 5.5: Test auth rejection (no key, bad key)
  - [ ] 5.6: Test ACL enforcement (forbidden operation)

## Dev Notes

### Key Files

**Modify:**
- `crates/fila-fibp/src/types.rs` — add admin + auth typed structs
- `crates/fila-server/src/binary_server.rs` — dispatch admin opcodes
- `crates/fila-server/src/binary_handlers.rs` — add admin + auth handlers
- `crates/fila-server/tests/binary_protocol.rs` — add admin + auth tests

### Existing Admin Patterns

The gRPC admin_service.rs (crates/fila-server/src/admin_service.rs) provides the reference implementation. Each admin handler follows the same pattern:
1. ACL check (admin permission on queue or superadmin)
2. Send SchedulerCommand to broker
3. Await oneshot reply
4. Map result to response

### Auth API Key Management

The Broker exposes these methods for auth:
- `broker.create_api_key(name, expires_at, is_superadmin)` → `(key_id, plaintext_key)`
- `broker.revoke_api_key(key_id)` → `Result`
- `broker.list_api_keys()` → `Vec<ApiKeyInfo>`
- `broker.set_acl(key_id, permissions)` → `Result`
- `broker.get_acl(key_id)` → `AclInfo`

These are direct broker methods, not SchedulerCommand variants.

### Protocol Spec Reference

Admin frame formats are in docs/protocol.md §Admin Operation Frames and §Auth & ACL Operation Frames.

### References

- [Source: crates/fila-server/src/admin_service.rs] — gRPC admin reference implementation
- [Source: crates/fila-server/src/binary_server.rs] — binary server to extend
- [Source: docs/protocol.md] — admin frame formats

## Dev Agent Record

### Agent Model Used

### Completion Notes List

### File List
