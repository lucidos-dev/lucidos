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

# See the same guard in web-dev.sh — a worktree-rooted PROJECT_DIR pins the whole
# stack to a throwaway checkout.
assert_stack_not_worktree_pinned "$PROJECT_DIR" || exit 1

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

# Kill old Tauri process before launching a new one.
#
# `pgrep -f` proposes candidates; the EXECUTABLE decides. A coding agent
# carries the engine's thread history inside a roughly 22 KB
# `--append-system-prompt` argument, so a thread quoting this phrase matches
# the pattern and used to be killed by it. Same class as ADR 0025, and as
# `select_cargo_lock_holders` in scripts/lib/workspace.sh.
while IFS= read -r tauri_pid; do
    [ -z "$tauri_pid" ] && continue
    tauri_comm="$(ps -p "$tauri_pid" -o comm= 2>/dev/null || true)"
    [ "${tauri_comm##*/}" = "cargo" ] || continue
    if command -v is_protected_host_pid >/dev/null 2>&1 && is_protected_host_pid "$tauri_pid"; then
        continue
    fi
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
