---
stepsCompleted: [1, 2, 3, 4, 5, 6]
inputDocuments:
  - '_bmad-output/brainstorming/brainstorming-session-2026-02-09.md'
date: 2026-02-10
author: Lucas
---

# Product Brief: fila

<!-- Content will be appended sequentially through collaborative workflow steps -->

## Executive Summary

Fila is an open-source, general-purpose message broker built in Rust where fair scheduling and throttling are first-class primitives, not afterthoughts. Existing queue systems -- Kafka, RabbitMQ, Pulsar, SQS, and others -- treat message delivery as a FIFO problem and leave fairness and rate limiting entirely to consumers. This forces every team building multi-tenant or rate-sensitive systems to implement the same painful "consume-check-redrive" pattern: pull a message, check external state, and requeue if conditions aren't met. Fila eliminates this by making scheduling decisions at dequeue time based on live fairness state and per-key throttle limits, so only ready messages are ever delivered to consumers.

---

## Core Vision

### Problem Statement

It is fundamentally impossible to build a high-performance event-driven system with fairness and throttling on top of existing message queues. Every established broker -- Kafka, RabbitMQ, Pulsar, NATS, SQS, and others -- is architecturally blind at dequeue time. They deliver messages in FIFO order with no awareness of which tenant is being starved, which rate limit has been hit, or which key should be skipped. Teams are forced to build fairness and throttling as an external layer: consuming messages, checking rules against external state, and redriving messages that can't be processed yet. This consume-check-redrive loop wastes compute, adds latency, creates operational complexity, and never achieves the performance of a broker that simply knows better.

### Problem Impact

Every team building multi-tenant or rate-sensitive event-driven systems hits this wall. The impact is threefold:

- **Wasted engineering effort**: Teams must design, build, and maintain custom fairness and throttling layers -- token buckets in Redis, priority demotion middleware, Postgres-backed round-robin schedulers, delay-queue plumbing. Every team builds the same thing differently, painfully, on top of queues that were never designed for it.
- **Degraded performance**: The consume-check-redrive loop burns CPU cycles, network bandwidth, and queue operations on messages that can't be processed yet. Consumers pull work only to put it back -- a fundamental architectural inefficiency.
- **Operational fragility**: Custom fairness layers are a constant source of bugs, edge cases, and tuning. Config staleness, redelivery loops, starvation under load, and coordination between rate-limiter services and queue consumers create ongoing operational burden.

This pain is felt across domains: email sending platforms respecting per-sender and per-provider rate limits, multi-tenant SaaS preventing noisy neighbors, webhook delivery systems honoring per-destination throughput, payment processors managing per-gateway limits, IoT platforms throttling misbehaving devices, CI/CD systems fairly allocating build capacity, and LLM inference platforms distributing GPU time across API keys.

### Why Existing Solutions Fall Short

No established, general-purpose, open-source message broker combines fair scheduling and per-key throttling at dequeue time:

- **Kafka, RabbitMQ, Pulsar, NATS, ActiveMQ**: Entirely FIFO. No dequeue-time scheduling awareness. Fairness and throttling are the consumer's problem.
- **Amazon SQS Fair Queues** (2025): Performs broker-side fairness based on in-flight imbalance, but has no per-key throttling. Proprietary and managed-only.
- **BullMQ Pro**: Achieves both fairness and per-key throttling, but is a commercial paid library on Redis -- not a standalone broker, limited to Node.js/Python/Elixir.
- **Temporal**: Adding task queue fairness with weights and rate limits, but is a workflow orchestration engine, not a message queue.
- **Hatchet, Inngest**: Built custom solutions for this exact gap, but are either task-specific or commercial hosted platforms.

The gap is clear: there is no open-source, standalone message broker where a user can say "schedule fairly across these keys, throttle each key at these rates" and have the broker handle it natively at dequeue time.

### Proposed Solution

Fila is a Rust-based message broker that makes fair scheduling and throttling first-class broker primitives. Its core architecture is built around a single-threaded scheduler that makes dequeue decisions based on live fairness state (Deficit Round Robin across fairness keys) and per-key throttle state (token buckets per throttle key). Messages from rate-limited or over-served keys are simply skipped -- never delivered to consumers, never redriven.

A Lua rules engine gives users full control over scheduling policy. Two hooks -- `on_enqueue` (assigns fairness keys, weights, and throttle keys) and `on_failure` (decides retry vs. dead-letter) -- let users define their own rules without the broker imposing opinions. External systems can dynamically adjust weights and limits at runtime via API, while Lua scripts read that state to make decisions.

