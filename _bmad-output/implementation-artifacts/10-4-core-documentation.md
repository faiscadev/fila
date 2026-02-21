# Story 10.4: Core Documentation & API Reference

Status: in-progress

## Story

As a user evaluating Fila,
I want comprehensive documentation that explains concepts, architecture, and API,
So that I can understand and adopt Fila quickly.

## Acceptance Criteria

1. **Given** Fila is ready for public use, **When** documentation is created, **Then** `README.md` includes: project overview, problem statement, quickstart (Docker + CLI), key concepts (fairness, throttling, Lua hooks), and links to detailed docs.
2. **Given** proto files exist, **Then** API reference documentation is generated from `.proto` files.
3. **Given** Fila is an AI-adjacent tool, **Then** a `llms.txt` file is structured for LLM agent consumption with project context, API surface, and usage patterns.
4. **Given** Fila has multiple core concepts, **Then** documentation covers: message lifecycle, fairness groups, DRR scheduling, token bucket throttling, Lua hooks, DLQ, runtime config.
5. **Given** the server accepts TOML configuration, **Then** a configuration reference documents all options with defaults.
6. **Given** a new user, **Then** download to fair-scheduling demo is achievable in under 10 minutes following the docs.

## Tasks / Subtasks

- [ ] Task 1: Rewrite README.md (AC: 1, 6)
  - [ ] 1.1: Project overview and problem statement
  - [ ] 1.2: Quickstart with Docker and install script
  - [ ] 1.3: Key concepts overview with links to detailed docs
  - [ ] 1.4: SDK links table (all 6 languages)
  - [ ] 1.5: Configuration and CLI overview
- [ ] Task 2: Create docs/concepts.md (AC: 4)
  - [ ] 2.1: Message lifecycle diagram
  - [ ] 2.2: Fairness groups and DRR scheduling
  - [ ] 2.3: Token bucket throttling
  - [ ] 2.4: Lua hooks (on_enqueue, on_failure)
  - [ ] 2.5: Dead letter queue and redrive
  - [ ] 2.6: Runtime configuration
- [ ] Task 3: Create docs/api-reference.md (AC: 2)
  - [ ] 3.1: Hot-path RPCs (Enqueue, Consume, Ack, Nack)
  - [ ] 3.2: Admin RPCs (CreateQueue, DeleteQueue, SetConfig, GetConfig, ListConfig, GetStats, Redrive, ListQueues)
  - [ ] 3.3: Message types and structures
- [ ] Task 4: Create docs/configuration.md (AC: 5)
  - [ ] 4.1: TOML config reference with all sections and defaults
  - [ ] 4.2: Environment variables
- [ ] Task 5: Create llms.txt (AC: 3)
  - [ ] 5.1: Project context and API surface for LLM consumption

## Dev Notes

### Documentation Structure

```
README.md         — landing page, quickstart, overview
docs/
  concepts.md     — core concepts deep dive
  api-reference.md — gRPC API reference from proto files
  configuration.md — server config reference
llms.txt          — LLM-optimized project summary
```

### References

- [Source: proto/fila/v1/service.proto] — Hot-path RPCs
- [Source: proto/fila/v1/admin.proto] — Admin RPCs
- [Source: proto/fila/v1/messages.proto] — Message types
- [Source: crates/fila-core/src/broker/config.rs] — Server configuration
- [Source: crates/fila-server/src/main.rs] — Server startup
- [Source: crates/fila-cli/src/main.rs] — CLI commands
