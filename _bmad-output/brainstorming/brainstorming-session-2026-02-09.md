---
stepsCompleted: [1, 2]
inputDocuments: []
session_topic: 'Fila - Rust-based fair-scheduling queue with native throttling and custom persistence engine'
session_goals: 'Explore full design space (architecture choices, tradeoffs, undiscovered features) and break down the task into achievable build plan'
selected_approach: 'progressive-flow-ai-recommended-hybrid'
techniques_used: ['Morphological Analysis', 'Assumption Reversal', 'Chaos Engineering', 'Cross-Pollination', 'Analogical Thinking', 'Decision Tree Mapping', 'Constraint Mapping']
ideas_generated: []
context_file: '_bmad/bmm/data/project-context-template.md'
---

# Brainstorming Session Results

**Facilitator:** Lucas
**Date:** 2026-02-09

## Session Overview

**Topic:** Fila — a Rust-based fair-scheduling queue with native support for hierarchical throttling (token buckets), fair scheduling (DRR + pluggable WFQ), property-based rules engine, custom append-only persistence layer, and at-least-once delivery semantics.

**Goals:**
1. Explore the full design space — architecture choices, tradeoffs, and undiscovered features/concerns
2. Break down this massive undertaking into a sensible, achievable build plan

### Context Guidance

_Project context template loaded covering: user problems/pain points, feature ideas, technical approaches, UX, business model, market differentiation, technical risks, and success metrics. Existing design exploration from prior ChatGPT session provides foundation covering component breakdown, custom persistence design, DRR vs WFQ analysis, and Rust-oriented architecture._

### Session Setup

_Lucas is building Fila as a side project in Rust. The core insight is that existing queue systems treat fairness and throttling as bolt-on features, while Fila makes them first-class citizens — including a custom persistence layer whose storage layout natively matches scheduler access patterns. Prior exploration covered: hierarchical shaping via token buckets, DRR/WFQ fair scheduling, append-only event log persistence, per-fairness-key queue segments, and a 12-component architecture breakdown._

## Technique Selection

**Approach:** Progressive Flow + AI-Recommended Hybrid
**Journey Design:** Systematic development from design space exploration to actionable build plan

**Progressive Techniques:**

- **Phase 1 - Expansive Exploration:** Morphological Analysis — systematically map every design dimension and explore parameter combinations
- **Phase 2 - Challenge & Stress-Test:** Assumption Reversal + Chaos Engineering — flip core assumptions and find failure modes
- **Phase 3 - Idea Synthesis:** Cross-Pollination + Analogical Thinking — transfer proven patterns from OS scheduling, network QoS, database engines
- **Phase 4 - Action Architecture:** Decision Tree Mapping + Constraint Mapping — map dependencies and create phased build plan

**Journey Rationale:** Lucas already has a solid design foundation from prior exploration. This session expands that foundation (Phase 1), stress-tests it (Phase 2), enriches it with battle-tested patterns from adjacent domains (Phase 3), and converts everything into an actionable breakdown (Phase 4).

## Phase 1: Morphological Analysis — Design Space Exploration

### Design Dimensions & Decisions

| # | Dimension | Decision | Graduate To |
|---|-----------|----------|-------------|
| 1 | Topology | Standalone broker | — |
| 2 | Wire protocol | gRPC only (tonic) | REST, custom protocol |
| 3 | Persistence | RocksDB | Custom engine |
| 4 | Fairness model | `fairness_key` + `weight`, no priority lanes | — |
| 5 | Message model | Opaque bytes + headers | Lua payload parsing (`fila.parse_json`, `fila.parse_protobuf`) |
| 6 | Delivery | Lease-based pull (`lease`, `ack`, `nack`, visibility timeout) | Streaming (gRPC bidirectional) |
| 7 | Scheduling | DRR | Pluggable trait, WFQ opt-in |
| 8 | Throttling | Lua assigns throttle keys at enqueue, API sets limits, Rust token buckets enforce at dequeue | — |
| 9 | Observability | OpenTelemetry (`tracing` + OTel exporter) | Decision event stream |
| 10 | Queue lifecycle | Imperative (API/CLI), DLQ as auto-created queue, redrive | Terraform provider |

### Cross-Cutting Architecture Decisions

- **Lua + Runtime State Model:** Lua defines scheduling/throttling LOGIC (`on_enqueue`, `on_failure`), external systems control DATA via runtime key-value config (`fila.get()`), Rust enforces decisions. Three layers: logic, data, enforcement.
- **Single-Node MVP:** All differentiation is in scheduling/fairness. Distribution is a future layer, not a rewrite. Trait-based storage abstraction enables graduation.
- **CLI-First Admin (`fila-cli`):** All configuration and management through CLI backed by gRPC. Human-friendly interface for queue creation, runtime config, stats, redrive.
- **Build Primitive, Graduate Later:** Consistent philosophy across all dimensions — build the powerful primitive first, layer convenience on top when real user patterns emerge.

