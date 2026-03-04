---
stepsCompleted: [1, 2, 3, 4]
inputDocuments: []
session_topic: "Fila's post-Epic 11 roadmap — strategic direction after v1 delivery"
session_goals: "Generate ideas across features, infrastructure, ecosystem, community; prioritize release engineering and benchmarking/competitive analysis"
selected_approach: 'ai-recommended'
techniques_used: ['question-storming', 'cross-pollination', 'first-principles-thinking']
ideas_generated: [10]
context_file: ''
session_active: false
workflow_completed: true
---

# Brainstorming Session Results

**Facilitator:** Lucas
**Date:** 2026-03-04

## Session Overview

**Topic:** Fila's post-Epic 11 roadmap — strategic direction after v1 delivery
**Goals:** Generate ideas across features, infrastructure, ecosystem, community; prioritize release engineering and benchmarking/competitive analysis

### Session Setup

- Two high-priority threads identified: (1) proper release cadence beyond bleeding-edge, (2) benchmarking and competitive analysis to inform direction
- Seed ideas from prior work: auth, clustering, consumer groups, management GUI
- Practical, data-driven mindset — wants evidence before committing to features

## Technique Selection

**Approach:** AI-Recommended Techniques
**Analysis Context:** Strategic planning for mature infrastructure product with 11 epics delivered

**Recommended Techniques:**

- **Question Storming:** Map the full problem space before generating solutions — surface unknowns about users, performance, market positioning
- **Cross-Pollination:** Learn from how other queue/infra projects (Redis, RabbitMQ, SQS, Kafka, NATS) evolved post-v1
- **First Principles Thinking:** Ground ideas in Fila's core value proposition to filter and prioritize

## Technique Execution Results

### Question Storming (50 questions generated)

**Scale & Performance (Q1-Q7, Q43-Q48):**
- Does Fila scale? What's the single-node throughput ceiling (msg/sec for enqueue, consume, ack)?
- What happens to latency under sustained load — graceful degradation or cliff?
- How does RocksDB behave with millions of queued messages? Compaction stalls?
- Memory footprint profile — linear with queue depth or surprise spikes?
- What's the actual bottleneck — CPU, disk, or network?
- Latency percentiles (p50/p95/p99) under load
- Queue depth impact at 1M, 10M, 100M messages
- Fairness key cardinality scaling — 10 keys vs 10K vs 1M
- Consumer concurrency — 1 vs 10 vs 100 consumers, lease contention
- RocksDB compaction impact on latency

**Adoption & Credibility (Q2, Q8-Q13, Q22-Q26):**
- Is anyone actually using it?
- Who is the first real user — Lucas, or someone else?
- What's the "hello world to production" story?
- Why pick Fila over SQS/Redis/RabbitMQ today? Honest elevator pitch?
- Is single-node forever, or does clustering make it serious?
- Without auth, can anyone deploy in a real environment?
- Does Lua scripting actually matter to users?
- Who is Fila for? Small teams? Embedded systems? Edge? Rust shops?
- Does Fila compete with cloud queues (SQS) or self-hosted (RabbitMQ, NATS)?
- What would make someone write a blog post about Fila?
- Is there a community play, or is this Lucas's project?

**Storage (Q14-Q18):**
- What does "our own storage layer" actually need?
- Could a purpose-built engine outperform RocksDB for queue workloads?
- What's the migration story — swap engines without breaking deployments?
- Is the storage abstraction clean enough to plug in a new backend?
- Would a custom storage layer be a separate project or Fila-internal?

**Deferral & Ordering (Q19-Q21):**
- What's the trigger that turns "deferred" into "now"?
- Is there a correct ordering — auth before clustering or clustering before auth?
- Can you ship "good enough" auth fast, or does it need full RBAC from day one?

**Release Strategy (Q27-Q36):**
- If every merge is a release, what's the versioning scheme?
- Do you even need semver for an application binary?
- How do SDK versions track server versions? Compatibility contract?
- Breaking change policy for the gRPC proto?
- What gates the merge if every merge is a release?
- Do users want stability releases with backported fixes?
- Do SDKs also release on every push to their own main?
- Is install.sh enough, or do users expect brew/apt/Docker?
- Who's responsible for knowing a release is broken?
- What's the rollback story?

**Kafka Competitive Analysis (Q37-Q42, Q49-Q50):**
- When someone "graduates" to Kafka, what specifically triggered the move?
- Is Fila trying to prevent that graduation or serve a different niche?
- Are there Kafka features queue users actually want (replay, consumer groups, partitioning)?
- Is "Kafka semantics in a single binary" viable or a trap?
- Redpanda already did "Kafka but simpler in C++". Is there space for Fila?
- Is the Kafka ecosystem (Schema Registry, Connect, Streams) the real moat?
- Benchmark yourself first or compare against competitors first?

### Cross-Pollination (6 patterns analyzed, 7 ideas generated)

