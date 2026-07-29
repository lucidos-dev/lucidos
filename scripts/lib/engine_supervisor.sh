#!/bin/bash
# Engine supervisor. Wraps the engine binary in a restart loop so an
# unexpected kill (SIGKILL from a stale worktree's ports.sh, OOM kill,
# panic) becomes a 1–30 s blip instead of a session-ending outage.
# See `docs/glossary.md` § "Engine supervisor" for the canonical term.
#
# Legitimate stops are pass-through. Three exit codes mean "stay dead":
#   0   — graceful_shutdown completed (engine handled SIGUSR1 or SIGINT)
#   130 — SIGINT default action  (Ctrl-C before handler installed)
#   138 — SIGUSR1 default action (signal arrived before handler installed)
#
# Defensive trap: when web-dev.sh kill_stale_processes does
# `pkill -P <old_web_dev>` it SIGTERMs the supervisor (which is a direct
# child). Without a trap the supervisor would interpret SIGTERM-during-
# wait as "child died with exit 143" and respawn the engine that's about
# to be SIGUSR1'd by the same kill_stale_processes call — fighting the
# rebuild flow. The trap forwards the shutdown intent to the engine as
# SIGUSR1 and exits.

# Usage: run_supervised <pidfile> <logfile> <cmd...>
#
# Writes the running child's pid to <pidfile> on every (re)start so
# kill_stale_processes / stop.sh / is_protected_host_pid all read the
# live pid even after a restart.
run_supervised() {
    local pidfile="$1"
    local logfile="$2"
    shift 2

    # Capture the supervisor's actual PID. `$$` won't work — the supervisor
    # is launched as `( run_supervised ... ) &` from start_engine and bash
    # subshells inherit `$$` from their parent. `$BASHPID` exists in bash
    # 4+ but macOS ships bash 3.2 by default. The portable trick: fork a
    # `sh -c` subprocess and read its `$PPID`, which is the supervisor's
    # actual PID. Without this every supervisor log line + the respawn
    # sidecar would carry the calling web-dev.sh's PID instead of the
    # supervisor's own, defeating self-identification.
    local self_pid
    self_pid=$(sh -c 'echo $PPID')

    # Shared log writer — keeps every supervisor line on the same
    # `[<ts>] [Supervisor pid=N] <msg>` shape so a debug session can
    # grep one supervisor instance out of an interleaved engine.log.
    _sup_log() {
        echo "[$(date '+%Y-%m-%d %H:%M:%S')] [Supervisor pid=$self_pid] $*" >> "$logfile"
    }

    local shutdown_requested=0
    # Without `set +m`, the engine log picks up `[1]+ Done` job-control
    # lines when the backgrounded child exits.
    set +m
    trap 'shutdown_requested=1' SIGTERM SIGINT

    _sup_log "starting; cmd=$*"

    local backoff=1
    local last_failure=0
    while [ "$shutdown_requested" -eq 0 ]; do
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
        # Log every spawn so a respawn cycle reads as a continuous
        # supervisor narrative even when the child crashes before
        # logging anything itself.
        _sup_log "spawned child pid=$child_pid"
        # SIGTERM may have landed between backgrounding and reaching
        # wait — bash runs the trap inline (not inside wait), so wait
        # would otherwise block until the child died naturally. Forward
        # the shutdown intent immediately when that race is observable.
        if [ "$shutdown_requested" -eq 1 ]; then
            kill -USR1 "$child_pid" 2>/dev/null || true
        fi
        # `wait` is interrupted by trapped signals — that's how SIGTERM
        # received during execution flips shutdown_requested for the
        # post-wait branch below.
        wait "$child_pid"
        local exit_code=$?

        if [ "$shutdown_requested" -eq 1 ]; then
            # SIGTERM came in while the engine was running. Forward the
            # shutdown intent — engine's SIGUSR1 handler runs
            # graceful_shutdown. `|| true` because the child may already
            # be gone (signal landed right as it exited on its own).
            kill -USR1 "$child_pid" 2>/dev/null || true
            wait "$child_pid" 2>/dev/null || true
            _sup_log "SIGTERM → forwarded SIGUSR1 to pid $child_pid, exiting"
            break
        fi

        # 0 = graceful_shutdown; 130 = SIGINT default; 138 = SIGUSR1 default
        case "$exit_code" in
            0|130|138)
                _sup_log "Child pid $child_pid exited cleanly ($exit_code), supervisor stopping"
                break
                ;;
        esac

        _sup_log "Child pid $child_pid died unexpectedly (exit $exit_code), restarting after ${backoff}s backoff"

        # Sidecar JSON the next engine reads + emits as
        # `EngineSupervisorRespawned` so the respawn is recorded in the
        # audit timeline even though it happens while the engine is dead.
        # Path is derived from the pidfile location so the supervisor
        # doesn't need a separate argument. Best-effort: a write failure
        # here only loses one audit row, not the respawn itself.
        #
        # Hand-rolled `printf` JSON is safe ONLY while every field stays
        # numeric or an ISO-8601 timestamp (no escaping needed). Adding a
        # free-text field (`cmd`, `error_message`, `workspace`) MUST
        # switch to a real encoder (`jq -n …` — see `scripts/status.sh`
        # for the established pattern).
        local sidecar
        sidecar="$(dirname "$pidfile")/engine.last-death.json"
        local died_at
        died_at="$(date -u '+%Y-%m-%dT%H:%M:%SZ')"
        printf '{"old_pid":%d,"exit_code":%d,"died_at":"%s","supervisor_pid":%d}\n' \
            "$child_pid" "$exit_code" "$died_at" "$self_pid" > "$sidecar" 2>/dev/null || true

        last_failure=$(date +%s)
    done
}