### Key Ideas Generated

1. **[Architecture #1]**: Fila as `core lib` + `thin broker binary` — not for embedded deployment, but for clean architecture and testability
2. **[Protocol #2]**: gRPC-first, single protocol for hot path and admin — REST and custom protocol are future doors
3. **[Persistence #3]**: RocksDB with custom key layout matching scheduler access patterns — get "cheap decisions" benefit without building a storage engine
4. **[Fairness #4]**: Weight-based prioritization over strict priority — natural starvation prevention, one less concept
5. **[Rules #5]**: Lua as the rules engine — single scripting layer for fairness, throttling, routing, validation
6. **[DX #6]**: Three-layer DX roadmap — custom Lua (day 1) > built-in helpers > declarative config sugar
7. **[Runtime #7]**: Lua + runtime state model — Lua reads from externally-controllable key-value store, external systems set weights/limits via API
8. **[Throttle #8]**: Hierarchical throttling as emergent behavior — Lua returns multiple throttle keys, no special hierarchy mechanism
9. **[Message #9]**: Payload always opaque bytes — parsing is opt-in via Lua helpers (`fila.parse_json`), broker stays fast
10. **[Lifecycle #10]**: DLQ is just a queue — auto-created, full Lua scripting, redrive feature
11. **[Lifecycle #11]**: Queues are like DB tables — imperative creation, declarative via external tools (Terraform) later
12. **[Observability #12]**: Fairness metrics as differentiator — per-key actual vs fair share, throttle hit rates, DRR state
13. **[Observability #13]**: Decision event stream (parked) — subscribable stream of scheduling decisions for real-time debugging
14. **[Delivery #14]**: Lease semantics are the primitive — streaming is just lease semantics over persistent gRPC streams
15. **[Scheduling #15]**: Lua-influenced scheduling — DRR is the engine (Rust), Lua is the steering wheel (policy). `on_enqueue` returns fairness_key + weight.
16. **[Unified #16]**: Two Lua hooks model — `on_enqueue(msg)` returns all computed metadata (fairness_key, weight, throttle_keys), `on_failure(msg, attempts)` handles retry/DLQ decisions

## Phase 2: Assumption Reversal + Chaos Engineering — Stress Testing

### Risks Identified & Mitigated

| Risk | Severity | Mitigation |
|------|----------|------------|
| Lua on enqueue hot path (infinite loops, slow scripts, memory leaks) | High | Execution timeout, memory limits, pre-compiled scripts, circuit breaker with defaults fallback |
| RocksDB key layout rigidity (hard to change later) | High | Strict storage trait (scheduler never touches keys), column families for access pattern isolation |
| Config change staleness (deep queues hold old weights) | Medium | Day 1: apply on enqueue only. Future: dependency-tracked re-evaluation (record fila.get() calls, re-evaluate on dequeue only when deps changed) |
| DRR over sparse key space (scanning empty keys) | Medium | Active keys set — only keys with pending messages participate in DRR rotation |

### Risks Accepted (Let It Burn)

| Risk | Rationale |
|------|-----------|
| Backpressure (unbounded queue growth) | User monitors via OTel, scales consumers. Max depth trivial to add later. |
| No authentication/authorization | Trust the network, deploy behind firewall. API keys/mTLS when real users need it. |

### Risks Already Handled By Design

| Risk | How |
|------|-----|
| Crash mid-enqueue | RocksDB WriteBatch — all-or-nothing |
| Crash mid-dequeue (after lease, before delivery) | Visibility timeout — message reappears. At-least-once semantics. |
| Token bucket state after crash | Resets on restart — brief burst window, acceptable tradeoff |

### Phase 2 Conclusion

Nothing found requires changing the Phase 1 architecture. Design is resilient. Risks are either mitigated, accepted, or already handled.

## Phase 3: Cross-Pollination — Patterns Stolen From Other Domains

### Adopted Patterns

| Source | Pattern | Application in Fila |
|--------|---------|-------------------|
| Redis (EVALSHA) | Pre-compile scripts, run bytecode | Compile Lua at queue creation, cache bytecode, run pre-compiled per message |
| Redis (architecture) | Single-threaded core, IO on separate threads | Scheduler thread (DRR + throttle) is single-threaded, no locks. gRPC IO threads feed via lock-free channels. |

### Parked Patterns (Future Backlog)

| Source | Pattern | Potential Value |
|--------|---------|----------------|
| Linux CFS | Long-term fairness debt tracking | Keys that were throttled accumulate debt, catch up when throttle lifts |
| Network QoS (trTCM) | Dual token buckets (committed + burst) | Green/yellow/red classification, soft throttling instead of hard cliff |
| Database WAL/CDC | Surface WAL as audit/event stream | Connects to parked decision event stream — audit trail + real-time debugging for free |
| Kafka | Consumer groups | Broker distributes lease assignments across registered consumers |
| Envoy | Filter chains / composable scripts | Multiple chained Lua scripts per queue, concern separation and reuse |

## Phase 4: Action Architecture — Build Plan

### Architecture Overview

```
Multiple gRPC IO threads → lock-free channel → Single scheduler thread (DRR + throttle) → lock-free channel → gRPC IO threads
```

### Dependency Graph

```
Message Types & Envelope (Protobuf)
         │
         ├──→ Storage Trait + RocksDB
         ├──→ Lua Sandbox (mlua)
         └──→ gRPC Server (tonic)
                    │
         Storage + Lua merge into:
                    │
              DRR Scheduler + Token Buckets
                    │
              Broker Core (single-threaded scheduler)
                    │
              fila-cli
```

### Milestone Plan

Observability (tracing + OTel) is built into every milestone from M1 onwards — not a separate phase.

#### M1: "A message goes in and comes out"

**Build:** Protobuf envelope (id, headers, payload, timestamps), storage trait + basic RocksDB, gRPC server (Enqueue, Lease, Ack), in-memory FIFO queue, broker process, tracing + OTel exporter setup.

**Observability:** tracing spans on enqueue/lease/ack, OTel exporter wired from day one.

**Validates:** Skeleton works end-to-end. gRPC ↔ RocksDB ↔ message lifecycle.

#### M2: "Fairness works"

**Build:** DRR scheduler (active keys set, deficit counters, round-robin), fairness_key + weight as enqueue fields (no Lua), visibility timeout on lease, Nack RPC, RocksDB column families (messages, leases, state).

**Observability:** DRR round metrics, per-key throughput counters, visibility timeout tracking.

**Validates:** Core differentiator — fair scheduling — is correct and performant.

#### M3: "Lua controls decisions"

**Build:** mlua integration, pre-compiled script cache (EVALSHA pattern), on_enqueue(msg) → {fairness_key, weight}, on_failure(msg, attempts) → retry/dlq, execution timeout + memory limits, queue creation with script, DLQ auto-creation.

**Observability:** Lua execution time histograms, script error counters, circuit breaker metrics.

**Validates:** Flexibility layer works. User-defined scheduling policy via Lua.

#### M4: "Throttling enforced"

**Build:** Token bucket implementation per throttle key, throttle_keys in on_enqueue return, throttle limit configuration, throttle column family, skip throttled keys in DRR dequeue loop.

**Observability:** Throttle hit/pass rates per key, token bucket state, throttle-induced delays.

**Validates:** Rate limiting works alongside fairness. Hierarchical throttling via multiple keys.

#### M5: "CLI + runtime config"

**Build:** Runtime key-value store (state column family), fila.get() bridge in Lua, gRPC admin RPCs (CreateQueue, SetConfig, GetStats, Redrive), fila-cli binary, queue CRUD, throttle limit setting, redrive.

**Observability:** Admin operation logging, config change events, runtime state metrics.

**Validates:** Full operator experience. External systems can dynamically control Fila behavior.

### Future Graduations (Post-MVP Backlog)

| Feature | Graduation From | Trigger |
|---------|----------------|---------|
| Streaming delivery (gRPC bidi) | Lease-based pull | Consumer throughput needs |
| REST API | gRPC only | Broader adoption demand |
| Custom storage engine | RocksDB | Storage becomes bottleneck |
| Distributed / clustering | Single-node | Single-node capacity exceeded |
| Lua built-in helpers + declarative config | Custom Lua only | User patterns emerge |
| Payload parsing (fila.parse_json) | Opaque bytes only | Users need payload-based keys |
| Consumer groups | Direct lease | Multiple consumers per queue |
| Decision event stream | OTel only | Advanced debugging needs |
| Dependency-tracked re-evaluation | Config applies on enqueue only | Deep queue staleness complaints |
| WFQ scheduler | DRR only | Variable-size message fairness needs |
| Terraform provider | Imperative CLI only | Infrastructure-as-code demand |
| Authentication/authorization | Trust the network | Production multi-tenant deployment |
| Backpressure / max queue depth | Unbounded | Operational necessity |
| Dual token buckets (committed + burst) | Single token bucket | Soft throttling demand |
| CFS-style fairness debt | Per-round DRR fairness | Long-term fairness requirements |
