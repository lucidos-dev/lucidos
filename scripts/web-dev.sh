#!/bin/bash
# Start Lucidos in browser-based development mode:
#   - PostgreSQL with pgvector in Docker
#   - Rust engine runs natively on macOS (fast iteration)
#   - Frontend served as a built bundle by default (vite build --watch + vite
#     preview); pass --hmr for the live Vite dev server
#   - Supports multiple workspaces running concurrently
set -e

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_DIR="$(dirname "$SCRIPT_DIR")"
FRONTEND_DIR="$PROJECT_DIR/crates/lucidos-app"
SCRIPT_NAME="web-dev.sh"

source "$SCRIPT_DIR/lib/ports.sh"
source "$SCRIPT_DIR/lib/workspace.sh"
source "$SCRIPT_DIR/lib/preflight.sh"

cd "$PROJECT_DIR"

parse_dev_args "$@"
check_prereqs
resolve_workspace
allocate_ports "$WORKSPACE"
detect_tls
setup_postgres
kill_stale_processes
build_or_find_engine
build_sdk
swap_ports

start_engine

# In --engine-only mode, skip Vite and exit after engine starts (used by restart API
# when running under Tauri — Tauri manages its own Vite, we just need a fresh engine)
if [ -n "$ENGINE_ONLY" ]; then
    show_banner "engine-only"
    exit 0
fi

start_vite
show_banner "web"

trap 'cleanup_processes; exit 0' SIGINT SIGTERM

if [ -n "$FOLLOW_LOG" ]; then
    echo "Press Ctrl+C to stop"
    tail -n 20 -f "$ENGINE_LOG"
elif [ -t 1 ]; then
    # Print the listening line for confirmation, then exit
    tail -n 100 "$ENGINE_LOG" | grep -m 1 "API server listening"
    echo ""
    echo "Tail log:  tail -f $ENGINE_LOG"
    echo "Stop:      ./scripts/stop.sh -w $WORKSPACE"
else
    # Spawned by restart API — keep web-dev.sh alive while the engine is
    # alive so the spawned process isn't orphaned. Prefer waiting on the
    # supervisor (our direct child, survives engine restarts); fall back
    # to polling the pidfile when start_engine reused an existing engine
    # and didn't spawn a fresh supervisor.
    if [ -n "${ENGINE_SUPERVISOR_PID:-}" ]; then
        wait "$ENGINE_SUPERVISOR_PID"
    else
        while [ -s "$ENGINE_PIDFILE" ] && kill -0 "$(cat "$ENGINE_PIDFILE")" 2>/dev/null; do
            sleep 5
        done
    fi
fi
