# Story 18.1: Broker-Managed Consumer Groups

Status: in-progress

## Story

As a developer building multi-instance consumer services,
I want to specify a consumer group when consuming messages,
so that the broker distributes messages across group members (each message to exactly one member) without requiring external coordination.

## Acceptance Criteria

1. **Given** a consumer calls Consume with `consumer_group = "my-group"` **Then** it joins the named group for that queue.
2. **Given** 3 consumers in the same group consuming from a queue with 9 messages **Then** each consumer receives ~3 messages (round-robin within group).
3. **Given** the broker tracks group membership **Then** it knows which consumers belong to which group for which queue.
4. **Given** messages are distributed across group members **Then** each message goes to exactly one member — no duplicates.
5. **Given** DRR fair scheduling selects a message for delivery **Then** if the target is a group, one member receives it via round-robin. Groups receive fairly-scheduled messages, then round-robin within group.
6. **Given** a consumer joins or leaves a group **Then** rebalancing happens automatically — remaining members absorb the departed member's share.
7. **Given** in-flight messages when a consumer disconnects **Then** they are governed by visibility timeout (existing mechanism) — no message loss.
8. **Given** a consumer calls Consume without `consumer_group` (empty string) **Then** it operates independently, same as before (backward compatible).
9. **Given** cluster mode with Raft-per-queue **Then** consumer groups work because consumers connect to the leader node, and group state is in the scheduler (leader-local).
10. **Given** an operator calls `GetConsumerGroups` admin RPC **Then** they see all active groups with queue, group name, member count, and member IDs.
11. **Given** mixed consumers (some in groups, some independent) on the same queue **Then** the group counts as one delivery target and the independent consumer as another, with round-robin across targets.

## Tasks / Subtasks

- [x] Task 1: Proto changes (backward compatible)
  - [x] 1.1: Add `string consumer_group = 2` to `ConsumeRequest` in service.proto
  - [x] 1.2: Add `GetConsumerGroups` RPC to admin.proto with request/response messages
  - [x] 1.3: Add `ConsumerGroupInfo`, `ConsumerGroupMember` proto messages

- [x] Task 2: Core consumer group manager
  - [x] 2.1: Create `ConsumerGroupManager` in `scheduler/consumer_groups.rs`
  - [x] 2.2: Implement join/leave/next_member/group_members/all_groups/remove_queue
  - [x] 2.3: Unit tests for group manager (5 tests)

- [x] Task 3: Scheduler integration
  - [x] 3.1: Add `consumer_group: Option<String>` to `ConsumerEntry` and `RegisterConsumer` command
  - [x] 3.2: Add `GetConsumerGroups` command variant to `SchedulerCommand`
  - [x] 3.3: Add `ConsumerGroupsError` per-command error type
  - [x] 3.4: Wire consumer group join/leave in handle_command for Register/Unregister
  - [x] 3.5: Wire GetConsumerGroups command handler
  - [x] 3.6: Clean up consumer groups on `drop_queue_consumers` (leader loss)

- [x] Task 4: Delivery logic changes
  - [x] 4.1: Refactor `try_deliver_to_consumer` to use delivery targets (Independent / Group)
  - [x] 4.2: Group target resolves to one member via ConsumerGroupManager.next_member()
  - [x] 4.3: Fallback to next group member if channel full/closed
  - [x] 4.4: Independent consumers work exactly as before

- [x] Task 5: gRPC service layer
  - [x] 5.1: Pass `consumer_group` from `ConsumeRequest` to `RegisterConsumer` command in service.rs
  - [x] 5.2: Implement `GetConsumerGroups` handler in admin_service.rs
  - [x] 5.3: Add `IntoStatus` for `ConsumerGroupsError` in error.rs

- [x] Task 6: SDK update
  - [x] 6.1: Add `consume_group(queue, group)` method to Rust SDK client
  - [x] 6.2: Existing `consume(queue)` unchanged (backward compatible)

- [x] Task 7: Integration tests (7 tests)
  - [x] 7.1: 3 consumers in group each get ~33% of 9 messages
  - [x] 7.2: Consumer leaves group, remaining 2 get ~50% each
  - [x] 7.3: No consumer group = independent (backward compat)
  - [x] 7.4: Mixed group + independent consumers
  - [x] 7.5: GetConsumerGroups returns correct info
  - [x] 7.6: Single-member group gets all messages
  - [x] 7.7: Multiple groups on same queue

- [x] Task 8: Update all existing RegisterConsumer call sites with `consumer_group: None`

## Technical Notes

- Consumer groups are a scheduler-local concept (in-memory). No Raft replication needed since consumers always connect to the Raft leader for their queue.
- The `ConsumerGroupManager` tracks (queue_id, group_name) -> members with per-group round-robin state.
- Delivery targets: each group counts as one target, each independent consumer counts as one target. Round-robin across targets. Within a group, internal round-robin.
- Session timeout for disconnected consumers handled by existing `UnregisterConsumer` on stream close + visibility timeout for in-flight messages.
