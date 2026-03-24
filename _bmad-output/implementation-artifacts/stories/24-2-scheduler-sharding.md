# Story 24.2: Scheduler Sharding

Status: in-progress

## Story

As a Fila operator running high-throughput multi-queue workloads,
I want the scheduler to shard across multiple threads,
So that multi-queue parallelism breaks the single-threaded bottleneck.

## Acceptance Criteria

1. **Given** `scheduler.shard_count` is set to N in fila.toml
   **When** the broker starts
   **Then** N scheduler threads are spawned, each with its own crossbeam channel and DRR state

2. **Given** `shard_count` defaults to 1
   **When** no configuration override is set
   **Then** behavior is identical to the pre-sharding single-scheduler architecture

3. **Given** a queue name
   **When** a command targeting that queue is sent
   **Then** consistent hashing routes it to the same shard every time, preserving per-queue ordering

4. **Given** ListQueues or GetStats (all queues) is requested
   **When** multiple shards exist
   **Then** the response aggregates results from all shards

5. **And** all existing tests pass with zero regressions (shard_count=1 default)

## Tasks / Subtasks

1. Add `shard_count` field to `SchedulerConfig` (default: 1)
2. Create `ShardRouter` for queue-to-shard consistent hashing
3. Modify `Broker` to spawn N scheduler threads via `ShardRouter`
4. Route queue-targeted commands through `ShardRouter::route(queue_id)`
5. Aggregate cross-shard responses for ListQueues and GetStats
6. Tests: config parsing, routing consistency, shard_count=1 regression, multi-shard integration
