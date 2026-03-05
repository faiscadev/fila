# Story 12.2: CI Regression Detection

Status: in-progress

## Story

As a developer,
I want automatic performance regression detection on every PR,
so that performance degradation is caught before merge.

## Acceptance Criteria

1. **Given** a PR is opened against the repository, **when** the CI benchmark workflow runs, **then** the benchmark suite from Story 12.1 executes on the PR branch and produces a JSON report.

2. **Given** the benchmark run completes, **when** results are compared against stored baselines, **then** baselines are loaded from the `main` branch's last successful bench run (stored as a GitHub Actions artifact).

3. **Given** results are compared against baselines, **when** any key metric regresses beyond the configurable threshold (default: 10%), **then** the CI check fails with a non-zero exit code.

4. **Given** the benchmark comparison completes, **when** there are any regressions or improvements, **then** a summary table is printed to CI output showing metric name, baseline value, current value, and change percentage (with improved/regressed/unchanged labels).

5. **Given** a PR merges to main, **when** the post-merge workflow runs, **then** baseline results are automatically updated by uploading the new bench-results.json as a GitHub Actions artifact.

6. **Given** CI environment variance exists, **when** benchmarks run, **then** the workflow runs the suite multiple times (default: 3) and uses the median of each metric to reduce noise.

7. **Given** a developer makes an intentional performance trade-off, **when** they trigger a workflow dispatch, **then** the baseline can be manually refreshed from the current branch's results.

8. **Given** the benchmark workflow is created, **when** it is pushed to the feature branch, **then** the workflow is triggered at least once on the feature branch to verify it works before merge (per CLAUDE.md CI workflow verification rule).

## Tasks / Subtasks

- [x] Task 1: Create bench comparison tool (AC: 2, 3, 4)
  - [x] Add `src/compare.rs` to `fila-bench` with `compare_reports(baseline: &BenchReport, current: &BenchReport, threshold: f64) -> CompareResult`
  - [x] `CompareResult` contains per-metric comparison: baseline value, current value, change_pct, status (improved/regressed/unchanged)
  - [x] Print summary table to stdout with aligned columns
  - [x] Return exit code 1 if any metric regresses beyond threshold
  - [x] Add a `compare` binary target to fila-bench (`src/bin/bench-compare.rs`) that reads two JSON report files and outputs the comparison
  - [x] 8 unit tests covering throughput/latency/memory regression, improvements, new metrics, zero baseline

- [x] Task 2: Add multi-run median aggregation (AC: 6)
  - [x] Add `src/aggregate.rs` with `aggregate_reports(reports: &[BenchReport]) -> BenchReport` that computes median value per metric across runs
  - [x] Add an `aggregate` binary target (`src/bin/bench-aggregate.rs`) that reads N JSON files and outputs the aggregated report
  - [x] 5 unit tests covering odd/even counts, single report, empty, multi-metric

- [x] Task 3: Create CI benchmark workflow (AC: 1, 5, 6, 7, 8)
  - [x] Create `.github/workflows/bench-regression.yml` triggered on `pull_request`, `push` (main), and `workflow_dispatch`
  - [x] On PR: build server+cli, run bench 3 times, aggregate to median, restore baseline from cache, compare, fail if regression
  - [x] On push to main: build server+cli, run bench 3 times, aggregate, save as baseline cache
  - [x] On workflow_dispatch: run bench on current branch, save as new baseline cache
  - [x] Install protoc in workflow (matches existing CI pattern)
  - [x] Use `actions/cache/save` and `actions/cache/restore` for baseline storage
  - [x] Upload results as artifact for every run

- [x] Task 4: Remove bench job from ci.yml (AC: 1)
  - [x] Remove the `bench` job from `.github/workflows/ci.yml` since it's now handled by the dedicated `bench-regression.yml` workflow

- [ ] Task 5: Verify workflow on feature branch (AC: 8)
  - [x] Temporarily broaden `bench-regression.yml` trigger to include the feature branch
  - [ ] Push and verify the workflow runs successfully
  - [ ] Narrow the trigger back to its intended scope before merge

