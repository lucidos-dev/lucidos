#!/usr/bin/env bash
#
# with-run-limit.sh: run a command, and tear down its whole process tree if it
# outlives a limit.
#
#   ./scripts/with-run-limit.sh <seconds> <label> -- <command> [args...]
#
# Exits with the command's own status, or 124 when the limit stopped it. A hung
# test otherwise spins on, holding its build slot until somebody notices
# (ADR 0391). test-engine.sh runs cargo under it, inside the slot, so the time
# spent waiting for a slot does not count.
#
# The tree is walked by parent pid from our own child, never by name
# (ADR 0025). SIGTERM goes to every process in it, then SIGKILL after a grace
# to each one still running with the same start time.
#
# Env:
#   LUCIDOS_RUN_LIMIT_GRACE_SECS   wait between SIGTERM and SIGKILL (default 10)
set -uo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
# shellcheck source=scripts/lib/proc_tree.sh
source "$SCRIPT_DIR/lib/proc_tree.sh"

TIMEOUT_EXIT=124
GRACE_SECS="${LUCIDOS_RUN_LIMIT_GRACE_SECS:-10}"

usage() {
    echo "usage: with-run-limit.sh <seconds> <label> -- <command> [args...]" >&2
    exit 2
}
[ $# -ge 4 ] && [ "$3" = "--" ] || usage
LIMIT="$1"
LABEL="$2"
shift 3
case "$LIMIT" in '' | *[!0-9]*) usage ;; esac

tree_of() { # <pid>
    ps -axo pid=,ppid= 2>/dev/null | proc_tree_walk "$1"
}

# The kernel's start time for a pid, or nothing. A pid plus its start time
# names one process, so a recycled pid is never signalled.
started_at() { # <pid>
    ps -o lstart= -p "$1" 2>/dev/null | sed 's/^ *//; s/ *$//'
}

# One "pid start-time" line per process in the tree.
identities_of() { # <pid>
    local pid
    for pid in $(tree_of "$1"); do
        printf '%s %s\n' "$pid" "$(started_at "$pid")"
    done
}

# A background job in a shell without job control ignores SIGINT, so Ctrl-C
# would no longer reach the command. Forward the stop as SIGTERM instead.
STOPPED=""
# shellcheck disable=SC2329 # invoked by the INT/TERM trap below
forward_stop() {
    STOPPED=1
    # shellcheck disable=SC2046
    kill -TERM $(tree_of "$CHILD") 2>/dev/null
}

EXPIRED="$(mktemp -t lucidos-run-limit.XXXXXX)"
rm -f "$EXPIRED"

"$@" &
CHILD=$!
CHILD_STARTED="$(started_at "$CHILD")"
trap forward_stop INT TERM

(
    sleep "$LIMIT"
    # Orphaned by a wrapper killed on its own, this watchdog outlives the
    # child. Its pid may then name another process, so check before acting.
    [ -n "$CHILD_STARTED" ] && [ "$(started_at "$CHILD")" = "$CHILD_STARTED" ] || exit 0
    # A child that exited right at the limit lingers as a zombie until reaped.
    # It finished, so it did not time out.
    case "$(ps -o stat= -p "$CHILD" 2>/dev/null)" in *Z*) exit 0 ;; esac
    : > "$EXPIRED"
    marked="$(identities_of "$CHILD")"
    # shellcheck disable=SC2046
    kill -TERM $(printf '%s\n' "$marked" | awk '{ print $1 }') 2>/dev/null
    sleep "$GRACE_SECS"
    printf '%s\n' "$marked" | while read -r pid started; do
        [ -n "$pid" ] || continue
        [ "$(started_at "$pid")" = "$started" ] && kill -KILL "$pid" 2>/dev/null
    done
) &
WATCHDOG=$!

wait "$CHILD"
STATUS=$?
# A trapped signal ends the first wait early; the second waits for the stop.
[ -n "$STOPPED" ] && wait "$CHILD" 2>/dev/null

if [ -e "$EXPIRED" ]; then
    # Let the watchdog finish its SIGKILL pass: our child may be gone while a
    # grandchild that ignored SIGTERM is not.
    wait "$WATCHDOG" 2>/dev/null
    rm -f "$EXPIRED"
    echo "with-run-limit: $LABEL ran past its ${LIMIT}s limit and was stopped (exit $TIMEOUT_EXIT)." >&2
    echo "with-run-limit: a 'has been running for over 60 seconds' line above names a hung test." >&2
    exit "$TIMEOUT_EXIT"
fi

# shellcheck disable=SC2046
kill $(tree_of "$WATCHDOG") 2>/dev/null
wait "$WATCHDOG" 2>/dev/null
exit "$STATUS"
