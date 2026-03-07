#!/usr/bin/env bash
# Run Fila's built-in benchmark suite and copy results to the competitive output dir.
# This ensures Fila results use the same BenchReport format as the competitor benchmarks.
#
# Usage: ./bench-fila.sh [output-dir]

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
REPO_ROOT="$(cd "$SCRIPT_DIR/../.." && pwd)"
OUTPUT_DIR="$(cd "${1:-.}" && pwd)"

echo ""
echo "############################################################"
echo "  Benchmarking FILA"
echo "############################################################"
echo ""

cd "$REPO_ROOT"

# Build the server, CLI, and bench tools
echo "[fila] Building..."
cargo build -p fila-server -p fila-cli -p fila-bench --bins 2>&1 | tail -1

# Run the benchmark suite
echo "[fila] Running benchmarks..."
cargo bench -p fila-bench --bench system

# Copy results
cp crates/fila-bench/bench-results.json "$OUTPUT_DIR/bench-fila.json"
echo ""
echo "Results written to: $OUTPUT_DIR/bench-fila.json"
