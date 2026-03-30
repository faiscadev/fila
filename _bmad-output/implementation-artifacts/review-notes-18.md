## PR #154: Baseline Benchmark & Flamegraph

### Gaps in Dev Process
- Dev agent skipped AC2 (flamegraph) entirely, claiming "macOS SIP blocks dtrace" — but `sample` tool works without sudo, and dtrace works with sudo. Agent took the easy path instead of finding alternatives.
- Dev agent recommended `skip_all` as the optimization target instead of the more surgical `skip(self, request)`. Without real profiling data, the recommendation was guesswork.

### Incorrect Decisions During Development
- Using "code analysis" as a substitute for actual profiling — code analysis shows what *might* be expensive, not what *is* expensive.
- Recommending `skip_all` in the research doc and story artifact, which later led Story 18.2 to implement the wrong fix.

### Deferred Work
- None

### Patterns for Future Stories
- Performance stories must include actual profiling data (flamegraph or equivalent), not code analysis. Encode this in story ACs explicitly: "produce an SVG flamegraph artifact".
- When an AC says "flamegraph", the dev agent must try multiple profiling tools before declaring it impossible. macOS options: `sample`, `dtrace` (with sudo), Instruments.app.
- The `profile-workload` binary now exists for future profiling needs — use it instead of ad-hoc scripts.
