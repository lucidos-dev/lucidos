#!/bin/bash
# Start Lucidos and open a Tauri desktop window.
#
# Without -b: starts from the latest build (or reuses a running stack).
# With -b: rebuilds engine + gateway first, then starts.
#
# This is web-dev.sh plus a window, and the two share one launch sequence. The
# window loads the GATEWAY at /<slug>/, which is the door the packaged app
# navigates to (crates/lucidos-app/src/desktop.rs). Pointing it at the engine's
# own port instead is what broke: the gateway already owns that port, so the
# launcher spawned a second engine and it died with AddrInUse. See
# docs/plans/2026-09-18-tauri-dev-window-goes-through-the-gateway.md.
#
# LUCIDOS_NO_GATEWAY=1 falls back to the legacy direct-engine model, the same
# fork web-dev.sh has. The window then loads the engine root.
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

# A worktree-rooted PROJECT_DIR pins the whole stack to a throwaway checkout
# (ADR 0021). Scope by whether the MACHINE-GLOBAL gateway is in play, exactly as
# web-dev.sh does: that daemon outlives this shell and propagates its paths into
# every engine it spawns, so the opt-out must not buy it.
if [ -n "${LUCIDOS_NO_GATEWAY:-}" ]; then
    assert_stack_not_worktree_pinned "$PROJECT_DIR" || exit 1
else
    assert_stack_not_worktree_pinned "$PROJECT_DIR" gateway || exit 1
fi

check_prereqs
check_tauri_cli
resolve_workspace
allocate_ports "$WORKSPACE"
detect_tls
setup_postgres
kill_stale_processes
build_or_find_engine
# Before the stack starts, so /api/v1/sdk.js is ready immediately.
build_sdk
swap_ports

# Gateway model by default (ADR 0014), matching web-dev.sh.
if [ -n "${LUCIDOS_NO_GATEWAY:-}" ]; then
    start_engine
else
    seed_gateway_registry
    start_gateway
fi

# ADR 0014: the engine serves the built dist/ directly (LUCIDOS_STATIC_DIR, set
# by swap_ports) — there is no live Vite dev server in the serving path. Start
# the shared `vite build --watch` so dist/ exists + rebuilds on change; the
# window loads it through whichever door start_gateway / start_engine opened.
# (tauri.conf's beforeDevCommand is empty: the build-watch is managed here,
# not by Tauri.)
start_vite
show_banner "tauri"

# Reads the same two signals show_banner branches on, so the banner and the
# window can never name different URLs.
WINDOW_URL="$(desktop_window_url \
    "${GATEWAY_MODE:-}" "$PROTO" "$GATEWAY_PORT" "${GATEWAY_WS_ID:-}" "$ENGINE_PORT")"

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

# Leaves the shared gateway and every peer workspace running. See
# cleanup_processes, which releases only this workspace's own markers.
trap 'cleanup_processes; exit 0' SIGINT SIGTERM

echo "Launching Tauri desktop app at $WINDOW_URL ..."

# Run Tauri in foreground — when user closes the window, cleanup trap fires.
# --config: override devUrl to the door serving this workspace.
# Trailing `-- --locked` reaches the inner cargo build, so this window is built
# strictly from the committed Cargo.lock and errors on manifest drift instead of
# rewriting it (ADR 0020). Same form as `cargo tauri build` in build-dmg.sh.
cd "$FRONTEND_DIR"
cargo tauri dev \
    --no-watch \
    --config "{\"build\":{\"devUrl\":\"$WINDOW_URL\"}}" \
    -- --locked
