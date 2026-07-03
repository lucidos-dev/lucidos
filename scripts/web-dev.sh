#!/bin/bash
# Start Lucidos in browser-based development mode (ADR 0014).
#
# ── This is the DEVELOPER entry point ───────────────────────────────────────
# It optimizes for fast iteration: a DEBUG engine build by default (quick to
# compile) and a long-lived `vite build --watch` that rebuilds dist/ on every
# source change. END USERS and the one-click installer do NOT run this — they run
# scripts/run.sh, a thin wrapper that reuses this exact orchestration but flips
# two defaults for *using* Lucidos: a RELEASE engine build and a ONE-SHOT
# `vite build` (no watcher left running). The ONLY behavioural fork between the
# two is the LUCIDOS_FRONTEND_ONESHOT branch in start_vite (scripts/lib/
# workspace.sh) — keep this script's dev defaults intact; don't "fix" it to
# behave like the user launcher.
# ────────────────────────────────────────────────────────────────────────────
#
#   - PostgreSQL with pgvector in Docker
#   - Rust engine runs natively on macOS (fast iteration), serving the built
#     frontend (dist/) DIRECTLY via LUCIDOS_STATIC_DIR — no Vite in the serving
#     path. `vite build --watch` (a checkout-level singleton) just rebuilds dist/
#     on source change; the engine serves it.
#   - The standalone `lucidos-gateway` binary fronts the workspace at /<slug>/
#     and spawns + supervises the loopback engine.
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

# --engine-build (ADR: new-version-available/switch flow): rebuild the on-disk
# engine binary in the BACKGROUND while the running engine keeps serving — no
# kill, no respawn, no Vite. The Apply-triggered background rebuild spawns this;
# the running engine detects the new on-disk build-id and surfaces "New version
# available", and the disruptive switch respawns onto it separately. Build only,
# then exit — no postgres/ports/kill needed to compile.
if [ -n "$ENGINE_BUILD_ONLY" ]; then
    build_or_find_engine
    build_sdk
    exit 0
fi

detect_tls
setup_postgres
kill_stale_processes
build_or_find_engine
build_sdk
swap_ports

# Gateway model by default (ADR 0014): the standalone lucidos-gateway binary
# fronts the workspace at /<slug>/ and spawns + supervises the engine.
# LUCIDOS_NO_GATEWAY=1 falls back to the legacy direct-engine model (engine on
# the user-facing port, app served at /).
if [ -n "${LUCIDOS_NO_GATEWAY:-}" ]; then
    start_engine
else
    seed_gateway_registry
    start_gateway
fi

# In --engine-only mode, skip Vite and exit after the engine/gateway starts
# (used by the restart API). In gateway mode start_gateway already reused the
# running gateway and asked it to respawn this workspace's stack onto the
# rebuilt binary, so there's nothing more to do.
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
