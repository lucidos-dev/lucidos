#!/bin/bash
# Stop Lucidos engine (workspace-aware, supports multiple concurrent workspaces)
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
            echo "Stop Lucidos engine and frontend for a single workspace."
            echo ""
            echo "Options:"
            echo "  -w, --workspace DIR   Workspace to stop (required)"
            echo "  -f, --force           Also stop the legacy per-workspace PostgreSQL container, if present"
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
source "$SCRIPT_DIR/lib/workspace.sh"

# Stop a single workspace by its path
stop_workspace() {
    local ws="$1"
    local stopped=""

    local engine_pid_file="$ws/.lucidos/engine.pid"
    local frontend_pid_file="$ws/.lucidos/frontend.pid"
    local build_watch_pid_file="$ws/.lucidos/build-watch.pid"

    # The gateway is now ONE shared, machine-global process fronting EVERY
    # workspace (ADR 0014) — do NOT kill it here, or we'd take down every other
    # workspace's proxy. Instead ask it (best-effort, over https then http — the
    # dev gateway serves https when certs exist) to stop just THIS workspace's
    # engine and drop its stack, so the gateway's supervisor won't respawn it.
    # The registry entry survives, so the workspace stays listed in the picker as
    # stopped. The engine.pid SIGUSR1 below is the fallback when the gateway is
    # unreachable. To stop the gateway itself: kill $(cat "$(gateway_pidfile)").
    local gw_pid gw_port slug
    gw_pid="$(cat "$(gateway_pidfile)" 2>/dev/null || true)"
    gw_port="${LUCIDOS_DEV_GATEWAY_PORT:-5251}"
    slug="$(workspace_slug)"
    if [ -n "$gw_pid" ] && kill -0 "$gw_pid" 2>/dev/null; then
        if gateway_curl -sk -X POST "https://localhost:$gw_port/~/api/v1/control/workspaces/$slug/stop" >/dev/null 2>&1 \
           || gateway_curl -s -X POST "http://localhost:$gw_port/~/api/v1/control/workspaces/$slug/stop" >/dev/null 2>&1; then
            echo "Asked shared gateway to stop workspace '$slug' (gateway left running for peers)"
            stopped="1"
        fi
    fi

    # Stop engine via SIGUSR1, not SIGTERM — the engine ignores SIGTERM to
    # survive accidental `xargs kill` from CC subprocess test scripts (see
    # main.rs shutdown_signal). SIGUSR1 is the legitimate stop signal.
    if [ -f "$engine_pid_file" ]; then
        local pid
        pid="$(cat "$engine_pid_file")"
        if kill -0 "$pid" 2>/dev/null; then
            echo "Stopping engine (PID $pid) for $ws"
            kill -USR1 "$pid" 2>/dev/null || true
            stopped="1"
        fi
        rm -f "$engine_pid_file"
    fi

    # Release the frontend marker. ADR 0014: the engine serves dist/ directly —
    # frontend.pid records the SHARED build-watch pid for ref-counting, which
    # release_frontend_marker removes WITHOUT killing (peers may share it); the
    # teardown below decides the shared watch's fate. A distinct dev-server pid
    # (e2e) is killed.
    [ -n "$(release_frontend_marker "$frontend_pid_file")" ] && stopped="1"

    # The --built mode `vite build --watch` is a checkout-level singleton shared
    # by every workspace of this checkout (see build_watch_pidfile in
    # workspace.sh). Clean up a legacy per-workspace build-watch.pid from
    # pre-singleton runs, then tear down the shared build-watch only if THIS was
    # the last workspace serving the frontend (frontend.pid removed above).
    if [ -f "$build_watch_pid_file" ]; then
        local pid
        pid="$(cat "$build_watch_pid_file")"
        if kill -0 "$pid" 2>/dev/null; then
            echo "Stopping legacy per-workspace build-watch (PID $pid) for $ws"
            kill "$pid" 2>/dev/null || true
            stopped="1"
        fi
        rm -f "$build_watch_pid_file"
    fi
    teardown_shared_build_watch_if_idle

    # Stop legacy per-workspace PostgreSQL container if --force. The shared
    # PostgreSQL container is never stopped for one workspace; it serves peers.
    if [ -n "$FORCE" ]; then
        local pg_name
        pg_name=$(printf '%s' "$ws" | cksum | awk '{print $1}')
        if docker inspect "lucidos-pg-$pg_name" >/dev/null 2>&1; then
            echo "Stopping legacy PostgreSQL container lucidos-pg-$pg_name for $ws"
            docker rm -f "lucidos-pg-$pg_name" 2>/dev/null || true
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

resolve_workspace_path
stop_workspace "$WORKSPACE"
