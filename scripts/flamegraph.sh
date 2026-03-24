#!/usr/bin/env bash
#
# Generate CPU flamegraphs for Fila workloads.
#
# Prerequisites:
#   macOS: cargo install flamegraph (uses DTrace, no extra setup)
#   Linux: cargo install flamegraph + perf (linux-tools-common)
#
# Usage:
#   ./scripts/flamegraph.sh [OPTIONS]
#
# Options:
#   --workload NAME    enqueue-only|consume-only|lifecycle|batch-enqueue (default: enqueue-only)
#   --duration SECS    how long to run the workload (default: 30)
#   --message-size N   payload size in bytes (default: 1024)
#   --concurrency N    number of concurrent producers/consumers (default: 1)
#   --heap             generate allocation flamegraph via DHAT
#   --output PATH      output SVG path (default: target/flamegraphs/<workload>-<timestamp>.svg)
#   --help             show this help
#
set -euo pipefail

WORKLOAD="enqueue-only"
DURATION=30
MSG_SIZE=1024
CONCURRENCY=1
HEAP=false
OUTPUT=""

usage() {
    sed -n '/^# Usage:/,/^[^#]/p' "$0" | head -n -1 | sed 's/^# *//'
    echo ""
    echo "Options:"
    echo "  --workload NAME    enqueue-only|consume-only|lifecycle|batch-enqueue (default: enqueue-only)"
    echo "  --duration SECS    how long to run the workload (default: 30)"
    echo "  --message-size N   payload size in bytes (default: 1024)"
    echo "  --concurrency N    number of concurrent producers/consumers (default: 1)"
    echo "  --heap             generate allocation flamegraph via DHAT"
    echo "  --output PATH      output SVG path (default: target/flamegraphs/<workload>-<timestamp>.svg)"
    echo "  --help             show this help"
}

while [[ $# -gt 0 ]]; do
    case "$1" in
        --workload)
            WORKLOAD="$2"
            shift 2
            ;;
        --duration)
            DURATION="$2"
            shift 2
            ;;
        --message-size)
            MSG_SIZE="$2"
            shift 2
            ;;
        --concurrency)
            CONCURRENCY="$2"
            shift 2
            ;;
        --heap)
            HEAP=true
            shift
            ;;
        --output)
            OUTPUT="$2"
            shift 2
            ;;
        --help|-h)
            usage
            exit 0
            ;;
        *)
            echo "error: unknown option: $1"
            usage
            exit 1
            ;;
    esac
done

# Validate workload name.
case "$WORKLOAD" in
    enqueue-only|consume-only|lifecycle|batch-enqueue) ;;
    *)
        echo "error: unknown workload '$WORKLOAD'"
        echo "available: enqueue-only, consume-only, lifecycle, batch-enqueue"
        exit 1
        ;;
esac

# Resolve project root (parent of scripts/).
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
cd "$ROOT"

# Output directory.
FLAMEGRAPH_DIR="$ROOT/target/flamegraphs"
mkdir -p "$FLAMEGRAPH_DIR"

TIMESTAMP="$(date +%Y%m%d-%H%M%S)"
if [[ -z "$OUTPUT" ]]; then
    OUTPUT="$FLAMEGRAPH_DIR/${WORKLOAD}-${TIMESTAMP}.svg"
fi

echo "=== fila flamegraph ==="
echo "workload:    $WORKLOAD"
echo "duration:    ${DURATION}s"
echo "msg_size:    ${MSG_SIZE}B"
echo "concurrency: $CONCURRENCY"
echo "output:      $OUTPUT"
echo ""

# Build the server and workload binary in release mode.
echo "building release binaries (with debug symbols for profiling)..."
CARGO_PROFILE_RELEASE_DEBUG=true cargo build --release --bin fila-server --bin fila --bin profile-workload 2>&1 | tail -1

# Check that cargo-flamegraph is installed.
if ! command -v cargo-flamegraph &>/dev/null && ! command -v flamegraph &>/dev/null; then
    echo "error: cargo-flamegraph not found"
    echo "install it with: cargo install flamegraph"
    exit 1
fi

# Set environment variables for the workload binary.
export PROFILE_WORKLOAD="$WORKLOAD"
export PROFILE_DURATION="$DURATION"
export PROFILE_MSG_SIZE="$MSG_SIZE"
export PROFILE_CONCURRENCY="$CONCURRENCY"

if [[ "$HEAP" == "true" ]]; then
    echo ""
    echo "heap profiling: running with DHAT..."
    echo "note: DHAT requires recompilation with dhat feature — this is a placeholder"
    echo "for now, generating a CPU flamegraph instead"
    echo ""
fi

echo "running workload under profiler..."
echo ""

# Use cargo flamegraph to profile the workload binary.
# On macOS, this uses DTrace. On Linux, it uses perf.
# CARGO_PROFILE_RELEASE_DEBUG=true ensures symbols are available.
CARGO_PROFILE_RELEASE_DEBUG=true cargo flamegraph \
    --bin profile-workload \
    --output "$OUTPUT" \
    -- 2>&1 || {
    # If --root fails (common on macOS without SIP disabled), retry without it.
    echo ""
    echo "retrying without --root flag..."
    CARGO_PROFILE_RELEASE_DEBUG=true cargo flamegraph \
        --bin profile-workload \
        --output "$OUTPUT" \
        -- 2>&1
}

echo ""
echo "=== flamegraph generated ==="
echo "output: $OUTPUT"
echo ""

# Print a summary if the SVG was generated.
if [[ -f "$OUTPUT" ]]; then
    SIZE=$(wc -c < "$OUTPUT" | tr -d ' ')
    echo "SVG size: ${SIZE} bytes"

    # Extract top functions from the SVG (the flamegraph encodes function names).
    # This is a best-effort summary — flamegraph SVGs contain sample counts in title attributes.
    echo ""
    echo "top functions (by sample count):"
    # Parse title="function_name (N samples, X%)" from the SVG.
    if command -v perl &>/dev/null; then
        perl -nle '
            while (/title="([^"]+)\s+\((\d+)\s+samples?,\s+([\d.]+)%\)"/g) {
                printf "%6.1f%%  %6d  %s\n", $3, $2, $1;
            }
        ' "$OUTPUT" | sort -t'%' -k1 -rn | head -10
    else
        echo "(install perl for function summary)"
    fi

    echo ""
    echo "open the SVG in a browser for the interactive flamegraph."
fi
