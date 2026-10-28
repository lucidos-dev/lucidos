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
#
# The window runs from its published launch binary, inside a minimal .app on
# macOS. Sibling links resolve the engine, gateway and CLI in the launch dir.
# It moves to a new build only when its engine does, on "Switch to new
# version" (ADR 0461). Frontend changes arrive through the engine, as in any
# browser.
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
resolve_workspace
allocate_ports "$WORKSPACE"
detect_tls
setup_postgres
kill_stale_processes
build_or_find_engine
# build_or_find_engine builds the window once this checkout has one, so only
# the first launch builds it here.
DEV_WINDOW_LAUNCH_DIR="$(launch_bin_dir)"
if [ ! -f "$(dev_window_bin "$DEV_WINDOW_LAUNCH_DIR")" ]; then
    build_dev_window "$DEV_WINDOW_LAUNCH_DIR"
fi
# Resolved, because stop_dev_windows matches a window by the exact path it was
# started from, and a later launch must name it the same way.
DEV_WINDOW_LAUNCH_DIR="$(cd "$DEV_WINDOW_LAUNCH_DIR" && pwd -P)"
DEV_WINDOW_BIN="$(dev_window_bin "$DEV_WINDOW_LAUNCH_DIR")"
# Before the stack starts, so /api/v1/sdk.js is ready immediately.
generate_build_outputs
swap_ports

# Gateway model by default (ADR 0014), matching web-dev.sh.
if [ -n "${LUCIDOS_NO_GATEWAY:-}" ]; then
    start_engine
else
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

# Replace this checkout's previous window, if one is still running.
stop_dev_windows "$DEV_WINDOW_BIN" "$DEV_WINDOW_LAUNCH_DIR/lucidos-app"

# Leaves the shared gateway and every peer workspace running. See
# cleanup_processes, which releases only this workspace's own markers.
trap 'cleanup_processes; exit 0' SIGINT SIGTERM

echo "Launching Tauri desktop app at $WINDOW_URL ..."

# Run the window in the foreground. Closing it ends this script; Ctrl-C signals
# the whole process group and fires the trap. No file watcher restarts it:
# run_dev_window relaunches the newer published binary once its engine has
# switched.
cd "$FRONTEND_DIR"
run_dev_window "$DEV_WINDOW_BIN" "$WINDOW_URL"
