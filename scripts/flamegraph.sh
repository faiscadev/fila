#!/usr/bin/env bash
#
# Generate CPU flamegraphs for fila-server workloads.
#
# Uses dtrace on macOS (requires sudo) or perf on Linux.
# Stack traces are collapsed and rendered via inferno.
#
# Prerequisites:
#   cargo install inferno
#   Linux: perf (linux-tools-common / linux-tools-$(uname -r))
#   macOS: dtrace (ships with Xcode, needs sudo)
#
# Usage:
#   ./scripts/flamegraph.sh [OPTIONS]
#
# Options:
#   --workload NAME      enqueue-only|consume-only|lifecycle (default: enqueue-only)
#   --duration SECS      how long to run the workload (default: 30)
#   --message-size N     payload size in bytes (default: 1024)
#   --concurrency N      number of concurrent producers/consumers (default: 1)
#   --output PATH        output SVG path (default: target/flamegraphs/<workload>-<timestamp>.svg)
#   --server-addr ADDR   profile an external server instead of starting one
#   --sample-hz N        sampling frequency in Hz (default: 997)
#   --help               show this help
#
set -euo pipefail

WORKLOAD="enqueue-only"
DURATION=30
MSG_SIZE=1024
CONCURRENCY=1
OUTPUT=""
SERVER_ADDR=""
SAMPLE_HZ=997

while [[ $# -gt 0 ]]; do
    case "$1" in
        --workload)     WORKLOAD="$2";     shift 2 ;;
        --duration)     DURATION="$2";     shift 2 ;;
        --message-size) MSG_SIZE="$2";     shift 2 ;;
        --concurrency)  CONCURRENCY="$2";  shift 2 ;;
        --output)       OUTPUT="$2";       shift 2 ;;
        --server-addr)  SERVER_ADDR="$2";  shift 2 ;;
        --sample-hz)    SAMPLE_HZ="$2";    shift 2 ;;
        --help|-h)
            sed -n '/^# Usage:/,/^set/p' "$0" | grep '^#' | sed 's/^# \{0,1\}//'
            exit 0
            ;;
        *)
            echo "error: unknown option: $1" >&2
            exit 1
            ;;
    esac
done

# Validate workload.
case "$WORKLOAD" in
    enqueue-only|consume-only|lifecycle) ;;
    *)
        echo "error: unknown workload '$WORKLOAD'" >&2
        echo "available: enqueue-only, consume-only, lifecycle" >&2
        exit 1
        ;;
esac

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
cd "$ROOT"

# Output path.
FLAMEGRAPH_DIR="$ROOT/target/flamegraphs"
mkdir -p "$FLAMEGRAPH_DIR"
TIMESTAMP="$(date +%Y%m%d-%H%M%S)"
if [[ -z "$OUTPUT" ]]; then
    OUTPUT="$FLAMEGRAPH_DIR/${WORKLOAD}-${TIMESTAMP}.svg"
fi

# Check inferno is installed.
for cmd in inferno-flamegraph; do
    if ! command -v "$cmd" &>/dev/null; then
        echo "error: $cmd not found. Install with: cargo install inferno" >&2
        exit 1
    fi
done

echo "=== fila flamegraph ==="
echo "workload:    $WORKLOAD"
echo "duration:    ${DURATION}s"
echo "msg_size:    ${MSG_SIZE}B"
echo "concurrency: $CONCURRENCY"
echo "sample_hz:   $SAMPLE_HZ"
echo "output:      $OUTPUT"
echo ""

# Build release with debug symbols.
echo "building release binaries (with debug symbols)..."
CARGO_PROFILE_RELEASE_DEBUG=2 cargo build --release --bin fila-server --bin fila --bin profile-workload 2>&1 | tail -1
echo ""

SERVER_BIN="$ROOT/target/release/fila-server"
WORKLOAD_BIN="$ROOT/target/release/profile-workload"

# Export workload config.
export PROFILE_WORKLOAD="$WORKLOAD"
export PROFILE_DURATION="$DURATION"
export PROFILE_MSG_SIZE="$MSG_SIZE"
export PROFILE_CONCURRENCY="$CONCURRENCY"
if [[ -n "$SERVER_ADDR" ]]; then
    export PROFILE_SERVER_ADDR="$SERVER_ADDR"
fi

