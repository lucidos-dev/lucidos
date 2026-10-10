#!/bin/bash
# e2e_gateway.sh: which gateway the e2e engine belongs to (ADR 0385).
#
# The e2e engine is started directly, so it inherits its caller's environment.
# From a coding-agent session that names the dev gateway and the `dev`
# workspace. Such an engine reports its boot as `dev`, and would restart `dev`.
# So the harness decides the gateway identity itself, and never inherits it:
# both variables name a live gateway that lists this workspace at the engine's
# port, or neither is set.
#
# LUCIDOS_E2E_GATEWAY picks where that gateway comes from:
#   auto (default)  a gateway already running: the inherited port, then dev's
#   own             start a session-scoped gateway (GitHub shards)
#
# Sourced by e2e.sh, after workspace.sh. Tested by e2e_gateway_test.sh.

# The identity the engine gets: "<gateway-port> <workspace-id>", or nothing.
# Pure. $1 is the port the gateway lists for the workspace, $2 the engine's.
e2e_gateway_identity() {
    local listed="$1" engine_port="$2" gateway_port="$3" ws_id="$4"
    [ -n "$listed" ] && [ -n "$gateway_port" ] && [ -n "$ws_id" ] || return 0
    [ "$listed" = "$engine_port" ] || return 0
    printf '%s %s\n' "$gateway_port" "$ws_id"
}

# The gateway ports to ask in auto mode, deduplicated. Pure.
e2e_gateway_auto_candidates() {
    local inherited="$1" dev="$2"
    [ -z "$inherited" ] || echo "$inherited"
    [ -z "$dev" ] || [ "$dev" = "$inherited" ] || echo "$dev"
}

# The port the gateway on $1 lists for workspace $2, or nothing.
e2e_gateway_listed_port() {
    local port="$1" ws_id="$2" scheme body
    for scheme in https http; do
        body="$(gateway_curl -sfk --max-time "${LUCIDOS_HEALTH_PROBE_TIMEOUT_S:-2}" \
            "$scheme://127.0.0.1:$port/~/api/v1/control/workspaces" 2>/dev/null)" || continue
        printf '%s' "$body" | python3 -c '
import json, sys
try:
    data = json.load(sys.stdin)
except ValueError:
    sys.exit(0)
for w in data.get("workspaces", []):
    if w.get("id") == sys.argv[1]:
        print(w.get("port", ""))
' "$ws_id"
        return 0
    done
}

e2e_gateway_dir() { echo "$E2E_WORKSPACE/.lucidos/e2e-gateway"; }

# The live pid of the harness's own gateway, or nothing. Always exits 0: the
# entry scripts run under `set -e`, and "no gateway" is an answer. A pidfile can
# outlive its process, so the pid counts only while its argv[0] is the gateway
# binary (ADR 0025): a reused pid is never signalled.
_e2e_gateway_live_pid() {
    local pid argv0
    pid="$(cat "$(e2e_gateway_dir)/gateway.pid" 2>/dev/null)" || return 0
    case "$pid" in '' | *[!0-9]*) return 0 ;; esac
    argv0="$(ps -o command= -p "$pid" 2>/dev/null)" || return 0
    argv0="${argv0%% *}"
    if [ "${argv0##*/}" = lucidos-gateway ]; then echo "$pid"; fi
}

_e2e_free_port() {
    python3 -c 'import socket; s = socket.socket(); s.bind(("127.0.0.1", 0)); print(s.getsockname()[1])'
}

# The environment the harness's own gateway runs with, one NAME=value per line.
# Pure. Isolated from the machine-global gateway: its own data dir, port and
# loopback bind, with the hook socket off.
e2e_gateway_env() {
    local data="$1" port="$2" engine_bin="$3" static_dir="$4"
    printf '%s\n' \
        "LUCIDOS_GATEWAY_DATA=$data" \
        "LUCIDOS_API_PORT=$port" \
        "LUCIDOS_GATEWAY_BIND_ADDR=127.0.0.1" \
        "LUCIDOS_HOOK_PORT=0" \
        "LUCIDOS_ENGINE_BIN=$engine_bin" \
        "LUCIDOS_STATIC_DIR=$static_dir"
}

