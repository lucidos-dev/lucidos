#!/bin/bash
# Start Lucidos in browser-based development mode:
#   - PostgreSQL with pgvector in Docker
#   - Rust engine runs natively on macOS (fast iteration)
#   - Frontend served via Vite dev server
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
    # Spawned by restart API — wait for engine to avoid orphaning
    wait $ENGINE_PID
fi
