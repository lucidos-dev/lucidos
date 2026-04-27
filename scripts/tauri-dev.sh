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
show_banner "tauri"

# Kill old Tauri process before launching a new one
while IFS= read -r tauri_pid; do
    [ -z "$tauri_pid" ] && continue
    echo "Killing old Tauri process (PID $tauri_pid)..."
    kill "$tauri_pid" 2>/dev/null || true
done < <(pgrep -f "cargo tauri dev" 2>/dev/null || true)

# Don't call start_vite — cargo tauri dev runs its own beforeDevCommand (npm run dev)
# which starts Vite. Just export the port env vars Vite needs.
export VITE_PORT="$INTERNAL_VITE_PORT"
export API_PORT="$ENGINE_PORT"

trap 'cleanup_processes; exit 0' SIGINT SIGTERM

echo "Launching Tauri desktop app..."

# Run Tauri in foreground — when user closes the window, cleanup trap fires.
# --config: override devUrl to point at the engine's port (which reverse-proxies to Vite)
cd "$FRONTEND_DIR"
cargo tauri dev \
    --no-watch \
    --config "{\"build\":{\"devUrl\":\"$PROTO://localhost:$ENGINE_PORT\"}}"
