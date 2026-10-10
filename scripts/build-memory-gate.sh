#!/usr/bin/env bash
# The build memory gate: refuse to start a heavy build on a host in memory
# trouble, and wait for it to recover first (ADR 0351).
#
# Usage:
#   scripts/build-memory-gate.sh [--label "<build>"]   # wait, then 0 GO or 72 refused
#   scripts/build-memory-gate.sh --once                # one reading: 0 GO, 72 NO-GO
#
# The rule is the pre-flight gate's, applied by host_memory_gate_verdict in
# scripts/lib/host_memory_guard.sh. Callers: scripts/with-build-slot.sh, before
# it asks for a slot, and scripts/harden-suites.sh, before each cargo suite.
#
# A pass holds for 60 s in the process tree. A caller that just passed exports
# LUCIDOS_BUILD_MEMORY_GATE_PASSED_AT, so a nested build skips the readings.
#
# Exit status: 0 GO, 72 refused. A host it cannot read is GO.

set -uo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
# shellcheck source=scripts/lib/host_memory_guard.sh
source "$SCRIPT_DIR/lib/host_memory_guard.sh"

PASS_HOLDS_SECS=60

label="a heavy build"
once=""
while [ $# -gt 0 ]; do
    case "$1" in
        --label)
            [ $# -ge 2 ] || { echo "build-memory-gate.sh: --label needs a value" >&2; exit 2; }
            label="$2"
            shift 2
            ;;
        --once) once=1; shift ;;
        *) echo "build-memory-gate.sh: unknown argument '$1'" >&2; exit 2 ;;
    esac
done

passed_at="${LUCIDOS_BUILD_MEMORY_GATE_PASSED_AT:-}"
case "$passed_at" in
    '' | *[!0-9]*) ;;
    *) [ $(($(_host_mem_now) - passed_at)) -lt "$PASS_HOLDS_SECS" ] && exit 0 ;;
esac

if [ -n "$once" ]; then
    if reasons="$(host_memory_gate_verdict "$HOST_MEMORY_FREE_FLOOR_ABS_GB")"; then
        echo "[build-memory-gate] GO"
        exit 0
    fi
    echo "[build-memory-gate] NO-GO"
    printf '%s\n' "$reasons" | sed 's/^/[build-memory-gate]   - /'
    exit "$HOST_MEMORY_BUILD_REFUSED_EXIT"
fi

host_memory_build_gate "$label"
