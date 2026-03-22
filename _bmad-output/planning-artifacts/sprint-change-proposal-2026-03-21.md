# Sprint Change Proposal — Epic 16.5: Stability Hardening & Test Coverage

**Date:** 2026-03-21
**Proposed by:** Lucas
**Trigger:** Epic 16 retrospective + codebase gap analysis
**Scope:** Minor — direct implementation by dev team

## Section 1: Issue Summary

Sixteen consecutive epics shipped at 100% completion, but the test suite has blind spots in the highest-risk areas. Clustering (Epic 14) has unit tests but zero e2e blackbox tests for multi-node failover. mTLS is not tested at e2e level — only plain TLS + API key. The "silent TLS downgrade" bug pattern found in all 5 external SDKs during Epic 16 review confirms that TLS edge cases need systematic coverage. No code coverage metrics exist anywhere in CI.

**Trigger source:** Epic 16 retrospective (2026-03-21) — "silent TLS downgrade is a universal SDK bug pattern" insight, combined with codebase gap analysis showing no cluster e2e tests, no mTLS e2e tests, and no coverage reporting.

## Section 2: Impact Analysis

- **Epic Impact:** New Epic 16.5 inserted between Epic 16 (done) and Epic 17 (backlog). Epic 17 unchanged but shifts timing.
- **Story Impact:** No existing stories modified. 3 new stories added.
- **Artifact Conflicts:** None. PRD, Architecture, UX unchanged.
- **Technical Impact:** New e2e tests and CI workflows added. No production code changes (unless tests surface bugs, which would be fixed inline).

## Section 3: Recommended Approach

**Direct Adjustment** — add new Epic 16.5 with 3 focused stories. No rollback, no scope reduction, no PRD changes needed.

**Rationale:** This is a "sharpen the saw" investment. The existing test suite (420 tests) is strong for functional coverage but has gaps in precisely the areas where production bugs are most expensive — distributed systems failover, TLS security bypasses, and auth edge cases. Hardening before adding new features (Epic 17) is the prudent path.

- **Effort:** Medium (3 stories, mostly test code + CI configuration)
- **Risk:** Low (no production code changes, additive tests only)
- **Timeline impact:** Epic 17 shifts by ~1 epic duration

## Section 4: Detailed Change Proposals

### 4.1 New Epic in epics.md

**Added:** Epic 16.5 section with 3 stories (16.5.1, 16.5.2, 16.5.3) between Epic 16 and Epic 17. FR Coverage Map updated with hardening line.

### 4.2 Sprint Status Update

**Added to sprint-status.yaml:**
```yaml
# Epic 16.5: Stability Hardening & Test Coverage
epic-16.5: backlog
16.5-1-cluster-e2e-test-suite: backlog
16.5-2-tls-auth-edge-case-hardening: backlog
16.5-3-ci-code-coverage-quality-gates: backlog
epic-16.5-retrospective: optional
```

### 4.3 Stories

- **16.5.1: Cluster E2E Test Suite** — Blackbox tests for multi-node failover, cross-node lifecycle, leader routing, node rejoin. 3-node cluster spawned in tests.
- **16.5.2: TLS & Auth Edge Case Hardening** — mTLS mutual auth, silent downgrade prevention, expired certs, auth revocation immediacy, bootstrap key scope. Includes documented TLS test checklist template.
- **16.5.3: CI Code Coverage & Quality Gates** — cargo-llvm-cov in CI, per-crate coverage reporting on PRs, security-critical path coverage visibility, baseline establishment.

### 4.4 No PRD or Architecture Changes

No modifications needed. Epic 16.5 strengthens existing NFR coverage (NFR22-29) without adding new requirements.

## Section 5: Implementation Handoff

- **Scope classification:** Minor — direct implementation by dev team
- **Story order:** 16.5.1 → 16.5.2 → 16.5.3 (coverage gates benefit from new tests in 16.5.1 and 16.5.2 being included in baseline)
- **Dependencies:** None. All infrastructure (clustering, TLS, auth) already exists.
- **Success criteria:** All 3 stories done, CI green, coverage reporting visible on PRs, cluster e2e tests catching real failover scenarios.

## Approval

**Status:** Approved by Lucas (2026-03-21)
