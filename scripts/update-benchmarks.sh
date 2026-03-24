#!/usr/bin/env bash
# Update docs/benchmarks.md from local benchmark JSON results.
#
# Usage:
#   ./scripts/update-benchmarks.sh                        # uses existing JSON files
#   ./scripts/update-benchmarks.sh --run                  # runs self-benchmarks first
#   ./scripts/update-benchmarks.sh --doc /path/to/doc.md  # custom doc path
#
# Any flags not consumed by this script are forwarded to bench-update-docs.
# Expects to be run from the repo root.

set -euo pipefail

SELF_BENCH="crates/fila-bench/bench-results.json"
COMPETITIVE_DIR="bench/competitive/results"
DOC="docs/benchmarks.md"
RUN_BENCH=false
EXTRA_ARGS=()

while [[ $# -gt 0 ]]; do
    case "$1" in
        --run)
            RUN_BENCH=true
            shift
            ;;
        --self-bench)
            SELF_BENCH="$2"
            shift 2
            ;;
        --competitive-dir)
            COMPETITIVE_DIR="$2"
            shift 2
            ;;
        --doc)
            DOC="$2"
            shift 2
            ;;
        *)
            EXTRA_ARGS+=("$1")
            shift
            ;;
    esac
done

if [[ "$RUN_BENCH" == true ]]; then
    echo "Running self-benchmarks..."
    cargo bench -p fila-bench --bench system
    echo ""
    echo "Note: competitive benchmarks require Docker."
    echo "Run 'cd bench/competitive && make bench-competitive' separately if needed."
fi

if [[ ! -f "$SELF_BENCH" ]]; then
    echo "Error: $SELF_BENCH not found."
    echo "Run benchmarks first: cargo bench -p fila-bench --bench system"
    echo "Or use: ./scripts/update-benchmarks.sh --run"
    exit 1
fi

echo "Building bench-update-docs..."
cargo build --release -p fila-bench --bin bench-update-docs

echo "Updating $DOC..."
./target/release/bench-update-docs \
    --self-bench "$SELF_BENCH" \
    --competitive-dir "$COMPETITIVE_DIR" \
    --doc "$DOC" \
    "${EXTRA_ARGS[@]+"${EXTRA_ARGS[@]}"}"