The architecture follows a Redis-inspired model: gRPC IO threads feed messages through lock-free channels to a single-threaded scheduler core, eliminating lock contention on the hot path. RocksDB provides persistence with a key layout designed to match scheduler access patterns.

### Key Differentiators

- **Fairness and throttling as broker primitives**: The only open-source, general-purpose broker where scheduling decisions at dequeue time consider both fairness state and per-key throttle state. Rate-limited keys are skipped, not consumed and redriven.
- **Lua rules engine for full policy control**: Users define their own fairness, throttling, and failure-handling logic. The broker provides the enforcement engine, not the opinions.
- **Zero wasted work**: Consumers only receive messages that are ready to be processed. No consume-check-redrive loop, no wasted cycles, no redelivery overhead.
- **Performance by design**: Single-threaded scheduler core with lock-free IO channels, pre-compiled Lua scripts, and a persistence layout matched to access patterns -- built for throughput from the ground up.
- **Born from real production pain**: Designed by engineers who have lived the consume-check-redrive problem in production and want to solve it properly.

## Target Users

### Primary Users

#### Persona 1: Platform/Backend Engineer — "Maya"

**Context:** Maya is a senior backend engineer on a platform team at a mid-to-large SaaS company. Her team owns the event-driven infrastructure that powers async workloads across the product -- processing jobs, delivering notifications, syncing data, handling webhooks. The system is multi-tenant, and different tenants generate wildly different volumes.

**Problem Experience:** Maya has spent the last quarter building a fairness and throttling layer on top of their existing message broker. She maintains a Redis-based token bucket system, a priority demotion service, and custom consumer logic that checks rate limits before processing each message. When a high-volume tenant spikes, the whole pipeline slows down for everyone. The fairness layer is brittle -- config staleness causes stale weights, redelivery loops waste compute, and edge cases surface weekly. Every time she touches it, something else breaks.

**Workarounds:** Consume-check-redrive loops, per-tenant Redis counters, delay queues for rate-limited messages, custom middleware that requeues work that can't be processed yet. She's effectively built a scheduler on top of a system that was never designed for one.

**Success Vision:** Maya replaces the entire custom fairness/throttling stack with fila. She writes two Lua hooks -- `on_enqueue` to assign fairness keys and throttle keys, `on_failure` to handle retries -- and the broker handles the rest. No more Redis token buckets, no more redelivery loops, no more consumer-side rate limit checks. She gets her quarter back.

**What makes her say "this is exactly what I needed":** The first time she sees fila skip a rate-limited key and serve the next ready tenant without any consumer involvement -- zero wasted work, no code required.

#### Persona 2: SRE/Infrastructure Engineer — "Tomas"

**Context:** Tomas is an SRE responsible for the reliability and performance of the event-driven infrastructure Maya's team builds on. He monitors queue depth, consumer lag, processing latency, and gets paged when things go wrong. He manages the broker, the Redis cluster backing the rate limiters, and the custom middleware services.

**Problem Experience:** The custom fairness layer is his biggest operational headache. It's three separate systems (broker, Redis, middleware) that must stay in sync. When Redis hiccups, rate limiting breaks. When the middleware service OOMs, messages pile up. Debugging fairness issues requires correlating logs across all three systems. He can't answer "why was tenant X starved for 10 minutes?" without a forensic investigation.

**Workarounds:** Custom dashboards stitching together metrics from three systems, runbooks for common failure modes, manual redrive scripts when messages get stuck.

**Success Vision:** Tomas operates a single binary with built-in OpenTelemetry metrics. Per-key throughput, throttle hit rates, DRR scheduling state, and Lua execution times are all available out of the box. When something looks wrong, he checks one system, not three. The operational surface area shrinks dramatically.

**What makes him say "this is exactly what I needed":** Per-fairness-key actual-vs-fair-share metrics on a Grafana dashboard, and being able to answer "why was tenant X delayed?" in under a minute.

### Secondary Users

#### Persona 3: Tech Lead / Architect — "Priya"

**Context:** Priya is a staff engineer or tech lead evaluating infrastructure choices for her team's next system. She's been through the "build fairness on top of Kafka" journey before and knows the cost. When she sees fila, she's evaluating whether it's mature enough to adopt, whether the model fits her architecture, and whether it saves her team from repeating the same painful build.

**Key Concerns:** Production readiness, community health, escape hatches (can we migrate away if needed?), operational maturity, and whether fila's Lua-based policy model is flexible enough for their use cases.

