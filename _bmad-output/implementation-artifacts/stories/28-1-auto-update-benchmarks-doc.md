# Story 28.1: Auto-update docs/benchmarks.md from CI results

Status: done

## Story

As a user reading docs/benchmarks.md,
I want the benchmark numbers to always reflect the latest main branch run,
so that I never see stale or manually maintained performance data.

## Problem

`docs/benchmarks.md` contains hardcoded benchmark numbers across ~15 tables (throughput, latency, fairness, Lua, scaling, batch, subsystem, competitive). These go stale whenever performance changes. CI already produces structured JSON results on every push to main (`bench-aggregated.json` from `bench-regression.yml`, `bench/competitive/results/bench-*.json` from competitive runs), but nothing feeds them back into the doc.

## Acceptance Criteria

1. **Given** a push to main triggers `bench-regression.yml`
   **When** benchmarks complete successfully
   **Then** a script renders `docs/benchmarks.md` from the JSON results and the current commit hash

2. **And** the script reads `bench-aggregated.json` (self-benchmarks) and produces all self-benchmark tables: throughput, latency, fair scheduling overhead, fairness accuracy, Lua overhead, cardinality scaling, consumer concurrency scaling, memory footprint, compaction impact

3. **And** the script reads `bench/competitive/results/bench-{fila,kafka,rabbitmq,nats}.json` and produces all competitive tables: throughput comparison, latency comparison, lifecycle throughput, multi-producer, resource usage

4. **And** batch benchmark tables (batch enqueue, batch size scaling, auto-batching latency, batched vs unbatched, delivery batching, concurrent producer batching) are rendered from `bench-aggregated.json` when batch metrics are present

5. **And** subsystem benchmark tables (RocksDB raw write, protobuf serialization, DRR scheduler, gRPC round-trip, Lua execution) are rendered from `bench-aggregated.json` when subsystem metrics are present

6. **And** the rendered doc preserves the existing prose sections (section headers, explanatory paragraphs, methodology notes, "Reproducing results" section) — only the data tables and commit reference are updated

7. **And** the doc includes a header noting the commit hash and timestamp of the benchmark run (e.g., "Results from commit `abc1234` on 2026-03-24")

8. **And** if competitive benchmark results are not available (they require Docker and run separately), the script preserves the last known competitive tables rather than clearing them

9. **And** a CI step after benchmarks runs the script and commits the updated doc to main (or opens a bot PR — implementation choice)

10. **And** the script is runnable locally: `./scripts/update-benchmarks.sh` (or equivalent) for developers who want to update the doc from local benchmark runs

11. **And** the script is idempotent — running it twice with the same JSON input produces identical output

## Tasks / Subtasks

- [x] Task 1: Create benchmark doc renderer script (`crates/fila-bench/src/bin/bench-update-docs.rs`)
  - [x] 1.1 Parse `bench-aggregated.json` and map benchmark names to doc table cells
  - [x] 1.2 Parse competitive result JSONs (`bench-{broker}.json`)
  - [x] 1.3 Render all markdown tables with proper formatting (aligned columns, units, computed fields like "overhead %")
  - [x] 1.4 Preserve prose sections (marker-based approach: `<!-- bench:section-start/end -->`)
  - [x] 1.5 Handle missing sections gracefully (batch/subsystem/competitive skipped when absent)

- [x] Task 2: Add markers to docs/benchmarks.md
  - [x] 2.1 Added 27 HTML comment marker pairs around all data tables
  - [x] 2.2 Include commit hash and timestamp via `header` and `traceability` markers

- [x] Task 3: Add CI integration
  - [x] 3.1 Added `update-docs` job to `bench-regression.yml` (on main push only)
  - [x] 3.2 Commits updated doc back to main via github-actions[bot]
  - [x] 3.3 Uses `[skip ci]` in commit message to prevent re-trigger

- [x] Task 4: Local developer experience
  - [x] 4.1 `./scripts/update-benchmarks.sh` wrapper (with optional `--run` flag)
  - [x] 4.2 Usage documented in script header

## Design Notes

**Script language choice:** Python is already used in the competitive benchmark suite and is good at JSON parsing + string templating. A Rust binary is also viable but heavier for a doc generation task.

**Template vs marker approach:**
- *Markers:* `<!-- bench:throughput-start -->` / `<!-- bench:throughput-end -->` in the existing doc. Script replaces content between markers. Prose lives in the doc itself.
- *Template:* Separate template file with `{{throughput_table}}` placeholders. Script renders the whole doc from the template. Prose lives in the template.

Markers are simpler and keep the doc readable in its raw form. Template is cleaner but adds a file.

**Commit strategy:** A `[skip ci]` commit from the benchmark workflow avoids infinite loops. Alternatively, the workflow can check if the doc actually changed before committing.

**Competitive benchmarks:** These require Docker (Kafka/RabbitMQ/NATS containers) and don't run in the standard `bench-regression.yml`. The script should merge competitive results when available but not fail or clear tables when they're absent.
