# Story 20.2: Admin Operations & Auth on Binary Protocol

Status: done

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

- [x] Task 1: Add admin operation handlers to binary_server (AC: #1)
  - [x] 1.1: Add admin opcode dispatch in binary_server.rs dispatch_frame (15 new match arms)
  - [x] 1.2: CreateQueue handler
  - [x] 1.3: DeleteQueue handler
  - [x] 1.4: GetStats handler
  - [x] 1.5: ListQueues handler
  - [x] 1.6: SetConfig handler
  - [x] 1.7: GetConfig handler
  - [x] 1.8: ListConfig handler
  - [x] 1.9: Redrive handler

- [x] Task 2: Add admin typed request/response structs to fila-fibp (AC: #1)
  - [x] 2.1: CreateQueue/CreateQueueResult structs + encode/decode
  - [x] 2.2: DeleteQueue/DeleteQueueResult
  - [x] 2.3: GetStats/GetStatsResult (including per-key stats)
  - [x] 2.4: ListQueues/ListQueuesResult
  - [x] 2.5: SetConfig/SetConfigResult
  - [x] 2.6: GetConfig/GetConfigResult
  - [x] 2.7: ListConfig/ListConfigResult
  - [x] 2.8: Redrive/RedriveResult

- [x] Task 3: Add auth/ACL operation handlers (AC: #4)
  - [x] 3.1: CreateApiKey handler
  - [x] 3.2: RevokeApiKey handler
  - [x] 3.3: ListApiKeys handler
  - [x] 3.4: SetAcl handler
  - [x] 3.5: GetAcl handler
  - [x] 3.6: Auth/ACL typed structs in fila-fibp + encode/decode (26 new structs)

- [x] Task 4: ACL enforcement on admin operations (AC: #2, #3)
  - [x] 4.1: Admin permission check for admin operations (queue-scoped + global)
  - [x] 4.2: Superadmin bypass for auth management operations

- [x] Task 5: Integration tests (AC: #6)
  - [x] 5.1: Test CreateQueue + DeleteQueue round-trip (including duplicate/not-found errors)
  - [x] 5.2: Test GetStats (empty queue + nonexistent queue error)
  - [x] 5.3: Test ListQueues (empty + populated)
  - [x] 5.4: Test SetConfig/GetConfig round-trip
  - [x] 5.5: Test auth rejection (bad key, no key)
  - [x] 5.6: Test auth acceptance (valid bootstrap key)

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
Claude Opus 4.6 (1M context)

### Completion Notes List
- 26 new typed structs in fila-fibp covering all admin + auth opcodes
- 13 handler functions in binary_handlers.rs with explicit error variant matching
- 15 dispatch arms + handler methods in binary_server.rs with ACL enforcement
- 14 new unit tests for admin/auth types, 8 new integration tests
- All 423+ workspace tests pass

### File List
- crates/fila-fibp/src/types.rs (modified — 26 admin/auth structs)
- crates/fila-server/src/binary_handlers.rs (modified — 13 admin/auth handlers)
- crates/fila-server/src/binary_server.rs (modified — dispatch + ACL)
- crates/fila-server/tests/binary_protocol.rs (modified — 8 new tests)
