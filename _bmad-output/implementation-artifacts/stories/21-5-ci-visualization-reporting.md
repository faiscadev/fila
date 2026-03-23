# Story 21.5: CI Visualization & Reporting

Status: review

## Story

As a developer reviewing a PR,
I want benchmark results visualized as trend charts on GitHub Pages with regression alerts on PRs,
so that I can see performance trends over time and catch regressions without reading raw JSON.

## Tasks / Subtasks

- [x] Task 1: Add `to_gab_json()` to BenchReport for github-action-benchmark format
- [x] Task 2: Split metrics by polarity (smallerIsBetter / biggerIsBetter)
- [x] Task 3: Update `write_and_print()` to emit GAB files alongside main report
- [x] Task 4: Update `bench-aggregate` binary to emit GAB files
- [x] Task 5: Add github-action-benchmark steps to bench-regression.yml
- [x] Task 6: Preserve existing bench-compare for backward compatibility

## Dev Agent Record

### Agent Model Used
Claude Opus 4.6 (1M context)

### File List
- `crates/fila-bench/src/report.rs` — added `to_gab_json()`, `gab_higher_is_better()`, updated `write_and_print()`
- `crates/fila-bench/src/bin/bench-aggregate.rs` — emit GAB files alongside aggregated report
- `.github/workflows/bench-regression.yml` — added 4 github-action-benchmark steps (main + PR, split by polarity)
