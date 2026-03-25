#!/usr/bin/env bash
# Benchmark InMemoryEngine vs RocksDB — isolate storage overhead.
#
# Runs the profile-workload binary (enqueue-only and lifecycle) with both
# storage backends and reports throughput numbers.
set -euo pipefail

DURATION="${1:-10}"
MSG_SIZE="${2:-1024}"
CONCURRENCY="${3:-1}"

echo "=== InMemory vs RocksDB Benchmark ==="
echo "Duration: ${DURATION}s | Message size: ${MSG_SIZE}B | Concurrency: ${CONCURRENCY}"
echo ""

# Build release binaries
echo "Building release binaries..."
cargo build --release -p fila-server -p fila-bench 2>&1 | tail -1

run_workload() {
    local storage_type="$1"
    local workload="$2"
    local extra_env=""

    if [ "$storage_type" = "memory" ]; then
        extra_env="FILA_STORAGE=memory"
    fi

    env $extra_env \
        PROFILE_WORKLOAD="$workload" \
        PROFILE_DURATION="$DURATION" \
        PROFILE_MSG_SIZE="$MSG_SIZE" \
        PROFILE_CONCURRENCY="$CONCURRENCY" \
        cargo run --release --bin profile-workload 2>&1
}

echo "--- RocksDB: enqueue-only ---"
run_workload "rocksdb" "enqueue-only"
echo ""

echo "--- InMemory: enqueue-only ---"
run_workload "memory" "enqueue-only"
echo ""

echo "--- RocksDB: lifecycle (enqueue + consume + ack) ---"
run_workload "rocksdb" "lifecycle"
echo ""

echo "--- InMemory: lifecycle (enqueue + consume + ack) ---"
run_workload "memory" "lifecycle"
echo ""

echo "--- RocksDB: consume-only ---"
run_workload "rocksdb" "consume-only"
echo ""

echo "--- InMemory: consume-only ---"
run_workload "memory" "consume-only"
echo ""

echo "Done. Compare the msg counts above to derive throughput (count / ${DURATION}s)."
