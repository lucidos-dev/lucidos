#!/bin/bash
# Gateway supervisor (dev). Watchdog loop that auto-restarts the machine-global
# workspace gateway on unexpected death (SIGKILL, OOM, panic) but stays out of
# the way of legitimate stops.
#
# DECOUPLED FROM engine_supervisor.sh ON PURPOSE. The gateway is ONE shared,
# machine-global daemon fronting every workspace — its lifecycle must NOT be tied
# to the per-workspace engine, nor to the `web-dev.sh` shell / terminal that
# happened to launch it. (The packaged app already supervises the gateway with a
# dedicated Rust `--service` process under launchd KeepAlive — see
# crates/lucidos-app/src/desktop.rs::run_service; this is the dev equivalent.)
#
# DETACH POLICY — the load-bearing difference from the engine supervisor:
#   trap '' SIGHUP SIGINT SIGTERM
# The gateway supervisor IGNORES terminal/launcher signals, so:
#   - closing the launching terminal (SIGHUP) can't orphan the gateway,
#   - Ctrl-C on web-dev.sh (SIGINT) can't take the shared gateway down,
#   - a stray SIGTERM (e.g. `pkill -P`) can't either.
# The engine supervisor instead FORWARDS these as a graceful stop, because an
# engine is tied to its dev session; the gateway is not. The ONLY legitimate stop
# is SIGUSR1 to the gateway CHILD (workspace.sh's `-b` stop + port-reclaim do
# exactly that), which makes the child exit 0 → the loop below stops and stays
# dead. The gateway binary itself also ignores SIGTERM and exits 0 on SIGUSR1
# (crates/lucidos-gateway/src/server.rs::install_shutdown), so this is consistent.
#
# Clean exit codes (child exited on purpose → stay dead, do NOT respawn):
#   0   — graceful_shutdown completed (gateway handled SIGUSR1 or SIGINT)
#   130 — SIGINT default action  (before handler installed)
#   138 — SIGUSR1 default action (signal arrived before handler installed)
#
# Usage: run_gateway_supervised <pidfile> <logfile> <cmd...>
#
# Writes the running gateway child's pid to <pidfile> on every (re)start so
# start_gateway's reuse-health check, the `-b` stop path, and the documented
# `kill $(cat …/gateway.pid)` all read the live child pid even after a respawn.
run_gateway_supervised() {
    local pidfile="$1"
    local logfile="$2"
    shift 2

    # Capture the supervisor's own PID for self-identifying log lines. `$$` is the
    # parent's in a `( … ) &` subshell on bash 3.2 (macOS default); the portable
    # trick is to read a forked `sh -c`'s $PPID. (Same approach as
    # engine_supervisor.sh.)
    local self_pid
    self_pid=$(sh -c 'echo $PPID')

    _gw_sup_log() {
        echo "[$(date '+%Y-%m-%d %H:%M:%S')] [GatewaySupervisor pid=$self_pid] $*" >> "$logfile"
    }

    # Machine-global daemon: never die with the launching shell/terminal. The
    # graceful stop path signals the CHILD (SIGUSR1), never this supervisor.
    trap '' SIGHUP SIGINT SIGTERM
    # Without `set +m`, the log picks up `[1]+ Done` job-control noise when the
    # backgrounded child exits.
    set +m

    _gw_sup_log "starting; cmd=$*"

    local backoff=1
    local last_failure=0
    while true; do
        # Backoff only after a recent failure — first launch is immediate.
        if [ "$last_failure" -gt 0 ]; then
            local now
            now=$(date +%s)
            if [ $((now - last_failure)) -lt 10 ]; then
                sleep "$backoff"
                backoff=$((backoff * 2))
                [ "$backoff" -gt 30 ] && backoff=30
            else
                backoff=1
            fi
        fi

        "$@" >> "$logfile" 2>&1 &
        local child_pid=$!
        echo "$child_pid" > "$pidfile"
        _gw_sup_log "spawned child pid=$child_pid"

        wait "$child_pid"
        local exit_code=$?

        # 0 = graceful_shutdown; 130 = SIGINT default; 138 = SIGUSR1 default.
        case "$exit_code" in
            0|130|138)
                _gw_sup_log "child pid $child_pid exited cleanly ($exit_code), supervisor stopping"
                break
                ;;
        esac

        _gw_sup_log "child pid $child_pid died unexpectedly (exit $exit_code), restarting after ${backoff}s backoff"

        # Sidecar JSON recording the unexpected gateway death, for an audit
        # breadcrumb. Hand-rolled printf JSON is safe ONLY while every field stays
        # numeric or an ISO-8601 timestamp (no escaping needed) — switch to `jq -n`
        # before adding any free-text field. Best-effort: a write failure only
        # loses one breadcrumb, not the respawn.
        local sidecar
        sidecar="$(dirname "$pidfile")/gateway.last-death.json"
        local died_at
        died_at="$(date -u '+%Y-%m-%dT%H:%M:%SZ')"
        printf '{"old_pid":%d,"exit_code":%d,"died_at":"%s","supervisor_pid":%d}\n' \
            "$child_pid" "$exit_code" "$died_at" "$self_pid" > "$sidecar" 2>/dev/null || true

        last_failure=$(date +%s)
    done
}