**Patterns analyzed:**
- **SQS** — "zero-ops" aspiration, the invisible queue
- **SQLite** — embeddable, competed with fopen(), most deployed DB on earth
- **Redis** — Swiss Army Knife, "already deployed" wins
- **Kafka → Redpanda** — same API, better runtime, wire compatibility
- **BullMQ** — developer-first queue, dashboard + DX as differentiator
- **NATS** — simple pub/sub that added persistence (JetStream) without losing simplicity
- **etcd/CockroachDB** — embedded Raft, no external dependencies
- **Litestream/LiteFS** — replication without full clustering
- **Prometheus** — explicitly owned "single-node is the product" positioning

**Key cross-pollination insight:** All patterns were ultimately rejected as "downplaying our ambitions." Fila's vision is bigger than being SQLite-for-queues or Prometheus-for-brokers.

### First Principles Thinking (vision crystallized)

**The breakthrough moment:** Fila isn't a lightweight alternative. Fila is the final broker solution.

**Core vision:** "Start with Fila day 1, never leave. Scales like Kafka, works like a queue, deploys like a single binary."

**Fundamental truths identified:**

1. **Kafka won because of the log, not the API.** Fila needs the log's properties (scale, durability, ordering) without the log's developer burden. Queue semantics on top of log-scale infrastructure.

2. **Scaling means partitioning. Partitioning means distribution.** No way around it. The question isn't if clustering, but whether Fila's clustering is better than Kafka's.

3. **The storage layer must be purpose-built.** RocksDB is a placeholder. Queue workloads are specific: sequential writes, TTL expiry, consumer cursors, high-throughput append.

4. **"Zero graduation"** — People avoid Kafka because it's hard to set up, hard to operate, hard to use, and expensive. Fila should eliminate the entire queue ladder (Redis → RabbitMQ → Kafka).

**Critical coupling identified:** Storage engine and clustering must be designed together. Building a storage engine without considering partitioning means rebuilding it later. But the interface can be distributed-aware before the implementation is — design for partitioning, build single-node first, extend to multi-node.

## Idea Organization and Prioritization

### Thematic Organization

**Theme 1: Vision & Positioning**
- "Zero graduation" — replace the entire queue ladder
- "Scales like Kafka, works like a queue" — the log's properties without the log's burden
- Kafka's weaknesses are Fila's pitch — hard to set up, hard to operate, hard to use, expensive

**Theme 2: Benchmarking & Measurement**
- Continuous benchmark suite running on every PR (not every release — shift-left)
- Benchmark yourself first: throughput, latency percentiles, queue depth, fairness key cardinality, consumer concurrency, compaction impact
- Competitive comparison second: head-to-head vs Kafka, RabbitMQ, Redis Streams

**Theme 3: Storage Engine + Clustering (coupled)**
- Purpose-built storage engine optimized for queue/broker access patterns
- Embedded consensus (Raft), zero external dependencies, single binary stays single binary
- Invisible partitioning — users create queues, Fila distributes them
- Queue semantics never leak the log — enqueue, consume, ack. No offsets, no rebalancing.
- Design the storage abstraction with partitioning in mind, build single-node first, extend to multi-node

**Theme 4: Auth & Security**
- Minimum viable: mTLS + API keys
- Required to unblock any real production deployment
- Ordering: comes after clustering — production users need both

**Theme 5: Release & Distribution**
- Continuous delivery from main — every merge is a release
- Versioning scheme TBD (semver vs calver vs something else)
- SDK compatibility contract needed
- Distribution channels: install.sh, brew, apt, Docker

**Theme 6: Developer Experience**
- Web dashboard (parked — not priority but on the list)
- Zero-ops self-healing: auto-compaction, storage management, health self-checks

### Prioritization Results

**Decided priority order:**

1. **Benchmarks** — you can't improve what you can't measure. Establish baseline, find bottlenecks, gate every PR.
2. **Clustering + Storage Engine** — designed together. Purpose-built storage that's partition-aware from day 1. This is the big one.
3. **Auth** — unblock real deployments. mTLS + API keys minimum.
4. **Release** — continuous delivery from main, versioning, SDK compat.
5. **DX** — web dashboard, zero-ops, distribution channels.

**Dropped ideas:**
- Embedded/library mode — Fila is a broker, not a library
- SQS-compatible API — downplays the ambition

**Each step unlocks the next:** benchmarks reveal bottlenecks → storage+clustering eliminates them → auth enables production → release enables distribution → DX enables adoption.

## Session Summary and Insights

**Key Achievements:**
- Crystallized Fila's long-term vision: "the final broker solution" — replaces Kafka by scaling like Kafka but working like a queue out of the box
- Identified "zero graduation" as the core positioning — people should use Fila from day 1 and never need to leave
- Established data-driven priority order: benchmark → cluster+storage → auth → release → DX
- Recognized that storage engine and clustering are a coupled workstream that must be designed together
- Generated 50 strategic questions and 10 ideas across 3 techniques

**Session Reflections:**
- Cross-pollination patterns (SQLite, Prometheus, NATS) were useful for pattern recognition but ultimately rejected as "downplaying ambitions" — a sign that the vision is clear and ambitious
- The "benchmark first" decision reflects strong engineering discipline — measure before optimizing
- Conscious deferral of auth and clustering across 11 epics was validated as the right call — the v1 foundation needed to be solid first
