#!/bin/bash
# preflight_reclaim.sh: stop every lucidos-engine that should not be running
# before a nightly build, and prove what that freed.
#
# Two callers: scripts/preflight-reclaim-engines.sh (the entry point) and
# scripts/lib/preflight_reclaim_test.sh (its hermetic test).

# THREE THINGS THIS GETS RIGHT, each of which made the previous pasted bash
# snippet a silent no-op on every nightly run.
#
#   1. THE SIGNAL. The engine installs a SIGTERM ignorer on purpose, so that a
#      stray `xargs kill` from a coding-agent test cannot stop it
#      (crates/lucidos-engine/src/main.rs). A SIGTERM pass could never work.
#      Nothing here sends a raw signal at all.
#   2. THE PATH. A non-dev workspace does not live under ~/workspaces. Every
#      path is read out of the target engine's own environment, never built
#      from an assumed layout. A guessed path makes stop.sh print "Workspace
#      not found" and exit 1, which reads like a clean no-op.
#   3. THE STOP THAT STICKS. A bare SIGUSR1 stops the process, and the gateway
#      supervisor respawns it in the same second with a fresh pid. Only the
#      full scripts/stop.sh path holds: it asks the shared gateway to drop the
#      workspace first, then SIGUSR1s the engine.

# So watching the host after the stop is the whole point of the script. A
# reclaimable engine that is still up afterwards is a FAILURE of the step, not
# a note. A reclaim that cannot show what it freed must not look like success.
#
# THE WATCH IS A POLL, NOT A SINGLE DELAYED RE-SCAN.
#
# A one-shot delay has to be longer than the worst case to be safe, and is then
# wrong in both directions: slow when the host settles at once, and silent when
# the engine takes longer to die than the delay. It was 10s, which is shorter
# than a supervisor respawn, so a run reported success while the workspace came
# back 20s later. A window shorter than the failure mode it verifies against is
# not a verification. The poll below ends the moment the answer is provable,
# either way.

# IT NEVER STOPS A KEEP-LIST WORKSPACE, AND IT CANNOT STOP ITS OWN ENGINE.
#
# The default list is the picker's two first-run name suggestions. A packaged
# install runs under launchd with KeepAlive true, so stopping one respawns it
# and only churns the host. The dev workspace is doubly protected: pgrep cannot
# see an ancestor of the calling process, so the engine this script runs inside
# is never in the list at all. The keep-list guard stays regardless. That
# ancestor property belongs to pgrep, not to a decision this file makes, and a
# guard we can read here beats one we would have to remember.

# The available-memory reading is the one in host_memory_guard.sh: free plus
# speculative plus purgeable plus file-backed. Do not invent a second formula,
# and never report "free RAM", which is near zero on a healthy macOS host.
# shellcheck source=host_memory_guard.sh
source "$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)/host_memory_guard.sh"
# For `proc_env_value`, the one `ps -E` parser. scripts/lib/workspace.sh reads
# the owning gateway's port with the same function.
# shellcheck source=proc_env.sh
source "$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)/proc_env.sh"

RECLAIM_TAG="[preflight-reclaim]"

# The repo root (the checkout) whose scripts/stop.sh gets called. It is the ROOT,
# never the scripts/ dir, because the call site appends scripts/stop.sh. This
# file lives in scripts/lib/, so the default climbs two levels: scripts/lib to
# scripts to root. The test points it at a sandbox holding a recording stub, so
# no real workspace is ever stopped.
RECLAIM_PROJECT_DIR="${RECLAIM_PROJECT_DIR:-$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)}"

# Workspaces this script must never stop, matched EXACTLY so that `devbox` is
# not caught by `dev`. Space separated.
_reclaim_keep_list() {
    printf '%s' "${LUCIDOS_RECLAIM_KEEP:-dev personal}"
}

# A whole number of seconds, or the default $2 when $1 is not one.
_reclaim_seconds() {
    case "${1:-}" in
        '' | *[!0-9]*) printf '%s' "$2" ;;
        *) printf '%s' "$1" ;;
    esac
}