**Success Vision:** Priya recommends fila to her team with confidence, knowing it solves a problem she's seen multiple teams waste months on. She points to the competitive landscape and says "nothing else does this."

### User Journey

**Discovery:** An engineer hits the fairness/throttling wall with their current queue. They search for solutions, find the same Stack Overflow answers and blog posts describing workarounds, and eventually discover fila -- either through a blog post explaining the consume-check-redrive anti-pattern, a conference talk, or a recommendation from someone who's been through it.

**Onboarding:** They run the fila broker locally, create a queue with a simple Lua script that assigns fairness keys, enqueue messages with different keys, and watch the broker serve them fairly. The CLI (`fila-cli`) makes this tactile and immediate. Time to first "aha": minutes, not hours.

**Core Usage:** In production, they define Lua hooks for their scheduling policy, set throttle limits via the API, and let fila handle dequeue decisions. Consumers become simple -- pull a message, process it, ack. No rate-limit checks, no requeue logic. OTel metrics flow into their existing observability stack.

**Success Moment:** The first production incident that doesn't happen -- a high-volume tenant spikes, and fila's DRR scheduler naturally gives other tenants their fair share. No pages, no manual intervention, no degradation. The system just works.

**Long-term:** Fila becomes the default queue for any workload with fairness or throttling requirements. The team evolves their Lua scripts as new use cases emerge. Runtime config via the API lets them adjust weights and limits without redeployment.

## Success Metrics

### User Success Metrics

- **Zero wasted work achieved**: Consumers never receive messages that can't be processed. No consume-check-redrive loops in user architectures -- the broker handles it.
- **Custom fairness layer eliminated**: Users replace their existing Redis token buckets, priority demotion middleware, and delay-queue plumbing with fila's native scheduling. Fewer systems to operate, fewer things to break.
- **Simple setup for common cases**: Users configure fairness and throttling for standard use cases through declarative configuration or built-in helpers -- no scripting required. Lua is available as the power layer for complex or custom policies, but the easy path doesn't require it.
- **Single-system observability**: Users answer "why was tenant X delayed?" from fila's built-in OTel metrics alone, without correlating logs across multiple systems.
- **Minutes to first value**: A new user can run fila locally, create a queue with simple config, and see fair scheduling in action within minutes, not hours.

### Project Success Criteria

- **It works**: Fila reliably handles the core use case -- fair scheduling with per-key throttling at dequeue time -- in production-grade conditions. Correct behavior under load, crash recovery, and edge cases.
- **Kafka-competitive throughput**: Fila achieves message throughput in the same ballpark as Kafka for comparable workloads, proving that fairness and throttling at dequeue time don't come at a fundamental performance cost.
- **Community recognition**: Hacker News frontpage -- the project resonates with engineers who have lived this pain and validates that the problem and approach are real.

### Business Objectives

N/A -- Fila is an open-source side project driven by solving an unsolved infrastructure problem, not by business metrics. Success is measured by technical merit and community impact.

### Key Performance Indicators

| KPI | Target | Measurement |
|-----|--------|-------------|
| Throughput (messages/sec) | Kafka-competitive (100k+ msg/s single node) | Benchmark suite against Kafka on equivalent hardware |
| Scheduling overhead | < 5% throughput cost vs. raw FIFO mode | Benchmark fair scheduling vs. FIFO-only mode |
| Fairness accuracy | Actual share within 5% of fair share under sustained load | Per-key throughput metrics over time under multi-key workload |
| Lua hook latency | < 50us p99 per invocation | Pre-compiled script execution benchmarks |
| Crash recovery | Full state recovery, zero message loss | Automated kill-and-restart test suite |
| Time to first value | < 10 minutes from download to fair-scheduling demo | New user onboarding test |

## MVP Scope

### Core Features

The MVP encompasses five milestones that deliver the complete fila primitive -- no shortcuts, no half-measures:

**M1 — "A message goes in and comes out"**
- Protobuf message envelope (id, headers, payload, timestamps)
- Storage trait + RocksDB implementation
- gRPC server (Enqueue, Lease, Ack)
- In-memory FIFO queue
- Broker process
- Tracing + OpenTelemetry exporter from day one

**M2 — "Fairness works"**
- DRR scheduler with active keys set, deficit counters, and round-robin
- fairness_key + weight as enqueue fields (no Lua yet)
- Visibility timeout on lease
- Nack RPC
- RocksDB column families (messages, leases, state)
- Per-key throughput counters and DRR round metrics

