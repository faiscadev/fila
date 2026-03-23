# Story 21.4: Failure-Path & Fairness Benchmarks

Status: review

## Story

As a developer validating Fila's behavior under adverse conditions,
I want benchmarks for nack storms, DLQ routing overhead, poison pill isolation, and formal fairness measurement,
so that I know the cost of failure paths and can prove Fila's fairness scheduling works correctly under load.

## Tasks / Subtasks

- [x] Task 1: Nack storm benchmark — 10% nack rate vs 100%-ack baseline
- [x] Task 2: DLQ routing overhead benchmark — 5% messages exhaust retries
- [x] Task 3: Poison pill isolation benchmark — per-fairness-key throughput
- [x] Task 4: Add Jain's Fairness Index to existing weighted fairness benchmark
- [x] Task 5: Add equal-weight fairness benchmark with JFI reporting

## Dev Agent Record

### Agent Model Used
Claude Opus 4.6 (1M context)

### File List
- `crates/fila-bench/src/benchmarks/failure_paths.rs` — NEW: 3 failure-path benchmarks
- `crates/fila-bench/src/benchmarks/fairness.rs` — added JFI to weighted + new equal-weight benchmark
- `crates/fila-bench/src/benchmarks/mod.rs` — added `pub mod failure_paths`
- `crates/fila-bench/benches/system.rs` — registered new benchmarks
