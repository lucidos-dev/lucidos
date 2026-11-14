#!/bin/bash
# The host memory watch: one tick. launchd runs it every 30 seconds once
# scripts/memory-watch-install.sh has installed the agent. The logic and its knobs
# live in scripts/lib/memory_watch.sh.
#
# Usage:
#   ./scripts/memory-watch.sh --once    record any runaway now, kill one past RAM
#
# It needs no Lucidos process to be running, so it still works on a host whose
# engines are starved. The log is ~/.lucidos/memory-watch/memory-watch.log.

set -u

# launchd starts agents with a minimal PATH; top, ps, lsof and sysctl live here.
export PATH="/usr/bin:/bin:/usr/sbin:/sbin:$PATH"

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
# shellcheck source=lib/ports.sh
source "$SCRIPT_DIR/lib/ports.sh"
# shellcheck source=lib/memory_watch.sh
source "$SCRIPT_DIR/lib/memory_watch.sh"

case "${1:-}" in
    --once)
        memory_watch_once
        ;;
    *)
        echo "Usage: $0 --once" >&2
        exit 2
        ;;
esac