# ── the watch window, derived from what can bring an engine back ────────
#
# Two paths return a stopped workspace, and the numbers below come from theirs.
# Both files are read by preflight_reclaim_test.sh, which fails if this
# derivation stops holding. A comment alone would rot in silence.
#
#   crates/lucidos-gateway/src/server.rs  the supervisor respawn
#     SUPERVISE_INTERVAL   2s   one probe, so also the poll cadence here
#     DEAD_MISS_THRESHOLD  2    misses before an exited engine is respawned
#     RESPAWN_BACKOFF      5s   floor on the gap before an attempt
#
#   crates/lucidos-app/index.html         the client lazy-start
#     the boot watchdog reloads the document after 15s, and retries once, so a
#     navigation can reach the gateway 30s after the engine stops answering
#
# Quiet window: 2*2 + 5 + 2*15 = 39s of continuous absence. Deadline: 39 + 20,
# the extra being twice the engine's own 10s graceful-shutdown budget
# (crates/lucidos-engine/src/main.rs), since only the HTTP half of its shutdown
# is bounded by that constant.

# Seconds of continuous absence that prove nothing is bringing this back.
_reclaim_quiet_s() { _reclaim_seconds "${LUCIDOS_RECLAIM_QUIET_S:-39}" 39; }

# Hard cap on the whole watch, drain included.
_reclaim_deadline_s() { _reclaim_seconds "${LUCIDOS_RECLAIM_DEADLINE_S:-59}" 59; }

# Gap between scans. The supervisor decides once per SUPERVISE_INTERVAL, so a
# faster poll cannot see anything a 2s one misses.
_reclaim_poll_s() { _reclaim_seconds "${LUCIDOS_RECLAIM_POLL_S:-2}" 2; }

# The retired knob, named rather than silently ignored. Honouring its old value
# would put the 10s window back, which is the bug this replaced. Removable once
# nothing sets it: docs/temporary-measures.md § "The retired-settle notice in
# the pre-flight engine reclaim".
_reclaim_warn_retired_settle() {
    [ -n "${LUCIDOS_RECLAIM_SETTLE_S:-}" ] || return 0
    echo "$RECLAIM_TAG NOTE: LUCIDOS_RECLAIM_SETTLE_S is retired and ignored." >&2
    echo "$RECLAIM_TAG   The single delayed re-scan is now a poll. Set LUCIDOS_RECLAIM_QUIET_S or LUCIDOS_RECLAIM_DEADLINE_S instead." >&2
}

# ── host seams (overridden by the test) ─────────────────────────────────
# Every one fails CLOSED when the test replaces it: an empty synthetic feed
# means "no engines", never "fall back to the real host". Same posture as the ps
# seam in webkit_reaper.sh, which put the host process table into a real kill
# once. The clock and the sleep are seams for a different reason: the poll below
# runs for the best part of a minute, and a test must not wait it out.

# Every live engine pid, one per line. `pgrep -x` matches the process NAME
# exactly. Deliberately not `pgrep -f 'target/debug/...'`: nothing launches from
# there any more, and a command-line match reads a mention as a match.
_reclaim_engine_pids() {
    pgrep -x lucidos-engine 2>/dev/null
}

# One process environment, as `ps -E` prints it: argv first, then NAME=value
# pairs, all on one line.
_reclaim_proc_env() {
    ps -E -p "$1" -o command= 2>/dev/null
}

# Seconds since the epoch, and a real pause. The poll's whole clock.
_reclaim_now() { date +%s; }
_reclaim_sleep() { sleep "$1"; }

# Every gateway log on this machine, one path per line, whether or not it
# exists. A comeback is explained from these and from nothing else.
#
# Two gateways run here and they log in different places: the dev one writes
# `gateway.log` under its data dir, and the packaged one has its stdout
# redirected by launchd (`desired_service_plist` in
# crates/lucidos-app/src/desktop.rs).
_reclaim_gateway_logs() {
    printf '%s\n' "${LUCIDOS_GATEWAY_DATA:-$HOME/.lucidos/gateway}/gateway.log"
    printf '%s\n' "$HOME/Library/Application Support/com.lucidos.app/logs/engine-service.out.log"
}

# The stop.sh this reclaim drives, under the repo root. One definition, so the
# precondition check and the call can never disagree on where it is.
_reclaim_stop_script() {
    printf '%s' "$RECLAIM_PROJECT_DIR/scripts/stop.sh"
}

# The one way this script stops anything. No raw signal, ever.
_reclaim_stop_workspace() {
    "$(_reclaim_stop_script)" -w "$1"
}

# ── pure helpers ────────────────────────────────────────────────────────

