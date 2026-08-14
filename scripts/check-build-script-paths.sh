#!/usr/bin/env bash
#
# check-build-script-paths.sh: fail if a cargo build script BAKES a checkout
# path with compile-time `env!`, instead of reading it at run time.
#
#   ./scripts/check-build-script-paths.sh            # gate
#   ./scripts/check-build-script-paths.sh --report   # list the build scripts, never fail
#
# Run by `/harden` Phase 4.5 for every diff. WHOLE-TREE, not diff-scoped,
# matching check-adrs.sh and check-context-budget.sh: what the tree does today
# does not depend on which branch introduced it, so a merge that reintroduces
# the form is caught by the next branch to run the gate.
#
# Two of the three failures this prevents are SILENT (a frozen gateway build id,
# an app stamped 0000.00.00.0), so no test would catch a reintroduction. That is
# why a deterministic gate exists rather than a convention.
#
# The banned form, the reason, and the scoping all live in
# scripts/lib/build_script_path_scan.sh. Read that header before widening this.
#
# Exit status: 0 clean, 1 a build script bakes a path OR the scan could not run.
# A gate that cannot run must never read as clean.

set -uo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
# shellcheck source=scripts/lib/build_script_path_scan.sh
source "$SCRIPT_DIR/lib/build_script_path_scan.sh"

REPORT_ONLY=0
while [ $# -gt 0 ]; do
    case "$1" in
        --report)
            REPORT_ONLY=1
            shift
            ;;
        -h | --help)
            # The header block, stopping at the first non-comment line, so the
            # help text cannot drift the way a fixed line range does. Same
            # convention as scripts/check-prompt-mirror.sh.
            awk 'NR == 1 { next } /^#/ { sub(/^# ?/, ""); print; next } { exit }' "${BASH_SOURCE[0]}"
            exit 0
            ;;
        *)
            echo "ERROR: unknown argument: $1" >&2
            exit 1
            ;;
    esac
done

REPO_ROOT="$(git rev-parse --show-toplevel 2>/dev/null)"
if [ -z "$REPO_ROOT" ]; then
    echo "ERROR: not inside a git checkout, so the build-script path gate cannot run." >&2
    exit 1
fi

if [ "$REPORT_ONLY" -eq 1 ]; then
    echo "Cargo build scripts under this gate:"
    build_script_path_files "$REPO_ROOT" | sed 's/^/  /'
    exit 0
fi

if HITS="$(build_script_path_scan "$REPO_ROOT")"; then
    SCAN_RC=0
else
    SCAN_RC=$?
fi
if [ "$SCAN_RC" -ne 0 ]; then
    echo "ERROR: the build-script path scan failed (status $SCAN_RC), so nothing was verified." >&2
    exit 1
fi

if [ -n "$HITS" ]; then
    {
        echo
        echo "✗ BLOCKED: a build script bakes its checkout path at compile time."
        echo
        printf '%s\n' "$HITS" | sed 's/^/  /'
        echo
        echo "  A build-script binary is reused across checkouts that share a"
        echo "  CARGO_TARGET_DIR, so a baked path can name a tree that is gone."
        echo
        echo "  $BUILD_SCRIPT_PATH_ADVICE"
    } >&2
    exit 1
fi

COUNT="$(build_script_path_files "$REPO_ROOT" | grep -c .)"
echo "✓ no build script bakes a checkout path ($COUNT checked)"
exit 0
