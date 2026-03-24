# Epic 29 Retrospective: Transport Optimization — Fair Benchmarks & Streaming Enqueue

**Date:** 2026-03-24
**Stories Completed:** 3/3 (100%)
**PRs:** #109 (29.1), #110 (29.2), #111 (29.3) — all merged to main
**Cubic Findings:** 7 total (2 on #109, 4 on #110, 1 on #111) — all addressed
**Test Suite:** 508 -> 534 tests (26 new), zero regressions

---

## Epic Summary

Epic 29 addressed the #1 bottleneck identified by post-optimization profiling: 94% of per-message time (~120us) was gRPC/HTTP2 transport overhead. Three stories: fix the competitive benchmark to use batching (fair comparison), add bidirectional streaming `StreamEnqueue` RPC to the server, and integrate streaming transparently into the Rust SDK's auto-batcher.

| Story | PR | Scope |
|-------|-----|-------|
| 29.1: Fair Competitive Benchmark | #109 | Updated `bench-competitive.rs` to use `BatchMode::Auto` with 4 concurrent producers for throughput scenarios; lifecycle stays unbatched |
| 29.2: Streaming Enqueue RPC | #110 | New `StreamEnqueue` bidirectional streaming RPC, server-side handler with write coalescing, 4 integration tests |
| 29.3: SDK Streaming Integration | #111 | SDK auto-batcher uses streaming transparently, lazy stream open, reconnection, `UNIMPLEMENTED` fallback, 5+ integration tests |

---

## What Went Well

### Profiling-driven epic delivered targeted results
Epic 29 was planned directly from profiling data (`post-optimization-profiling-analysis-2026-03-24.md`). Every story addressed a specific, measured bottleneck. No wasted work on theoretical optimizations. This validates the "profile-first" rule added in the Epic 22-24 retro.

### Clean decomposition: server then SDK
Splitting streaming into server-side (29.2) then SDK integration (29.3) was the right call. The server RPC was testable independently via raw gRPC stream tests, and the SDK could build on a known-working server. Each PR was reviewable in isolation.

### Backward compatibility preserved throughout
- `BatchMode::Disabled` still uses unary RPCs (no streaming)
- `batch_enqueue()` (explicit manual batch) still uses unary `BatchEnqueue`
- SDK gracefully falls back to unary when server doesn't support `StreamEnqueue` (UNIMPLEMENTED detection)
- Existing unary `Enqueue` and `BatchEnqueue` RPCs unchanged

### Server-side architecture: extracted standalone function
Story 29.2 extracted `enqueue_single_standalone()` as a free async function (no `&self`) callable from spawned tasks. This pattern avoids lifetime issues with tokio spawns holding references to the service struct. Clean separation.

### Zero debug struggles
All 3 stories shipped without debug log entries. The profiling research provided clear direction, and the implementation patterns (write coalescing, bidirectional streaming, sequence number correlation) were well-understood before coding started.

---

## What Didn't Go Well

### Benchmark validation deferred across stories
Story 29.1 deferred end-to-end benchmark validation (Tasks 4.1, 5.2 — require Docker + all broker containers). Story 29.2 deferred benchmark comparison (AC #12 — streaming vs unary throughput). Story 29.3 deferred benchmark validation (AC #10 — >= 25K msg/s target). None of the NFR targets (NFR-T1: within 3x of Kafka, NFR-T2: >= 30K msg/s streaming) have been validated. This is the same pattern from Epics 22-24 where all benchmarks were deferred.

### Streaming enqueue is Rust SDK only
Like batch operations before Epic 26, streaming enqueue exists only in the Rust SDK. The 5 external SDKs (Go, Python, JS, Ruby, Java) don't have `StreamEnqueue` support. This creates the same feature gap that motivated Epic 26.

### Story 29.3 task checkboxes not updated
The story spec file still shows all tasks as `[ ]` (unchecked) despite being complete. The dev agent completed the work but didn't update the task tracking in the story file. Minor bookkeeping gap.

### No story spec for Epic 29 in planning artifacts
The epic spec is in `transport-optimization-epics.md` but there's no entry in the main `epics.md` file. Epic 29 exists only in sprint-status.yaml and the transport-specific file. Future agents looking at the top-level epic index won't find it.

---

## Cubic Findings Summary

| PR | Findings | Severity | Examples |
|----|----------|----------|----------|
| #109 | 2 | Low | Warmup doc inconsistency, tracking |
| #110 | 4 | Mixed | stdout drain, post-close assertion, queue-not-found assertion, tracking |
| #111 | 1 | Low | Tracking |

All 7 findings addressed before merge. The PR #110 findings were the most substantive — catching potential assertion failures in edge cases (post-close behavior, queue-not-found mid-stream).

---

## Previous Retro Action Item Follow-Through

The most recent relevant retro is the Epic 22-24 combined retro.

| Action Item | Status | Evidence |
|-------------|--------|----------|
| Epic 26: SDK Batch Operations | Done | Epic 26 completed (6/6 stories) |
| Epic 27: Profiling Infrastructure | Done | Epic 27 completed (3/3 stories) |
| Profile-first rule in CLAUDE.md | Done | Section exists in CLAUDE.md |
| Trim CLAUDE.md (remove stale sections) | Done | Completed in E22-24 retro |
| Accept memory/disk regression | Done | Recorded in memory file |

**Score: 5/5 (100%)** — encode-or-drop continues at 100%.

Epic 26 retro action items:

| Action Item | Status | Evidence |
|-------------|--------|----------|
| Proto enhancement (structured BatchEnqueueResult errors) | Not done | Still returns string errors. Not blocking but tech debt. |
| Encode concurrent SDK execution pattern | Done | CLAUDE.md "Multi-SDK Epics" section |
| Memory update for BatchMode design | Done | In memory file |

**Score: 2/3 (67%)** — proto enhancement is a future item, not blocking.

---

## Action Items (Encode or Drop)

| # | Action | Decision | Rationale |
|---|--------|----------|-----------|
| 1 | **Benchmark validation needed** — NFR targets (30K msg/s streaming, within 3x of Kafka) are unvalidated. Run competitive + streaming benchmarks. | **Encode** — add note to transport-optimization-epics.md that benchmark validation is pending | Cannot claim transport gap is closed without numbers |
| 2 | **External SDK streaming support** — 5 SDKs lack StreamEnqueue. Similar to Epic 26 gap. | **Drop** — do not create a new epic yet | Streaming is an optimization, not a feature gap. SDKs still work via unary RPCs. Only worth adding if/when a user needs it. |
| 3 | **Epic 29 missing from epics.md** — top-level index doesn't reference Epic 29 | **Encode** — add Epic 29 entry to epics.md | Consistency with all other epics |
| 4 | **Update memory file** — record Epic 29 results and streaming architecture | **Encode** — update MEMORY.md | Future agent context |

---

## Key Takeaways

1. **Profile-first rule works.** Epic 29 was the first epic planned entirely from profiling data. Every story addressed a measured bottleneck. Zero wasted work, zero surprises. The rule added in the E22-24 retro paid off immediately.

2. **Benchmark deferral is a recurring pattern.** Epics 22-24 deferred all benchmarks. Epic 29 did the same. The "profile before optimizing" rule exists, but there's no complementary rule for "validate after optimizing." The benchmarks require Docker + broker containers, which makes them hard to run in the dev agent flow, but the numbers are what prove the optimization worked.

3. **Streaming is a transport concern, not a feature concern.** Unlike batch operations (which change the API surface), streaming is transparent to users. The SDK falls back gracefully. External SDK streaming support is nice-to-have, not a gap.

---

## Sprint Status Update

```yaml
epic-29-retrospective: done
```
