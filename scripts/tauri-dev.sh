#!/bin/bash
# Start Lucidos and open a Tauri desktop window.
#
# Without -b: starts engine from latest build (or reuses if already running).
# With -b: rebuilds engine first, then starts.
set -e

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_DIR="$(dirname "$SCRIPT_DIR")"
FRONTEND_DIR="$PROJECT_DIR/crates/lucidos-app"
SCRIPT_NAME="tauri-dev.sh"

source "$SCRIPT_DIR/lib/ports.sh"
source "$SCRIPT_DIR/lib/workspace.sh"
source "$SCRIPT_DIR/lib/preflight.sh"

cd "$PROJECT_DIR"

parse_dev_args "$@"
check_prereqs
check_tauri_cli
resolve_workspace
allocate_ports "$WORKSPACE"
detect_tls
setup_postgres
swap_ports

kill_stale_processes
build_or_find_engine

# Build SDK before engine starts so /api/v1/sdk.js is ready immediately
ensure_frontend_deps
build_sdk

start_engine

# ADR 0014: the engine serves the built dist/ directly (LUCIDOS_STATIC_DIR, set
# by swap_ports) — there is no live Vite dev server in the serving path. Start
# the shared `vite build --watch` so dist/ exists + rebuilds on change; the
# window (devUrl below) loads it from the engine. (tauri.conf's beforeDevCommand
# is now empty — the build-watch is managed here, not by Tauri.)
start_vite
show_banner "tauri"

# Kill old Tauri process before launching a new one
while IFS= read -r tauri_pid; do
    [ -z "$tauri_pid" ] && continue
    echo "Killing old Tauri process (PID $tauri_pid)..."
    kill "$tauri_pid" 2>/dev/null || true
done < <(pgrep -f "cargo tauri dev" 2>/dev/null || true)

trap 'cleanup_processes; exit 0' SIGINT SIGTERM

echo "Launching Tauri desktop app..."

# Run Tauri in foreground — when user closes the window, cleanup trap fires.
# --config: override devUrl to the engine's port, which serves the built dist/.
cd "$FRONTEND_DIR"
cargo tauri dev \
    --no-watch \
    --config "{\"build\":{\"devUrl\":\"$PROTO://localhost:$ENGINE_PORT\"}}"
