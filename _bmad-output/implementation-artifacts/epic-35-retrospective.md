# Epic 35 Retrospective: Phase 1 — gRPC Streaming & Batch Tuning

**Date:** 2026-03-26
**Stories:** 4/4 completed (100%)
**PRs:** #130, #131, #132, #133 — all merged

---

## What Went Well

1. **SDK batch consolidation was the headline win** — Story 35.2 delivered +75.5% improvement for auto-batch mode by sending all accumulated messages in a single StreamEnqueueRequest. The proto already supported it; only the SDK needed changes.

2. **Clean parallel execution** — Stories 35.2 and 35.3 were independent (both depended only on 35.1). Both developed in parallel and merged cleanly.

3. **Benchmark-driven decisions** — Story 35.1 established a verified baseline. Story 35.4 measured actual impact. No guessing.

4. **Zero Cubic findings** — All 4 PRs passed Cubic review with 0 issues.

5. **Pre-existing format issue caught** — CI Format check failed on PRs due to a rustfmt difference between local and CI. Fixed once on 35.1, then again on 35.2 (different branch didn't have the fix). Not a regression.

## What Didn't Go Well

1. **gRPC tuning yielded only 2-5% single-producer improvement** — Window size and frame size increases had modest impact. The bottleneck is structural HTTP/2 overhead (HPACK, stream state), not flow control settings.

2. **Benchmark CI (bench-regression.yml) fails on all PRs** — Missing `gh-pages` branch for storing results. This is a pre-existing infrastructure gap, not a code issue.

3. **Merge conflicts on tracking files** — Each branch modified sprint-status.yaml independently, causing conflicts during the review/merge phase. Expected with parallel story branches.

## Action Items

| # | Action | Encoded In | Status |
|---|--------|-----------|--------|
| 1 | Fix bench-regression.yml gh-pages branch dependency | Deferred — CI infrastructure issue, not blocking | Deferred |
| 2 | Consider rebasing parallel branches before merge to reduce conflicts | Knowledge — not worth encoding, minor friction | Complete |
| 3 | Phase 2 (Epic 36: FIBP) is GO — transport still 46.5% CPU | sprint-status.yaml (epic-36 backlog, ready to start) | Complete |

## Previous Retro Action Items

| # | From Epic | Action | Status |
|---|-----------|--------|--------|
| 1 | Epic 34 | Run full benchmark suite to measure cumulative impact | Done (Story 35.1) |
| 2 | Epic 34 | Research custom transport protocol | Done (technical-custom-transport-kafka-parity-2026-03-25.md) |

## Key Metrics

- **Single-producer enqueue:** 10,412 → 10,656 msg/s (+2.3%)
- **Auto-batch throughput:** 13,129 → 23,050 msg/s (+75.5%)
- **Tail latency p99.9:** 0.41 → 0.32 ms (-22%)
- **HTTP/2 CPU share:** 53% → 46.5%
- **Test suite:** unchanged (432 tests, zero regressions)
