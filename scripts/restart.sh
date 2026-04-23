#!/bin/bash
# Restart CognOS engine for a specific workspace
set -e

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_DIR="$(dirname "$SCRIPT_DIR")"

cd "$PROJECT_DIR"

# Parse arguments — collect everything to pass through
WORKSPACE=""
ARGS=()
while [[ $# -gt 0 ]]; do
    case $1 in
        -w|--workspace)
            WORKSPACE="$2"
            ARGS+=("$1" "$2")
            shift 2
            ;;
        -h|--help)
            echo "Usage: $0 -w <workspace> [OPTIONS]"
            echo ""
            echo "Stops and restarts CognOS for a specific workspace."
            echo ""
            echo "Options:"
            echo "  -w, --workspace DIR   Workspace directory (required)"
            echo "  -b, --build           Build engine before starting"
            echo "  -r, --release         Build in release mode"
            echo "  -h, --help            Show this help"
            exit 0
            ;;
        *)
            ARGS+=("$1")
            shift
            ;;
    esac
done

if [ -z "$WORKSPACE" ]; then
    echo "Error: -w <workspace> is required for restart."
    echo ""
    echo "Usage: $0 -w <workspace>"
    exit 1
fi

# Stop this workspace
"$SCRIPT_DIR/stop.sh" -w "$WORKSPACE"

# Wait for cleanup
sleep 2

# Start with all passed arguments
"$SCRIPT_DIR/web-dev.sh" "${ARGS[@]}"