# Exact keep-list membership. The pattern half is quoted, so a workspace id is
# matched literally and the space padding makes `devbox` fail against `dev`.
_reclaim_is_kept() {
    case " $(_reclaim_keep_list) " in
        *" $1 "*) return 0 ;;
    esac
    return 1
}

_reclaim_gb() {
    case "${1:-}" in
        '') printf 'unknown' ;;
        *) printf '%s GB' "$1" ;;
    esac
}

# The signed change in available memory. Prints nothing when either reading is
# missing, because a guard that could not measure must not report a number.
_reclaim_delta() {
    [ -n "${1:-}" ] && [ -n "${2:-}" ] || return 0
    awk -v a="$1" -v b="$2" 'BEGIN { printf " (delta %+.2f GB)", b - a }'
}

# ── classification ──────────────────────────────────────────────────────

# One tab separated "pid class id path" line per live engine. The class is
# `keep`, `reclaim`, or `unknown`.
#
# `unknown` is a pid whose workspace id could not be read. It is left alone and
# never counted as a survivor: we cannot call it reclaimable without being able
# to identify it, and killing what you cannot name is how a reclaim takes out
# the wrong engine.
#
# An unreadable id is written as `?`, so no field before the last one can be
# empty. A tab is IFS whitespace, so `read` collapses a run of them: an empty
# middle field would shift the path one column left, into the id. A slug can
# never be `?`, because the gateway maps every non-alphanumeric to a dash.
_reclaim_scan() {
    local pids pid procenv id ws class
    pids="$(_reclaim_engine_pids)"
    [ -n "$pids" ] || return 0
    printf '%s\n' "$pids" | while read -r pid; do
        case "$pid" in
            '' | *[!0-9]*) continue ;;
        esac
        procenv="$(_reclaim_proc_env "$pid")"
        id="$(proc_env_value "$procenv" LUCIDOS_WORKSPACE_ID)"
        ws="$(proc_env_value "$procenv" LUCIDOS_WORKSPACE)"
        if [ -z "$id" ]; then
            class="unknown"
            id="?"
            ws=""
        elif _reclaim_is_kept "$id"; then
            class="keep"
        else
            class="reclaim"
        fi
        printf '%s\t%s\t%s\t%s\n' "$pid" "$class" "$id" "$ws"
    done
}

# "3 (dev:79603 coldrun:44011 ?:60001)", or "0" on an empty host. An
# unidentified engine shows as "?:<pid>", so the count never hides one.
_reclaim_summarize() {
    local scan="$1" pid class id ws n=0 names=""
    if [ -n "$scan" ]; then
        while IFS="$(printf '\t')" read -r pid class id ws; do
            [ -n "$pid" ] || continue
            n=$((n + 1))
            names="$names $id:$pid"
        done <<EOF
$scan
EOF
    fi
    if [ "$n" -eq 0 ]; then
        printf '0'
    else
        printf '%d (%s)' "$n" "${names# }"
    fi
}

# The pid the scan $1 shows for workspace $2, or nothing when it is not up.
_reclaim_pid_of() {
    printf '%s\n' "$1" | awk -F'\t' -v want="$2" '$3 == want { print $1; exit }'
}

# ── the comeback log ────────────────────────────────────────────────────
#
# A workspace that returns during the watch has to be explained, because the two
# paths that bring one back need different answers from the reader.
#
#   respawning '<ws>' after N missed probe(s)   the gateway supervisor still
#                                               held the stack: the stop never
#                                               reached the owning gateway
#   lazy-starting '<ws>' on demand              the stop DID land, and a client
#                                               window navigated to it again
#
# Each log is measured before the first stop and read only from that offset, so
# yesterday's respawn cannot be quoted as today's cause.

_reclaim_log_size() { wc -c <"$1" 2>/dev/null | tr -d ' '; }

# "<bytes>\t<path>" per gateway log, sampled now. Call before the first stop.
_reclaim_mark_gateway_logs() {
    local path size
    while IFS= read -r path; do
        [ -n "$path" ] || continue
        size="$(_reclaim_log_size "$path")"
        printf '%s\t%s\n' "${size:-0}" "$path"
    done <<EOF
$(_reclaim_gateway_logs)
EOF
}

