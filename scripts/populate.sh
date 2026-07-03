#!/bin/bash
# Populate Lucidos with test data (2 years of history)
set -e

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_DIR="$(dirname "$SCRIPT_DIR")"
DEFAULT_WORKSPACE="$PROJECT_DIR/test-workspace"

cd "$PROJECT_DIR"

source "$SCRIPT_DIR/lib/workspace.sh"

# Parse arguments
WORKSPACE="${LUCIDOS_WORKSPACE:-$DEFAULT_WORKSPACE}"
CLEAN=""
BUILD=""
while [[ $# -gt 0 ]]; do
    case $1 in
        -w|--workspace) WORKSPACE="$2"; shift 2 ;;
        -c|--clean) CLEAN="1"; shift ;;
        -b|--build) BUILD="1"; shift ;;
        -h|--help)
            echo "Usage: $0 [OPTIONS]"
            echo ""
            echo "Populates the workspace with 2 years of test history data."
            echo "This includes events, artifacts, notifications, and memory entries."
            echo ""
            echo "Options:"
            echo "  -w, --workspace DIR   Use specified workspace (default: test-workspace)"
            echo "  -c, --clean           Clear existing data before populating"
            echo "  -b, --build           Build before running"
            echo "  -h, --help            Show this help"
            exit 0
            ;;
        *) echo "Unknown option: $1"; exit 1 ;;
    esac
done

# Best-effort: missing workspace is not fatal here (populate may bootstrap it).
resolve_workspace_path 2>/dev/null || {
    if [[ "$WORKSPACE" != */* ]]; then
        WORKSPACE="$HOME/workspaces/$WORKSPACE"
    fi
}

# Stop engine for this workspace if running
if [ -f "$WORKSPACE/.lucidos/engine.pid" ] && kill -0 "$(cat "$WORKSPACE/.lucidos/engine.pid")" 2>/dev/null; then
    echo "Stopping running engine for $WORKSPACE..."
    "$SCRIPT_DIR/stop.sh" -w "$WORKSPACE"
    sleep 2
fi

# Clean if requested
if [ -n "$CLEAN" ]; then
    echo "Cleaning workspace data..."
    rm -rf "$WORKSPACE/data"
    rm -rf "$WORKSPACE/.lucidos"
fi

# Build if requested
if [ -n "$BUILD" ]; then
    echo "Building..."
    cargo build --locked --bin populate_memory
fi

echo ""
echo "Populating test data..."
echo "  Workspace: $WORKSPACE"
echo ""

LUCIDOS_WORKSPACE="$WORKSPACE" cargo run --locked --bin populate_memory

echo ""
echo "Done! Start the engine with:"
echo "  ./scripts/start.sh -w $WORKSPACE"
