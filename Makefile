.PHONY: flamegraph flamegraph-lifecycle flamegraph-consume

# Generate a CPU flamegraph for enqueue-only workload (1KB, 30s).
flamegraph:
	./scripts/flamegraph.sh --workload enqueue-only --duration 30 --message-size 1024

# Full message lifecycle (concurrent produce + consume + ack).
flamegraph-lifecycle:
	./scripts/flamegraph.sh --workload lifecycle --duration 30 --message-size 1024

# Consume-only workload (pre-fills queue, then profiles consumption).
flamegraph-consume:
	./scripts/flamegraph.sh --workload consume-only --duration 30 --message-size 1024