# The newest gateway log line naming a comeback for workspace $1, or nothing.
#
# Only bytes appended since the mark are read, so an older episode's respawn is
# never quoted as this one's cause. A log that SHRANK was rotated, and then the
# whole of the new file is what arrived since.
_reclaim_comeback_reason() {
    local id="$1" size path now appended hit
    while IFS="$(printf '\t')" read -r size path; do
        [ -n "$path" ] && [ -r "$path" ] || continue
        now="$(_reclaim_log_size "$path")"
        [ -n "$now" ] || continue
        if [ "$now" -lt "$size" ]; then
            appended="$now"
        else
            appended=$((now - size))
        fi
        [ "$appended" -gt 0 ] || continue
        hit="$(tail -c "$appended" "$path" 2>/dev/null |
            grep -F -e "respawning '$id'" -e "lazy-starting '$id'" |
            tail -n 1)"
        if [ -n "$hit" ]; then
            printf '%s' "$hit"
            return 0
        fi
    done <<EOF
$_RECLAIM_LOG_MARKS
EOF
}

# Which of the two paths the log line $1 describes.
_reclaim_comeback_path() {
    case "$1" in
        *respawning*) printf 'gateway supervisor respawn' ;;
        *lazy-starting*) printf 'client lazy-start' ;;
        *) printf 'unrecognized' ;;
    esac
}

# Report every workspace in the "id<TAB>pid" list $1 as back after $2 seconds.
_reclaim_report_comeback() {
    local list="$1" elapsed="$2" id pid reason
    while IFS="$(printf '\t')" read -r id pid; do
        [ -n "$id" ] || continue
        echo "$RECLAIM_TAG WARNING: $id came back ${elapsed}s after stop.sh, as pid $pid." >&2
        reason="$(_reclaim_comeback_reason "$id")"
        if [ -n "$reason" ]; then
            echo "$RECLAIM_TAG   $(_reclaim_comeback_path "$reason"), from the gateway log:" >&2
            echo "$RECLAIM_TAG     $reason" >&2
        else
            echo "$RECLAIM_TAG   no gateway log line names it, so which path brought it back is unknown." >&2
        fi
    done <<EOF
$list
EOF
}

# ── the watch ───────────────────────────────────────────────────────────

# Watch every workspace this run stopped until the host is provably settled.
#
# Returns 0 only after all of them have been continuously absent for the quiet
# window. Returns 1 the moment one is back under a NEW pid, and at the deadline
# if the host never went quiet. Same pid means the engine is still draining,
# which is not a comeback and not settled either.
_reclaim_watch() {
    local quiet deadline poll started now scan back still_up absent_since=""
    local id oldpid ws nowpid
    quiet="$(_reclaim_quiet_s)"
    deadline="$(_reclaim_deadline_s)"
    poll="$(_reclaim_poll_s)"
    started="$(_reclaim_now)"
    echo "$RECLAIM_TAG watching for ${quiet}s of quiet, up to ${deadline}s, every ${poll}s."
    while :; do
        scan="$(_reclaim_scan)"
        back=""
        still_up=""
        while IFS="$(printf '\t')" read -r id oldpid ws; do
            [ -n "$id" ] || continue
            nowpid="$(_reclaim_pid_of "$scan" "$id")"
            [ -n "$nowpid" ] || continue
            if [ "$nowpid" = "$oldpid" ]; then
                still_up=1
            else
                back="$back$(printf '%s\t%s' "$id" "$nowpid")
"
            fi
        done <<EOF
$_RECLAIM_STOPPED
EOF
        now="$(_reclaim_now)"
        if [ -n "$back" ]; then
            _reclaim_report_comeback "$back" "$((now - started))"
            return 1
        fi
        if [ -n "$still_up" ]; then
            absent_since=""
        else
            [ -n "$absent_since" ] || absent_since="$now"
            if [ "$((now - absent_since))" -ge "$quiet" ]; then
                echo "$RECLAIM_TAG quiet for $((now - absent_since))s: nothing this run stopped came back."
                return 0
            fi
        fi
        if [ "$((now - started))" -ge "$deadline" ]; then
            echo "$RECLAIM_TAG WARNING: gave up after $((now - started))s without ${quiet}s of quiet." >&2
            echo "$RECLAIM_TAG   Raise LUCIDOS_RECLAIM_DEADLINE_S if an engine here genuinely drains that slowly." >&2
            return 1
        fi
        _reclaim_sleep "$poll"
    done
}

