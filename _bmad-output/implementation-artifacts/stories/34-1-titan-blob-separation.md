# Story 34.1: Titan Blob Separation — Reduce RocksDB Write Amplification

Status: pending

## Story

As a developer,
I want to evaluate and optionally enable TiKV-style blob separation for message payloads in RocksDB,
So that write amplification is reduced from 10-50x to ~2-5x without replacing RocksDB entirely.

## Acceptance Criteria

1. **Given** RocksDB level compaction produces ~33x write amplification for 1KB messages
   **When** Titan-style blob separation is enabled for CF_MESSAGES
   **Then** message payloads (large values) are stored in separate BlobFiles
   **And** the LSM tree contains only keys + pointers (small values)
   **And** compaction moves only keys/pointers, not full payloads

2. **Given** blob separation is enabled
   **When** enqueue throughput is measured
   **Then** write throughput improves (expected 2-6x for 1KB+ values based on TiKV benchmarks)
   **And** write amplification is measured and compared to baseline

3. **Given** blob separation changes read patterns
   **When** consume throughput is measured (with in-memory delivery from 33.1)
   **Then** performance is acceptable for the storage-fallback path (consumer lag scenario)
   **And** the range query performance trade-off (40% to several times worse) is documented but acceptable because Fila's hot path uses point lookups, not range queries

4. **Given** this is a lower-risk alternative to full storage replacement
   **When** the evaluation is complete
   **Then** a recommendation is made: Titan sufficient vs custom storage needed
   **And** if Titan provides sufficient throughput, stories 34.2/34.3 may be skipped

5. **Given** the evaluation
   **When** results are documented
   **Then** write amplification, throughput, disk I/O, and memory impact are all quantified

## Tasks / Subtasks

- [ ] Task 1: Enable blob separation in RocksDB config
  - [ ] 1.1 Enable Titan / BlobDB for CF_MESSAGES
  - [ ] 1.2 Configure blob file size, GC settings, min blob size threshold
  - [ ] 1.3 Verify existing data migrates during compaction

- [ ] Task 2: Benchmark write throughput
  - [ ] 2.1 Run enqueue benchmark (1KB, 64KB messages)
  - [ ] 2.2 Measure write amplification (compaction stats)
  - [ ] 2.3 Compare against Plateau 2 baseline

- [ ] Task 3: Benchmark read performance
  - [ ] 3.1 Measure point lookup latency (consumer lag fallback path)
  - [ ] 3.2 Verify in-memory delivery path unaffected

- [ ] Task 4: Write evaluation and recommendation
  - [ ] 4.1 Document throughput, write amp, disk I/O, memory
  - [ ] 4.2 Go/no-go for Stories 34.2/34.3 (custom storage)

## Dev Notes

### Titan / BlobDB

TiKV demonstrated 2x QPS for 1KB values, 6x for 32KB values with Titan blob separation. RocksDB has native BlobDB support (since v6.18) that implements the same pattern. The key insight: message bodies are the large values; separating them from the LSM tree means compaction only processes small key+pointer entries.

### Risk Assessment

This is the lowest-risk Plateau 3 story. It uses the same RocksDB API, requires only configuration changes + minor code adjustments, and is proven at scale by TiKV. If Titan provides 2-3x write throughput improvement, it may be sufficient to reach the target — making the custom storage engine (34.2/34.3) unnecessary.

### Key Files to Modify

| File | Change |
|------|--------|
| `crates/fila-core/src/storage/rocksdb.rs` | Enable BlobDB / Titan for CF_MESSAGES |
| `crates/fila-core/src/config.rs` | Blob separation config options |

### References

- [Source: TiKV Titan Design](https://www.pingcap.com/blog/titan-storage-engine-design-and-implementation/) — Architecture and benchmarks
- [Source: TiKV Titan Overview](https://docs.pingcap.com/tidb/stable/titan-overview/) — Configuration guide
- [Research: technical-kafka-parity-profiling-strategy-2026-03-25.md, Pattern 6 variant] — Blob separation analysis