## Dev Notes

### Architecture & Design Decisions

**Separate workflow, not inline in ci.yml.**
The bench regression workflow is complex (multi-run, artifact management, comparison logic) and should be its own file. This also lets it run independently and have different trigger rules. The existing `bench` job in `ci.yml` will be removed to avoid duplicate runs.

**Binary targets for compare and aggregate, not shell scripts.**
Using Rust binaries in fila-bench keeps all logic in one language, makes it testable, and avoids bash parsing fragility. The binaries read JSON files and produce output — simple CLI tools.

**GitHub Actions artifacts for baseline storage.**
Artifacts are simpler than cache for this use case: they persist across workflows, don't evict under pressure, and can be downloaded by name. Use `actions/download-artifact@v4` with `github-token` to download from the main branch's workflow.

**Median of 3 runs for noise reduction.**
CI machines have variable performance. Running 3 times and taking the median per metric smooths out outliers. This is a pragmatic balance: more runs = more stable but slower CI.

### Key Implementation Details

**Comparing reports:**
```rust
pub struct MetricComparison {
    pub name: String,
    pub baseline: f64,
    pub current: f64,
    pub change_pct: f64,
    pub unit: String,
    pub status: CompareStatus, // Improved, Regressed, Unchanged
}

pub enum CompareStatus {
    Improved,
    Regressed,
    Unchanged,
}
```

Change percentage: `(current - baseline) / baseline * 100`. For throughput metrics (higher is better), a negative change means regression. For latency metrics (lower is better), a positive change means regression. The compare tool needs to understand metric polarity.

**Metric polarity mapping:**
- `*_throughput*`, `*_msg/s*`, `*_mbps*` → higher is better (negative change = regression)
- `*_latency*`, `*_p50*`, `*_p95*`, `*_p99*`, `*_ms*`, `*_us*` → lower is better (positive change = regression)
- `*_overhead*`, `*_deviation*` → lower is better (positive change = regression)
- `*_rss*`, `*_memory*` → lower is better (positive change = regression)

**Artifact naming convention:**
- Baseline artifact: `bench-baseline` (uploaded on merge to main)
- PR run artifact: `bench-results-{run_number}` (for each of the 3 runs)
- Aggregated artifact: `bench-results-aggregated` (the median report)

### Workflow Structure

```yaml
name: Bench Regression
on:
  pull_request:
  push:
    branches: [main]
  workflow_dispatch:
    inputs:
      update_baseline:
        description: 'Update baseline from current branch'
        type: boolean
        default: false
```

**PR path:** build → run bench ×3 → aggregate → download baseline → compare → fail if regressed
**Main push path:** build → run bench ×3 → aggregate → upload as baseline
**Manual path:** build → run bench ×3 → aggregate → upload as baseline

### JSON Report Consumption

Story 12.1's `BenchReport` already serializes to JSON with `bench-results.json`. The compare and aggregate tools consume this format directly. No schema changes needed.

### What NOT To Do

- Do NOT use GitHub Actions cache for baselines — artifacts are more reliable and don't evict
- Do NOT compare single runs — CI variance makes single-run comparison unreliable
- Do NOT hardcode metric polarity — derive it from metric name patterns or unit
- Do NOT block on benchmark failures in the main CI workflow — keep the regression check as a separate check
- Do NOT use `continue-on-error` on the regression check — the whole point is to block merges on regressions

### References

- [Source: crates/fila-bench/src/report.rs] — BenchReport/BenchResult JSON schema
- [Source: crates/fila-bench/benches/system.rs] — Benchmark suite entry point
- [Source: .github/workflows/ci.yml] — Current CI with bench job to remove
- [Source: CLAUDE.md#CI-Workflow-Verification] — Requirement to test workflows on feature branch
- [Source: _bmad-output/planning-artifacts/epics.md#12.2] — Epic plan with ACs

## Dev Agent Record

### Agent Model Used

### Debug Log References

### Completion Notes List

### File List
