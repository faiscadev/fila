# Sprint Change Proposal — 2026-03-10

## Section 1: Issue Summary

**Trigger:** Pre-implementation technical research on clustering architecture (`_bmad/docs/research/decoupled-scheduler-sharded-storage.md`) revealed that the planned approach for Epics 13 and 14 is incompatible with the optimal clustering model.

**Core Finding:** A CockroachDB-style single-binary Raft-per-queue model is the right architecture for Fila. This has three implications:

1. **RocksDB is sufficient** — Under Raft, it serves as a local state machine backend. Raft owns durability and recovery. A custom WAL/segment log/compaction engine (Epic 13 Stories 13.2-13.5) is redundant.
2. **Shard by queue, not by partition** — The research validates queue-level Raft groups over Kafka-style partitions. The PartitionId-based storage trait (Epic 13 Story 13.1) is incompatible.
3. **One Raft log replicates everything** — Messages, DRR state, leases, config. No separate "storage replication" vs "scheduler replication." This simplifies Epic 14 significantly.

**Evidence:** Research document with production precedents (Google Borg, Apache Pulsar+BookKeeper, Meta FOQS), throughput analysis (30-50K Raft scheduling decisions/sec), latency analysis (1.4-3ms p50 with pipelining to ~360µs steady-state), and comprehensive failure mode coverage with mitigations.

## Section 2: Impact Analysis

### Epic Impact

**Epic 13 (Purpose-Built Storage Engine) — 5 open PRs, none merged:**
- **Stories 13.2-13.5 (WAL, indexing, compaction, cutover):** Fully redundant. Raft owns durability/recovery; RocksDB handles local state.
- **Story 13.1 (Storage trait):** Concept valuable, but PartitionId design conflicts with queue-level sharding. Needs redesign with Fila-domain terms.
- **Action:** Close all 5 PRs. Replace with 2-story "Storage Abstraction & Clustering Prep" epic.

**Epic 14 (Clustering & Horizontal Scaling) — all backlog:**
- **Story 14.2 (Partitioned Queue Management):** Directly conflicts — partition-based distribution replaced by queue-level Raft groups.
- **Stories 14.1, 14.3, 14.4, 14.5:** Conceptually similar goals but implementation details change significantly for Raft-per-queue model.
- **Action:** Full replan with queue-level Raft groups, single-binary model, simplified HA.

**Epics 15-17:** No impact. Auth, release engineering, and DX are orthogonal to clustering architecture.

### Artifact Conflicts

| Artifact | Impact | Action |
|----------|--------|--------|
| PRD (Phase 3 section) | Described partition-based coupled workstream | Updated: Raft-per-queue model, FR66 deferred, NFR30-33 deferred |
| PRD (FR66) | Purpose-built storage engine | Deferred to post-clustering optimization |
| PRD (NFR30-33) | Storage engine performance targets | Deferred with FR66 |
| Architecture doc | Listed custom storage and clustering as deferred | Updated: clustering decided (Raft-per-queue), custom storage deferred |
| Epics doc | Epic 13 (5 stories), Epic 14 (5 stories, partition-based) | Updated: Epic 13 (2 stories), Epic 14 (5 stories, Raft-per-queue) |
| Sprint status | Listed old story IDs | Updated with new story IDs |

## Section 3: Recommended Approach

**Selected: Direct Adjustment**

- Close Epic 13 PRs #49-53 (zero merge cost — none were merged)
- Delete Epic 13 feature branches
- Reshape Epic 13: 5 stories → 2 stories (storage trait cleanup + phase 2 viability seams)
- Replan Epic 14: partition-based → Raft-per-queue model (5 stories retained, content redesigned)
- Update PRD, architecture doc, epics doc, sprint status

**Rationale:**
- ~5,100 lines of unmerged code. Zero sunk cost in production.
- Research provides clear, well-reasoned replacement architecture with production precedents.
- Storage trait cleanup (new 13.1) is smaller and better-targeted than the original.
- Epic 14 replan aligns with CockroachDB/Kafka KRaft proven model.
- No scope reduction — clustering goal unchanged, approach improved.

**Effort:** Medium (document updates only; no code to revert)
**Risk:** Low (nothing merged, architecture research is thorough)

## Section 4: Detailed Change Proposals

### PRs Closed
- PR #49: `feat/13.1-storage-trait-rocksdb-adapter` — PartitionId design conflicts
- PR #50: `feat/13.2-write-path-wal-segment-log` — Raft owns durability
- PR #51: `feat/13.3-read-path-indexing` — Custom indexes unnecessary
- PR #52: `feat/13.4-background-maintenance-compaction` — Custom compaction unnecessary
- PR #53: `feat/13.5-integration-cutover-validation` — No custom engine

### Branches Deleted
- `feat/13.1-storage-trait-rocksdb-adapter`
- `feat/13.2-write-path-wal-segment-log`
- `feat/13.3-read-path-indexing`
- `feat/13.4-background-maintenance-compaction`
- `feat/13.5-integration-cutover-validation`

### Epic 13: Reshaped

**OLD:** Purpose-Built Storage Engine (5 stories: storage trait, WAL, indexing, compaction, cutover)
**NEW:** Storage Abstraction & Clustering Prep (2 stories: clean storage trait, phase 2 viability seams)

See `epics.md` for full story acceptance criteria.

### Epic 14: Replanned

**OLD:** Partition-based clustering (Raft consensus, partitioned queue management, partition routing, partition replication, cluster observability)
**NEW:** Raft-per-queue clustering (Raft integration, queue-level Raft groups, transparent routing to Raft leaders, Raft-based replication/failover, cluster observability)

Key differences:
- Queue = Raft group (not queue = partitions across nodes)
- Leader handles everything for its queues (scheduling + storage + delivery)
- Followers have full replicated state — failover is just leader election
- No cross-node DRR merging needed (leader does all DRR)
- No partition data migration on rebalance (leadership transfer only)
- Ack-after-replicate and fencing tokens are explicit safety requirements

See `epics.md` for full story acceptance criteria.

### PRD Updates
- FR66: Deferred (purpose-built storage engine → post-clustering optimization)
- NFR30-33: Deferred with FR66
- Phase 3 section: Rewritten for Raft-per-queue model
- FR67-70: Updated wording (partitions → Raft groups/leaders)

### Architecture Doc Updates
- Deferred decisions: Custom storage engine marked as deferred with rationale
- New "Decided (Post-Research)" entry for clustering architecture

## Section 5: Implementation Handoff

**Scope:** Major — fundamental replan of two epics based on architectural research.

**Actions completed:**
- [x] PRs #49-53 closed with explanatory comments
- [x] Feature branches deleted
- [x] `epics.md` updated (Epic 13 reshaped, Epic 14 replanned)
- [x] `sprint-status.yaml` updated (new story IDs)
- [x] `architecture.md` updated (clustering decision recorded)
- [x] `prd.md` updated (FR66 deferred, NFR30-33 deferred, Phase 3 rewritten)
- [x] Sprint Change Proposal written

**Next steps:**
- Begin Epic 13 implementation with new Story 13.1 (Clean Storage Trait Abstraction)
- Story 13.2 (Phase 2 Viability Seams) follows
- Epic 14 implementation begins after Epic 13 completes