OS="$(uname -s)"
STACKS_FILE=$(mktemp)
trap 'rm -f "$STACKS_FILE"' EXIT

case "$OS" in
    Linux)
        if ! command -v perf &>/dev/null; then
            echo "error: perf not found. Install linux-tools-common or linux-tools-\$(uname -r)" >&2
            exit 1
        fi

        echo "profiling with perf (Linux)..."
        echo ""

        # If profiling the embedded server (no PROFILE_SERVER_ADDR), we profile
        # the workload binary which includes the server process in-tree.
        # For external servers, we'd need to attach perf to the server PID.
        if [[ -z "$SERVER_ADDR" ]]; then
            # Profile the workload binary (which starts its own server).
            perf record -F "$SAMPLE_HZ" -g --call-graph dwarf -o "$STACKS_FILE.perf" \
                "$WORKLOAD_BIN" 2>&1

            perf script -i "$STACKS_FILE.perf" | inferno-collapse-perf > "$STACKS_FILE"
            rm -f "$STACKS_FILE.perf"
        else
            # Start workload in background, attach perf to external server.
            "$WORKLOAD_BIN" &
            WORKLOAD_PID=$!

            # Find server PID by addr — caller must provide it or we guess.
            echo "error: --server-addr with perf requires manual PID. Use embedded server instead." >&2
            kill "$WORKLOAD_PID" 2>/dev/null
            exit 1
        fi
        ;;

    Darwin)
        if ! command -v dtrace &>/dev/null; then
            echo "error: dtrace not found" >&2
            exit 1
        fi

        DTRACE_FILE=$(mktemp)

        if [[ -z "$SERVER_ADDR" ]]; then
            # Start the workload binary (which spawns its own fila-server).
            "$WORKLOAD_BIN" &
            WORKLOAD_PID=$!

            # Give the server a moment to start and the workload to warm up.
            sleep 3

            # Find the fila-server child process.
            SERVER_PID=$(pgrep -P "$WORKLOAD_PID" fila-server 2>/dev/null || true)
            if [[ -z "$SERVER_PID" ]]; then
                # The workload binary starts fila-server as a subprocess via BenchServer.
                # It might be a grandchild. Search more broadly.
                SERVER_PID=$(pgrep -f "fila-server" | grep -v "$$" | head -1 || true)
            fi

            if [[ -z "$SERVER_PID" ]]; then
                echo "error: could not find fila-server process" >&2
                kill "$WORKLOAD_PID" 2>/dev/null
                exit 1
            fi

            echo "workload PID: $WORKLOAD_PID"
            echo "server PID:   $SERVER_PID"
            echo ""

            # Profile the server with dtrace for the duration of the workload.
            # The -x ustackframes=100 gives us deep stacks.
            PROFILE_SECS=$((DURATION - 3))  # subtract warmup time
            if [[ "$PROFILE_SECS" -lt 5 ]]; then
                PROFILE_SECS=5
            fi

            echo "profiling with dtrace for ${PROFILE_SECS}s (requires sudo)..."
            sudo dtrace -x ustackframes=100 \
                -n "profile-$SAMPLE_HZ /pid == $SERVER_PID/ { @[ustack()] = count(); }" \
                -o "$DTRACE_FILE" &
            DTRACE_PID=$!

            # Wait for the workload to finish, then stop dtrace.
            wait "$WORKLOAD_PID" 2>/dev/null || true
            sleep 1
            sudo kill "$DTRACE_PID" 2>/dev/null || true
            wait "$DTRACE_PID" 2>/dev/null || true

            inferno-collapse-dtrace < "$DTRACE_FILE" > "$STACKS_FILE"
            rm -f "$DTRACE_FILE"
        else
            echo "error: --server-addr with dtrace not yet supported. Use embedded server." >&2
            exit 1
        fi
        ;;

    *)
        echo "error: unsupported OS: $OS" >&2
        exit 1
        ;;
esac

# Generate the flamegraph SVG.
TITLE="fila-server $WORKLOAD (${MSG_SIZE}B, ${DURATION}s)"
inferno-flamegraph --title "$TITLE" < "$STACKS_FILE" > "$OUTPUT"

echo ""
echo "=== flamegraph generated ==="
echo "output: $OUTPUT"
echo "size:   $(wc -c < "$OUTPUT" | tr -d ' ') bytes"
echo ""
echo "open in a browser for interactive zoom/search."
