# Sprint Change Proposal — Epic Reordering: SDK Before E2E

**Date:** 2026-02-17
**Author:** Bob (Scrum Master)
**Approved by:** Lucas

## Section 1: Issue Summary

During Story 7.1 (Blackbox E2E Test Suite) implementation, three consecutive failed approaches revealed that writing e2e tests with raw gRPC client calls doesn't exercise the real production client path. The Rust SDK must exist first so that e2e tests use the same client that production consumers use. Additionally, admin operations in e2e tests should exercise the `fila` CLI binary, not raw gRPC calls — testing both binaries together.

**Discovery context:** Aborted execute-epic session for Epic 7 on 2026-02-17. All code changes were reverted; working tree is clean on `main`.

**Evidence:**
1. First attempt imported internal server types (`AdminService`, `HotPathService`) — whitebox, not blackbox
2. Second attempt spawned server as subprocess with raw gRPC clients — better, but exercises a code path no production user will take
3. User identified that without the Rust SDK, message delivery e2e tests are pointless — they test plumbing, not the real integration

## Section 2: Impact Analysis

### Epic Impact

| Epic | Impact | Detail |
|------|--------|--------|
| Epic 7 (Scheduler Refactoring) | **Restructured** | Split into Epic 7 (SDK) and Epic 8 (E2E + Refactoring). Rust SDK becomes prerequisite for e2e tests. |
| Epic 8 (Client SDKs) | **Restructured** | Rust SDK pulled out (→ new Epic 7). Remaining 5 SDKs become new Epic 9. |
| Epic 9 (Distribution & Docs) | **Renumbered** | Becomes Epic 10. Content unchanged. |

### Story Impact

| Old Story | New Story | Change |
|-----------|-----------|--------|
| 7.1: Blackbox E2E Test Suite | 8.1: Blackbox E2E Test Suite | Moved to Epic 8; ACs rewritten to use SDK + CLI |
| 7.2: Scheduler Decomposition | 8.2: Scheduler Decomposition | Moved to Epic 8; ACs unchanged |
| 7.3: Cleanup & Deferred Items | 8.3: Cleanup & Deferred Items | Moved to Epic 8; ACs unchanged |
| 8.1: Rust Client SDK | 7.1: Rust Client SDK | Pulled forward to new Epic 7 |
| 8.2-8.6: Other SDKs | 9.1-9.5: Other SDKs | Renumbered into new Epic 9 |
| 9.1-9.4: Docs & Distribution | 10.1-10.4: Docs & Distribution | Renumbered into new Epic 10 |

### Artifact Conflicts

- **PRD:** No conflicts — scope and requirements unchanged
- **Architecture:** No conflicts — SDK is already specified as thin gRPC wrapper from proto
- **epics.md:** Requires full rewrite of Epics 7-9, addition of Epic 10
- **sprint-status.yaml:** Requires restructuring of all remaining entries
- **FR Coverage Map:** FR46 moves from Epic 8 to Epic 7

## Section 3: Recommended Approach

**Selected:** Direct Adjustment — restructure stories across epics.

**Rationale:**
- No code to revert (working tree clean)
- The Rust SDK is architecturally simple (thin tonic wrapper over fila-proto) — low risk to build first
- E2E tests using the real SDK + CLI exercise production code paths
- Scheduler refactoring still gets the e2e safety net, just after the SDK
- Total story count unchanged (13 stories across 4 epics vs. 13 across 3)
- All FRs still covered; no scope change

**Effort:** Low — planning artifact updates only
**Risk:** Low — no code changes, no scope changes
**Timeline:** No impact — same number of stories, just reordered

## Section 4: Detailed Change Proposals

### Epic 7: Rust Client SDK (NEW — 1 story)

**Description:** Developers integrate Fila into Rust applications using an idiomatic client SDK. The SDK is a thin wrapper over tonic gRPC client generated from fila-proto. This epic delivers the first SDK, which also serves as the client for the e2e test suite in Epic 8.

**FRs covered:** FR46, FR48 (partial — Rust only)

**Stories:**
- **7.1: Rust Client SDK** — ACs from old Story 8.1 (unchanged)

### Epic 8: E2E Tests & Scheduler Refactoring (RESTRUCTURED — 3 stories)

**Description:** Build a true blackbox e2e test suite using the Rust SDK (for producer/consumer operations) and the `fila` CLI binary (for admin operations), then use it as a safety net to decompose the monolithic scheduler. The observability layer from Epic 6 provides runtime verification during restructuring.

**Prerequisite:** Epic 7 (Rust SDK) complete.

**Stories:**
- **8.1: Blackbox E2E Test Suite** — Rewritten ACs: uses fila-client SDK + `fila` CLI binary, separate `fila-e2e` crate, spawns `fila-server` as subprocess
- **8.2: Scheduler Decomposition** — ACs from old Story 7.2 (unchanged, dependency updated to 8.1)
- **8.3: Cleanup & Deferred Items** — ACs from old Story 7.3 (unchanged, dependency updated to 8.2)

### Epic 9: Client SDKs (RESTRUCTURED — 5 stories)

**Description:** Developers integrate Fila into applications using idiomatic client SDKs in five additional languages: Go, Python, JavaScript/Node.js, Ruby, and Java.

**FRs covered:** FR42, FR43, FR44, FR45, FR47, FR48 (partial — 5 languages)

**Stories:**
- **9.1: Go Client SDK** — ACs from old Story 8.2
- **9.2: Python Client SDK** — ACs from old Story 8.3
- **9.3: JavaScript/Node.js Client SDK** — ACs from old Story 8.4
- **9.4: Ruby Client SDK** — ACs from old Story 8.5
- **9.5: Java Client SDK** — ACs from old Story 8.6

### Epic 10: Distribution & Documentation (RENUMBERED — 4 stories)

**Description:** Users install Fila via Docker, `cargo install`, or `curl | bash` shell script. Comprehensive documentation enables onboarding in under 10 minutes.

**FRs covered:** FR50, FR51, FR52, FR54, FR55, FR56, FR57, FR58, FR59, FR60

**Stories:**
- **10.1: Docker Image** — ACs from old Story 9.1
- **10.2: Binary Distribution & Installation** — ACs from old Story 9.2
- **10.3: Core Documentation & API Reference** — ACs from old Story 9.3
- **10.4: Tutorials, Examples & Lua Patterns** — ACs from old Story 9.4

## Section 5: Implementation Handoff

**Change scope:** Minor — Direct implementation by development team.

**Artifacts to update:**
1. `_bmad-output/planning-artifacts/epics.md` — Rewrite Epics 7-9, add Epic 10
2. `_bmad-output/implementation-artifacts/sprint-status.yaml` — Restructure remaining entries
3. `_bmad-output/epic-execution-state.yaml` — Reset for new Epic 7
4. `memory/MEMORY.md` — Record restructuring decision

**Handoff:** Development team (via execute-epic workflow) can begin Epic 7 immediately after artifacts are updated.

**Success criteria:**
- All 4 epics defined with correct story ordering and dependencies
- sprint-status.yaml reflects new structure
- FR coverage map updated (FR46 → Epic 7)
- execute-epic workflow can pick up Epic 7 cleanly