**M3 — "Lua controls decisions"**
- mlua integration with pre-compiled script cache (EVALSHA pattern)
- `on_enqueue(msg)` hook returning fairness_key, weight, throttle_keys
- `on_failure(msg, attempts)` hook for retry vs. dead-letter decisions
- Execution timeout + memory limits
- Queue creation with script attachment
- DLQ auto-creation
- Lua execution time histograms, script error counters, circuit breaker metrics

**M4 — "Throttling enforced"**
- Token bucket implementation per throttle key
- throttle_keys returned from `on_enqueue` Lua hook
- Throttle limit configuration via API
- Throttle column family in RocksDB
- Skip throttled keys in DRR dequeue loop
- Throttle hit/pass rates per key, token bucket state metrics

**M5 — "CLI + runtime config"**
- Runtime key-value store (state column family)
- `fila.get()` bridge in Lua for reading runtime config
- gRPC admin RPCs (CreateQueue, SetConfig, GetStats, Redrive)
- `fila-cli` binary for all queue management and operations
- Queue CRUD, throttle limit setting, redrive
- Admin operation logging, config change events

**Cross-cutting:** OpenTelemetry observability is built into every milestone from M1 onwards -- not a separate phase.

### Out of Scope for MVP

The following are explicitly deferred. The MVP builds the powerful primitive; these graduate when real usage patterns demand them:

- **Streaming delivery** (gRPC bidirectional) -- lease-based pull is the MVP primitive
- **REST API** -- gRPC only for MVP
- **Custom storage engine** -- RocksDB serves as the MVP backend
- **Distributed / clustering** -- single-node; distribution is a future layer, not a rewrite
- **Simple / declarative config sugar** -- Lua is the MVP policy layer; built-in helpers and declarative config come post-MVP when user patterns emerge
- **Payload parsing** (`fila.parse_json`, `fila.parse_protobuf`) -- payload is opaque bytes; parsing is opt-in later
- **Consumer groups** -- direct lease only
- **Decision event stream** -- OTel metrics only
- **WFQ scheduler** -- DRR only; WFQ is a pluggable trait graduation
- **Authentication / authorization** -- trust the network; deploy behind firewall
- **Backpressure / max queue depth** -- users monitor via OTel, scale consumers
- **Management GUI** -- CLI-first for MVP
- **Hosted platform** -- future possibility

### MVP Success Criteria

The MVP is successful when:

- **It works correctly**: Fair scheduling and per-key throttling produce correct behavior under sustained load, crash recovery, and edge cases. No message loss, no starvation, no fairness violations.
- **It performs**: Throughput is in the same ballpark as Kafka on equivalent hardware (100k+ msg/s single node), proving that dequeue-time fairness doesn't come at a fundamental performance cost.
- **It's operable**: A single engineer can deploy, configure, monitor, and manage fila through the CLI and OTel metrics without external documentation or tribal knowledge.
- **It resonates**: The project reaches Hacker News frontpage, validating that the problem and approach connect with engineers who have lived this pain.

### Future Vision

If fila succeeds, it becomes the Kafka of fair-scheduling queues -- foundational infrastructure that big tech and startups alike depend on for any workload requiring fairness and throttling.

**The full vision includes:**
- **Distributed clustering** -- multi-node deployment for horizontal scalability
- **Consumer groups** -- broker-managed consumer coordination and load distribution
- **WFQ scheduler** -- pluggable weighted fair queuing for variable-size message fairness
- **Authentication and authorization** -- production-grade multi-tenant security
- **Streaming delivery** -- gRPC bidirectional streams for high-throughput consumers
- **REST API** -- broader accessibility beyond gRPC
- **Simple config sugar** -- declarative configuration and built-in helpers for common patterns, translating to Lua under the hood
- **Payload parsing** -- Lua helpers for JSON, Protobuf, enabling payload-based routing and keying
- **Custom storage engine** -- purpose-built storage when RocksDB becomes the bottleneck
- **Decision event stream** -- subscribable stream of scheduling decisions for real-time debugging and audit
- **CFS-style fairness debt** -- long-term fairness tracking across throttle windows
- **Dual token buckets** -- committed + burst rates for soft throttling
- **Terraform provider** -- infrastructure-as-code for queue management
- **Management GUI** -- built-in web interface for queue management, monitoring, and debugging -- no third-party tools required
- **Hosted platform** -- managed fila service

**Community vision:** A thriving open-source project with active contributors and maintainers, production deployments at scale, and an ecosystem of integrations and tooling built by the community.

**The ultimate validation:** We know fila has truly succeeded when AWS launches a managed version of it -- the same compliment they paid to Elasticsearch and Kafka. (For the record: predicted on February 10th, 2026.)
