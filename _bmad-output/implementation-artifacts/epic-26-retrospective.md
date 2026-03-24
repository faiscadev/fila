# Epic 26 Retrospective: SDK Batch Operations & Smart Batching

## Epic Summary

- **Stories:** 6/6 complete (100%)
- **PRs:** Rust #102 (fila), Go #3 (fila-go), Python #3 (fila-python), JS #3 (fila-js), Ruby #3 (fila-ruby), Java #3 (fila-java)
- **Blockers:** 0
- **Cubic findings:** 6 on initial Rust PR (all addressed), 1 P3 doc nitpick on final push
- **Test count:** 110+ new tests across 6 SDKs

## What Went Well

### Mid-story design pivot succeeded without disruption
Story 26.1 started with `BatchConfig` (linger_ms timer). During implementation, Lucas identified a better approach: opportunistic batching that adapts automatically. The pivot from timer-based `BatchConfig` to `BatchMode` with Auto/Linger/Disabled happened cleanly mid-story without restarting. The final design is simpler and smarter than planned.

### Concurrent agent execution for external SDKs
Stories 26.2-26.6 ran as 5 parallel agents, each working in a separate repo. All 5 completed successfully within ~8 minutes of each other. This is dramatically faster than sequential execution (which would have taken 5x as long).

### Zero-config smart batching design
The `BatchMode::Auto` algorithm (opportunistic drain + concurrent flush) is genuinely zero-config and zero-latency-penalty. It doesn't use timers, heuristics, or tuning parameters. The channel acts as a natural buffer, and the arrival rate is the implicit tuning knob. This is a significant improvement over the original linger_ms design.

### Cubic caught real bugs
Cubic identified the `zip` result-count mismatch bug in `flush_batch` that would have caused confusing errors when the server returned fewer results than messages sent. Also caught that the batch_size test was sending messages serially (defeating the purpose). Both were real issues, not noise.

## What Didn't Go Well

### BatchEnqueue proto returns string errors, losing type info
When `enqueue()` routes through `BatchEnqueue` for batched messages, per-message errors come back as strings (e.g., "no-such-queue") instead of structured gRPC status codes (e.g., `NOT_FOUND`). This forced a single-item optimization in `flush_batch` to use the regular `Enqueue` RPC for 1-message batches to preserve exact error semantics like `QueueNotFound`. The proto should return structured errors per message.

### External SDK PRs not verified by Cubic
Only the Rust SDK PR runs Cubic. The 5 external SDK repos don't have Cubic configured, so those PRs were merged without automated review. This is a known gap from Epic 16.

### Sprint-status story keys don't match the new BatchMode naming
The sprint-status keys were created before the design pivot (e.g., `26-1-rust-sdk-auto-batching-linger-ms-timer`). They now describe the wrong thing. Not blocking but confusing for future retrospectives.

## Action Items

1. **Proto enhancement (future):** Add structured error codes to `BatchEnqueueResult` (e.g., `oneof { EnqueueResponse success, EnqueueError error }` instead of `string error`). This would eliminate the single-item optimization hack.

2. **Encode concurrent SDK execution pattern:** The 5-agent concurrent execution pattern worked well. Future multi-SDK epics should always use parallel agents. Encode this in CLAUDE.md as a workflow recommendation.

3. **Memory update:** Record the BatchMode design decision and opportunistic batching algorithm for future reference.

## Metrics

- **Duration:** ~1 session (single conversation)
- **Design pivots:** 1 (BatchConfig → BatchMode, mid-story)
- **Cubic findings addressed:** 6/6 on Rust PR
- **Agent parallelism:** 5 concurrent agents for external SDKs
- **Previous retro action items completed:** N/A (first epic in this sprint)