# ── the step ────────────────────────────────────────────────────────────
# Exit 0 when every reclaimable engine is down afterwards, 1 otherwise.
preflight_reclaim_main() {
    local avail_before avail_after scan_before scan_after tool stop_script
    local pid class id ws
    local stopped=0 failed=0

    # Every workspace this run stopped, as "id<TAB>pid<TAB>path". The watch
    # compares each pid against the host afterwards, so a fresh pid under the
    # same id is a comeback rather than an engine that never left.
    _RECLAIM_STOPPED=""
    # Where each gateway log stood before the first stop, so a comeback is
    # explained from what was written after it and never from an older line.
    _RECLAIM_LOG_MARKS=""

    _reclaim_warn_retired_settle

    for tool in pgrep ps; do
        if ! command -v "$tool" >/dev/null 2>&1; then
            echo "$RECLAIM_TAG ERROR: no $tool on PATH, so no engine can be found." >&2
            return 1
        fi
    done

    # Prove the stop mechanism exists before the loop reaches for it. A wrong
    # RECLAIM_PROJECT_DIR is caught here, as one clear line, not as a bash
    # "No such file or directory" mid-reclaim that reads like a per-workspace
    # failure. The path must be an executable file, so a directory or an
    # unreadable stub is a failure too.
    stop_script="$(_reclaim_stop_script)"
    if [ ! -x "$stop_script" ] || [ ! -f "$stop_script" ]; then
        echo "$RECLAIM_TAG ERROR: no executable stop.sh at $stop_script. RECLAIM_PROJECT_DIR must be the repo root, holding scripts/stop.sh." >&2
        return 1
    fi

    avail_before="$(_host_mem_read_available_gb)"
    scan_before="$(_reclaim_scan)"

    echo "$RECLAIM_TAG engines up before: $(_reclaim_summarize "$scan_before")"
    echo "$RECLAIM_TAG available before: $(_reclaim_gb "$avail_before")"
    echo "$RECLAIM_TAG keep list: $(_reclaim_keep_list)"

    if [ -n "$scan_before" ]; then
        _RECLAIM_LOG_MARKS="$(_reclaim_mark_gateway_logs)"
        while IFS="$(printf '\t')" read -r pid class id ws; do
            [ -n "$pid" ] || continue
            [ "$class" = "keep" ] && continue
            if [ "$class" = "unknown" ]; then
                echo "$RECLAIM_TAG skip pid $pid: no LUCIDOS_WORKSPACE_ID to identify it by."
                continue
            fi
            # No fallback to a constructed path. Guessing one is what made the
            # old snippet exit 1 on "Workspace not found" and read as clean.
            if [ -z "$ws" ]; then
                echo "$RECLAIM_TAG WARNING: $id (pid $pid) has no LUCIDOS_WORKSPACE, so stop.sh cannot be aimed at it." >&2
                failed=1
                continue
            fi
            echo "$RECLAIM_TAG stop $id (pid $pid): $ws"
            if _reclaim_stop_workspace "$ws"; then
                stopped=$((stopped + 1))
                _RECLAIM_STOPPED="$_RECLAIM_STOPPED$(printf '%s\t%s\t%s' "$id" "$pid" "$ws")
"
            else
                echo "$RECLAIM_TAG WARNING: stop.sh failed for $id (pid $pid) at $ws." >&2
                failed=1
            fi
        done <<EOF
$scan_before
EOF
    fi

    if [ "$stopped" -gt 0 ]; then
        _reclaim_watch || failed=1
    fi

    avail_after="$(_host_mem_read_available_gb)"
    scan_after="$(_reclaim_scan)"

    echo "$RECLAIM_TAG engines up after: $(_reclaim_summarize "$scan_after")"
    echo "$RECLAIM_TAG available after: $(_reclaim_gb "$avail_after")$(_reclaim_delta "$avail_before" "$avail_after")"

    if [ -n "$scan_after" ]; then
        while IFS="$(printf '\t')" read -r pid class id ws; do
            [ "${class:-}" = "reclaim" ] || continue
            echo "$RECLAIM_TAG WARNING: $id (pid $pid) is up at the end of the watch, at $ws." >&2
            failed=1
        done <<EOF
$scan_after
EOF
    fi

    if [ "$failed" -ne 0 ]; then
        echo "$RECLAIM_TAG VERDICT: FAILED. The host did not give back what this step claims to reclaim." >&2
        return 1
    fi
    return 0
}
