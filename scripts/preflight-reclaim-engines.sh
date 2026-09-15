#!/bin/bash
#
# preflight-reclaim-engines.sh: stop every lucidos-engine that should not be
# running before a nightly build, and prove what that freed.
#
#   ./scripts/preflight-reclaim-engines.sh
#
# Run it BEFORE the memory gate, so the gate reads a host that has already
# given back what it can. It touches no threshold and no guard.
#
# Every engine is enumerated by process name, identified from its own
# environment, and stopped through scripts/stop.sh. Nothing here sends a raw
# signal, and no workspace path is ever constructed from an assumed layout.
#
# After the stops it WATCHES the host, polling until every workspace it stopped
# has stayed gone long enough that neither the gateway supervisor nor a client
# window can still be bringing it back. A workspace that returns inside that
# window fails the step, and the warning names the gateway log line explaining
# which of the two paths returned it.
#
# Environment:
#   LUCIDOS_RECLAIM_KEEP        workspaces never stopped (default "dev personal",
#                               space separated, matched exactly)
#   LUCIDOS_RECLAIM_QUIET_S     seconds of continuous absence that settle it
#                               (default 39, derived from the gateway
#                               supervisor's pacing and the client's boot
#                               watchdog)
#   LUCIDOS_RECLAIM_DEADLINE_S  cap on the whole watch (default 59)
#   LUCIDOS_RECLAIM_POLL_S      seconds between scans (default 2)
#
# LUCIDOS_RECLAIM_SETTLE_S is RETIRED. Its 10s was shorter than a respawn, which
# is the bug the watch replaced, so setting it now prints a note and changes
# nothing.
#
# Exit status: 0 when every reclaimable engine is down afterwards. 1 when one
# survived or came back, when one could not be aimed at, or when stop.sh failed.
# A reclaim that cannot show what it freed must not look like success.
#
# The reasoning behind each rule, and the three bugs they replace, is in
# scripts/lib/preflight_reclaim.sh.

set -uo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"

while [ $# -gt 0 ]; do
    case "$1" in
        -h | --help)
            # The header block, stopping at the first non-comment line. Same
            # convention as scripts/check-prose.sh: a fixed line range drifts
            # the moment the header grows.
            awk 'NR == 1 { next } /^#/ { sub(/^# ?/, ""); print; next } { exit }' "${BASH_SOURCE[0]}"
            exit 0
            ;;
        *)
            echo "ERROR: unknown argument: $1" >&2
            echo "Run '$0 --help' for usage." >&2
            exit 1
            ;;
    esac
done

# shellcheck source=scripts/lib/preflight_reclaim.sh
source "$SCRIPT_DIR/lib/preflight_reclaim.sh"

preflight_reclaim_main