# Start the harness's own gateway, or reuse the one this session started.
# Sets E2E_GATEWAY_PORT. It only ever adopts the engine the harness starts: the
# registry lists the workspace at the engine's port with autostart off, and
# nothing navigates to /<slug>/, so it never spawns an engine or a Postgres.
start_e2e_gateway() {
    local dir pid port
    dir="$(e2e_gateway_dir)"
    pid="$(_e2e_gateway_live_pid)"
    if [ -n "$pid" ]; then
        E2E_GATEWAY_PORT="$(cat "$dir/port")"
        return 0
    fi
    # The gateway refuses a worktree engine binary with no opt-out (ADR 0021).
    # Say so here rather than inside its boot log.
    # shellcheck disable=SC2153 # ENGINE_BIN is workspace.sh's, set before any call
    case "$ENGINE_BIN" in
        */.lucidos/worktrees/*)
            echo "ERROR: LUCIDOS_E2E_GATEWAY=own cannot run from a coding-agent worktree:" >&2
            echo "       the gateway refuses an engine binary built in one (ADR 0021)." >&2
            return 1
            ;;
    esac

    mkdir -p "$dir"
    port="$(_e2e_free_port)" || return 1
    (
        export LUCIDOS_GATEWAY_DATA="$dir" GATEWAY_PORT="$port"
        seed_gateway_registry
    ) || return 1

    local env_args=() line
    while IFS= read -r line; do env_args+=("$line"); done <<EOF
$(e2e_gateway_env "$dir" "$port" "$ENGINE_BIN" "$FRONTEND_DIR/dist")
EOF
    env -u LUCIDOS_GATEWAY_PORT -u LUCIDOS_WORKSPACE_ID -u LUCIDOS_GATEWAY_BIND_ALL \
        -u LUCIDOS_TLS_CERT -u LUCIDOS_TLS_KEY \
        -u LUCIDOS_GATEWAY_TLS_CERT -u LUCIDOS_GATEWAY_TLS_KEY \
        "${env_args[@]}" "$GATEWAY_BIN" >>"$dir/gateway.log" 2>&1 &
    pid=$!
    echo "$pid" >"$dir/gateway.pid"
    echo "$port" >"$dir/port"

    local _
    for _ in {1..60}; do
        if curl -sf --max-time 1 "http://127.0.0.1:$port/~/api/v1/health" >/dev/null 2>&1; then
            echo "e2e gateway: started its own on :$port (PID $pid, log $dir/gateway.log)"
            E2E_GATEWAY_PORT="$port"
            return 0
        fi
        kill -0 "$pid" 2>/dev/null || break
        sleep 0.5
    done
    echo "ERROR: the e2e gateway did not come up on :$port. Its log ends:" >&2
    tail -n 20 "$dir/gateway.log" >&2
    stop_e2e_gateway
    return 1
}

# What the gateway log on $1 says it did, in one line. The harness's gateway
# should only adopt, so a start or respawn line here is the thing to read.
# `grep -c` exits 1 on a count of zero, which `set -e` would take as a failure.
e2e_gateway_log_summary() {
    local adopted started
    adopted="$(grep -c 'adopted the engine already running' "$1" 2>/dev/null)" || true
    started="$(grep -cE 'lazy-starting|respawn|failed to start' "$1" 2>/dev/null)" || true
    echo "e2e gateway: adopted the engine ${adopted:-0} time(s), started or respawned one ${started:-0} time(s)"
}

# Stop the harness's own gateway. SIGUSR1 is its stop: it ignores SIGTERM.
# A no-op when this session started none.
stop_e2e_gateway() {
    local dir pid _
    dir="$(e2e_gateway_dir)"
    pid="$(_e2e_gateway_live_pid)"
    if [ -n "$pid" ]; then
        e2e_gateway_log_summary "$dir/gateway.log"
        echo "Stopping the e2e gateway (PID $pid)..."
        kill -USR1 "$pid" 2>/dev/null || true
        for _ in {1..20}; do
            kill -0 "$pid" 2>/dev/null || break
            sleep 0.5
        done
    fi
    rm -f "$dir/gateway.pid" "$dir/port"
}

# Give the engine about to start its gateway identity, or none. Call after
# swap_ports, so ENGINE_PORT is the e2e engine's port, and before start_engine.
attach_e2e_gateway() {
    local mode="${LUCIDOS_E2E_GATEWAY:-auto}" ws_id candidates port identity=""
    ws_id="$(workspace_slug)"
    case "$mode" in
        own)
            start_e2e_gateway || return 1
            candidates="$E2E_GATEWAY_PORT"
            ;;
        auto)
            candidates="$(e2e_gateway_auto_candidates \
                "${LUCIDOS_GATEWAY_PORT:-}" "${LUCIDOS_DEV_GATEWAY_PORT:-$DEFAULT_DEV_GATEWAY_PORT}")"
            ;;
        *)
            echo "ERROR: LUCIDOS_E2E_GATEWAY must be auto or own, not '$mode'" >&2
            return 1
            ;;
    esac

    # shellcheck disable=SC2153 # ENGINE_PORT is workspace.sh's, set by swap_ports
    for port in $candidates; do
        identity="$(e2e_gateway_identity \
            "$(e2e_gateway_listed_port "$port" "$ws_id")" "$ENGINE_PORT" "$port" "$ws_id")"
        [ -z "$identity" ] || break
    done

    if [ -n "$identity" ]; then
        export LUCIDOS_GATEWAY_PORT="${identity%% *}" LUCIDOS_WORKSPACE_ID="${identity#* }"
        echo "e2e gateway: the engine is '$LUCIDOS_WORKSPACE_ID' to the gateway on :$LUCIDOS_GATEWAY_PORT"
        return 0
    fi
    unset LUCIDOS_GATEWAY_PORT LUCIDOS_WORKSPACE_ID
    if [ "$mode" = own ]; then
        echo "ERROR: the e2e gateway does not list '$ws_id' at :$ENGINE_PORT" >&2
        return 1
    fi
    echo "e2e gateway: none lists '$ws_id' at :$ENGINE_PORT, so the engine runs without one"
}
