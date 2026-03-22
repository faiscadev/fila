# Epic 16.5 Review Notes

## PR #79: Cluster E2E Test Suite

### Gaps in Dev Process
- Cubic finding on `fila queue inspect` AC wording (per-queue vs cluster-wide) went unaddressed across 2 review cycles before epic-review caught it

### Incorrect Decisions During Development
- None identified

### Deferred Work
- None

### Patterns for Future Stories
- Cubic findings on planning artifacts (epics.md) are easy to miss since they're doc-only — worth scanning all Cubic comments before marking a story complete

## PR #80: TLS & Auth Edge Case Hardening

### Gaps in Dev Process
- Bootstrap key AC in epics.md said "can only create keys, cannot perform data operations" but implementation gives superadmin scope — spec/implementation mismatch went unaddressed until epic-review
- Same pattern as PR #79: Cubic finding on epics.md AC accuracy left unresolved

### Incorrect Decisions During Development
- None identified

### Deferred Work
- None

### Patterns for Future Stories
- Cubic finding about premature "completed" status in epic-execution-state.yaml is a false positive — execute-epic workflow sets this before CI. Consider if execute-epic should use "pr-pending" instead of "completed"

## PR #81: CI Code Coverage & Quality Gates

### Gaps in Dev Process
- Story artifact lacks explicit AC status lines (SATISFIED/MET) — all tasks checked off but no formal AC sign-off section

### Incorrect Decisions During Development
- None identified

### Deferred Work
- None

### Patterns for Future Stories
- Clean PR — single Cubic finding was already addressed before epic-review. Coverage workflow verified on feature branch per CLAUDE.md rule.
