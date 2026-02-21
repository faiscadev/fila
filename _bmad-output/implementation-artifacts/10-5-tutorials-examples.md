# Story 10.5: Tutorials, Examples & Lua Patterns

Status: in-progress

## Story

As a developer adopting Fila,
I want guided tutorials, working code examples, and copy-paste Lua patterns,
So that I can implement common use cases without starting from scratch.

## Acceptance Criteria

1. **Given** the documentation exists, **When** tutorials and examples are created, **Then** guided tutorials cover: multi-tenant fair scheduling, per-provider throttling, exponential backoff retry.
2. **Given** the SDK exists, **Then** working `examples/fair_scheduling.rs` demonstrates multi-tenant fairness with Lua hooks.
3. **Given** the SDK exists, **Then** working `examples/throttling.rs` demonstrates per-key rate limiting.
4. **Given** all 6 SDKs exist, **Then** each SDK has a working code example showing enqueue -> consume -> ack flow.
5. **Given** Lua is a core feature, **Then** copy-paste Lua hook examples are provided for common patterns: tenant fairness, provider throttling, exponential backoff, header-based routing.
6. **Given** docs should not rot, **Then** each example is tested in CI to prevent documentation rot.

## Tasks / Subtasks

- [ ] Task 1: Create docs/tutorials.md (AC: 1)
  - [ ] 1.1: Multi-tenant fair scheduling tutorial
  - [ ] 1.2: Per-provider throttling tutorial
  - [ ] 1.3: Exponential backoff retry tutorial
- [ ] Task 2: Create Rust examples in examples/ directory (AC: 2, 3, 6)
  - [ ] 2.1: examples/fair_scheduling.rs
  - [ ] 2.2: examples/throttling.rs
  - [ ] 2.3: Add examples to CI (cargo build --examples)
- [ ] Task 3: Create docs/sdk-examples.md (AC: 4)
  - [ ] 3.1: Code examples for all 6 SDKs (Rust, Go, Python, JS, Ruby, Java)
- [ ] Task 4: Create docs/lua-patterns.md (AC: 5)
  - [ ] 4.1: Tenant fairness pattern
  - [ ] 4.2: Provider throttling pattern
  - [ ] 4.3: Exponential backoff pattern
  - [ ] 4.4: Header-based routing pattern

## Dev Notes

### Rust examples

Examples will be in `crates/fila-sdk/examples/` so they're part of the SDK crate and can use `fila_sdk` directly. They require a running fila-server to work (documented in each example).

### CI for examples

`cargo build --examples -p fila-sdk` will be added to the CI workflow to ensure examples compile. Runtime testing is not needed since they require a running server.
