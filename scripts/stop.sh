#!/bin/bash
# Stop CognOS engine (workspace-aware, supports multiple concurrent workspaces)
set -e

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_DIR="$(dirname "$SCRIPT_DIR")"

cd "$PROJECT_DIR"

# Parse arguments
WORKSPACE=""
FORCE=""
while [[ $# -gt 0 ]]; do
    case $1 in
        -w|--workspace) WORKSPACE="$2"; shift 2 ;;
        -f|--force) FORCE="1"; shift ;;
        -h|--help)
            echo "Usage: $0 -w <workspace> [OPTIONS]"
            echo ""
            echo "Stop CognOS engine and frontend for a single workspace."
            echo ""
            echo "Options:"
            echo "  -w, --workspace DIR   Workspace to stop (required)"
            echo "  -f, --force           Also stop the workspace's PostgreSQL container"
            echo "  -h, --help            Show this help"
            exit 0
            ;;
        *) echo "Unknown option: $1"; exit 1 ;;
    esac
done

if [ -z "$WORKSPACE" ]; then
    echo "Error: -w <workspace> is required. Refusing to stop all workspaces." >&2
    echo "Run '$0 --help' for usage." >&2
    exit 1
fi

source "$SCRIPT_DIR/lib/sleep.sh"

# Stop a single workspace by its path
stop_workspace() {
    local ws="$1"
    local stopped=""

    local engine_pid_file="$ws/.cognos/engine.pid"
    local frontend_pid_file="$ws/.cognos/frontend.pid"

    # Stop engine
    if [ -f "$engine_pid_file" ]; then
        local pid
        pid="$(cat "$engine_pid_file")"
        if kill -0 "$pid" 2>/dev/null; then
            echo "Stopping engine (PID $pid) for $ws"
            kill "$pid" 2>/dev/null || true
            stopped="1"
        fi
        rm -f "$engine_pid_file"
    fi

    # Stop frontend
    if [ -f "$frontend_pid_file" ]; then
        local pid
        pid="$(cat "$frontend_pid_file")"
        if kill -0 "$pid" 2>/dev/null; then
            echo "Stopping frontend (PID $pid) for $ws"
            kill "$pid" 2>/dev/null || true
            stopped="1"
        fi
        rm -f "$frontend_pid_file"
    fi

    # Stop PostgreSQL container if --force
    if [ -n "$FORCE" ]; then
        local pg_name
        pg_name=$(printf '%s' "$ws" | cksum | awk '{print $1}')
        if docker inspect "cognos-pg-$pg_name" >/dev/null 2>&1; then
            echo "Stopping PostgreSQL container cognos-pg-$pg_name for $ws"
            docker rm -f "cognos-pg-$pg_name" 2>/dev/null || true
            stopped="1"
        fi
    fi

    release_sleep_lock "$ws"

    if [ -n "$stopped" ]; then
        echo "Stopped: $ws"
    else
        echo "Not running: $ws"
    fi
}

if [ -d "$WORKSPACE" ]; then
    WORKSPACE="$(cd "$WORKSPACE" && pwd)"
fi
stop_workspace "$WORKSPACE"
