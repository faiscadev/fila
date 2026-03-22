# Sprint Change Proposal — Epic 18 Consumer Groups Revert

**Date:** 2026-03-22
**Trigger:** Semantic mismatch in Epic 18 consumer groups implementation
**Scope:** Major — fundamental design rework needed
**Approved:** Yes

## Issue Summary

Epic 18 (Consumer Groups, PR #85) shipped consumer groups with throughput-splitting semantics. Each consumer group was treated as one delivery target competing via round-robin — groups split messages between them. The correct behavior (Kafka-style) is that each consumer group gets an independent view of every message with its own delivery state.

This is not a bug in the implementation — the code matches the ACs — but the ACs themselves encoded the wrong semantics. The feature needs a fundamental rearchitecture, not a patch.

## Impact Analysis

- **Epic 18:** Reverted from "done" to "deferred". PR #85 code reverted. Stories TBD pending design.
- **Epic 19:** Unaffected. Story 19.1 (Lua helpers) is independent. Story 19.3 (SDK guides) has a consumer group dependency that will wait.
- **Epic 20:** Unaffected.
- **PRD:** No change needed. FR77 text is correct; implementation semantics were wrong.
- **Architecture:** No doc change now. Design decisions will be documented when stories are created.

## Actions Taken

1. Reverted PR #85 merge commit (82c7887) — all consumer group code removed
2. Updated `sprint-status.yaml` — Epic 18 marked as "deferred"
3. Updated `epics.md` — Epic 18 summary and story section updated to reflect deferred status
4. Created design analysis document: `_bmad-output/planning-artifacts/research/consumer-group-semantics.md`
5. 432 tests pass, zero regressions

## Deferred Design Work

The consumer group feature requires resolution of fundamental design questions before re-implementation:

- Per-group DRR scheduling vs shared scheduling
- Per-group throttle state vs shared throttle
- Whether `on_enqueue` Lua hook scope should change (currently sets consumption-time concerns)
- Independent consumer behavior (implicit default group vs each-gets-all)
- Message completion semantics (when all groups acked)
- Storage model for per-group state
- Raft log implications for group-aware ack/nack

See the full analysis in `_bmad-output/planning-artifacts/research/consumer-group-semantics.md`.

## Handoff

- **Development team:** Revert complete. No further action until design is resolved.
- **Architect/PM:** Resolve design questions in the research doc before creating new stories.
- **SM:** Will create new stories once design decisions are made.
