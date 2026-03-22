# Story 19.3: SDK & Integration Guides

Status: ready-for-dev

## Story

As a developer,
I want comprehensive SDK guides and integration examples,
so that I can integrate Fila into my application quickly in my language of choice.

## Acceptance Criteria

1. **Given** a developer using any supported SDK
   **When** they read the SDK quick start guide
   **Then** `docs/sdk-quickstart.md` provides per-SDK setup instructions (Rust, Go, Python, JS, Ruby, Java)
   **And** each section covers: installation, connection, create queue, enqueue, consume, ack/nack

2. **Given** a developer building common messaging patterns
   **When** they read the integration patterns guide
   **Then** `docs/integration-patterns.md` covers: producer/consumer, fan-out, request-reply
   **And** each pattern includes a description, architecture diagram (ASCII), and at least one SDK example

3. **Given** a developer needing TLS or API key auth
   **When** they read the SDK guides
   **Then** `docs/sdk-quickstart.md` includes a "Security" section showing TLS and API key configuration for each SDK

4. **Given** a developer encountering issues
   **When** they read the troubleshooting guide
   **Then** `docs/troubleshooting.md` covers: connection refused, TLS errors, auth failures, queue not found, consumer timeout, message redelivery

5. **Given** the docs site navigation
   **Then** `docs/SUMMARY.md` is updated to include the new guides

## Tasks / Subtasks

- [ ] Task 1: Create per-SDK quick start guide (AC: 1, 3)
  - [ ] 1.1: Create `docs/sdk-quickstart.md` with sections for each SDK
  - [ ] 1.2: Add security section (TLS + API key) per SDK

- [ ] Task 2: Create integration patterns guide (AC: 2)
  - [ ] 2.1: Create `docs/integration-patterns.md`
  - [ ] 2.2: Document producer/consumer, fan-out, request-reply patterns

- [ ] Task 3: Create troubleshooting guide (AC: 4)
  - [ ] 3.1: Create `docs/troubleshooting.md` with common issues and solutions

- [ ] Task 4: Update navigation (AC: 5)
  - [ ] 4.1: Update `docs/SUMMARY.md` with new pages

- [ ] Task 5: Update sprint-status.yaml
  - [ ] 5.1: Mark story 19-3 as in-progress

## Dev Notes

### Existing Documentation

- `docs/sdk-examples.md` — Already has code examples for all 6 SDKs (create queue, enqueue, consume, ack/nack). This is a reference doc — the quick start guide should be more tutorial-focused.
- `docs/tutorials.md` — Guided walkthroughs for fairness, throttling, Lua hooks.

### SDK Package Names

- **Rust**: `fila-sdk` (crates.io)
- **Go**: `github.com/faiscadev/fila-go`
- **Python**: `fila` (PyPI)
- **JavaScript**: `fila-client` (npm)
- **Ruby**: `fila-client` (RubyGems)
- **Java**: `dev.faisca:fila-client` (Maven Central)

### Consumer Groups Note

Epic 18 (consumer groups) is deferred. Skip consumer group examples. When Epic 18 ships, update these guides.

### References

- [Source: docs/sdk-examples.md] — existing SDK code examples
- [Source: docs/tutorials.md] — existing tutorials
- [Source: _bmad-output/planning-artifacts/epics.md#Epic-19] — Story ACs

## Dev Agent Record

### Agent Model Used

Claude Opus 4.6 (1M context)

### Debug Log References

### Completion Notes List

### File List
