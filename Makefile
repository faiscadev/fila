.PHONY: flamegraph flamegraph-lifecycle flamegraph-consume flamegraph-batch

# Generate a CPU flamegraph with sensible defaults (enqueue-only, 1KB, 30s).
flamegraph:
	./scripts/flamegraph.sh --workload enqueue-only --duration 30 --message-size 1024

# Full message lifecycle (concurrent produce + consume + ack).
flamegraph-lifecycle:
	./scripts/flamegraph.sh --workload lifecycle --duration 30 --message-size 1024

# Consume-only workload (pre-fills queue, then profiles consumption).
flamegraph-consume:
	./scripts/flamegraph.sh --workload consume-only --duration 30 --message-size 1024

# Batch enqueue workload (auto-batching enabled).
flamegraph-batch:
	./scripts/flamegraph.sh --workload batch-enqueue --duration 30 --message-size 1024
