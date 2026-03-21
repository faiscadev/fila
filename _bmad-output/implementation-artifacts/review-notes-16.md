## PR #78: SDK Auth Feature Parity

### Gaps in Dev Process
- **SDK changes pushed directly to main without PRs.** All 5 external SDK repos (fila-go, fila-python, fila-js, fila-ruby, fila-java) received TLS and API key auth support via direct commits to main — no pull requests, no Cubic review, no human review. This is especially concerning because auth/TLS code is security-sensitive.
- **No story artifact file created.** Story 16.2 has no spec file in `_bmad-output/implementation-artifacts/stories/`, making AC verification impossible during epic review. The CLAUDE.md rule about mid-epic story additions requiring spec files was not followed.
- **This PR (#78) originally contained only tracking file changes** (+8/-8 across 2 files). The actual code that matters — auth wiring in 5 SDKs — lived in external repos with no review trail.

### Incorrect Decisions During Development
- The dev agent treated external SDK repos as "fire and forget" — push to main and move on. The execute-epic workflow has no gate for external repo changes, so the agent took the fastest path.

### Review-Driven Fixes
During epic review, the following was done as compensating controls:
- **Retroactive PRs created in all 5 SDK repos** — auth commits were reverted from main, feature branches created, PRs opened, CI + Cubic run.
- **28 Cubic findings caught and fixed** across 5 SDKs (5 P1s, 23 P2/P3s). Key P1s:
  - Silent TLS downgrade when mTLS options provided without CA cert (Go, Python, JS, Ruby, Java)
  - IPv6 address parsing broken in Java
- **System trust store support added** to all 6 SDKs (review-driven scope addition):
  - Rust SDK: `with_tls()`, CLI: `--tls`
  - Go: `WithTLS()`, Python: `tls=True`, JS: `tls: true`, Ruby: `tls: true`, Java: `withTls()`
- **3 Cubic findings fixed on PR #78** itself (Rust SDK/CLI): silent mTLS ignore, partial mTLS validation.

### Deferred Work
- None — all findings addressed during review.

### Patterns for Future Stories
- **External repo changes MUST go through PRs**, same as the main repo. The execute-epic workflow and story ACs should explicitly require PR numbers for external repos to be recorded.
- **Story specs for multi-repo stories should list all repos and expected PRs** so epic-review can verify each one was properly reviewed.
- **Must add CLAUDE.md rule:** "Stories that modify external repositories must open PRs in those repos, not push directly to main."
- **System trust store should be a default TLS option** — users shouldn't need to provide CA certs for publicly-signed servers.
