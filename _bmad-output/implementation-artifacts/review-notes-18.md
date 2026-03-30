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

## PR #155: Fix Tracing Overhead on Hot-Path Functions

### Gaps in Dev Process
- Original implementation used `skip_all` on all 17 functions (4 hot-path + 13 admin) — a blunt hammer that degrades admin observability for no performance gain. Review caught this; archive branch `54daf2c` had the correct pattern.
- Story ACs themselves said `skip_all` — the wrong recommendation from 18.1's research doc propagated into the story spec. ACs had to be renegotiated during review.
- `sudo cargo run` in the research doc usage examples — would leave root-owned build artifacts. Cubic caught this.

### Incorrect Decisions During Development
- Applying tracing changes to admin functions that don't carry payload bytes and aren't on the hot path.
- Not checking the archive branch for prior art — the correct `skip(self, request)` pattern was already implemented and validated there.

### Deferred Work
- Competitive bench workflow runs on every PR (~13 min) but produces artifacts nobody looks at. Should be moved to `workflow_dispatch` only.

### Patterns for Future Stories
- Check archive branch for prior art before implementing performance fixes — prior work may have already validated the approach.
- `skip(self, request)` is the correct pattern for hot-path tracing: skips expensive params explicitly, still captures future params unlike `skip_all`.
- Refactored profile-workload to use clap derive during review — new tooling should use clap from the start.
