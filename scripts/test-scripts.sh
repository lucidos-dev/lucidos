#!/bin/bash
# test-scripts.sh: run every scripts/lib/*_test.sh, report per-suite pass or
# fail and a total, and exit non-zero if any suite fails.
#
#   ./scripts/test-scripts.sh          (= make test-scripts)
#
# These are the unit tests for the shell libraries under scripts/lib/. Each runs
# directly, with its own tiny harness, and exits non-zero on failure. Nothing
# else collected them, so a suite could rot unrun for months; this target runs
# them together. It is separate from `make lint` and from the engine suite.
#
# Discovery is `git ls-files`, so a suite added in any commit is covered the day
# it lands. That is the same reason lint-shell.sh discovers that way rather than
# from a hand-maintained list, which silently fails to cover a new file.
#
# Each suite MUST be hermetic. The libraries it exercises find and stop real
# engines, so a suite that reached the real host would stop live workspaces
# (ADR 0025). This runner adds no isolation of its own and relies on that.

set -uo pipefail

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$REPO_ROOT" || exit 1

# git ls-files first, so a new suite is covered the day it is committed. Fall
# back to a glob only when git cannot answer, so a tarball checkout still runs.
suites="$(git ls-files 'scripts/lib/*_test.sh' 2>/dev/null)"
if [ -z "$suites" ]; then
    suites="$(ls scripts/lib/*_test.sh 2>/dev/null)"
fi
if [ -z "$suites" ]; then
    echo "ERROR: no scripts/lib/*_test.sh suites found." >&2
    exit 1
fi

LOGDIR="$(mktemp -d -t lucidos-test-scripts.XXXXXX)"
cleanup() { rm -rf "$LOGDIR"; }
trap cleanup EXIT

total=0
passed=0
failed=0
failed_suites=""

while IFS= read -r suite; do
    [ -n "$suite" ] || continue
    total=$((total + 1))
    log="$LOGDIR/$(printf '%s' "$suite" | tr '/' '_').log"
    if bash "$suite" >"$log" 2>&1; then
        passed=$((passed + 1))
        echo "PASS  $suite"
    else
        failed=$((failed + 1))
        failed_suites="$failed_suites $suite"
        echo "FAIL  $suite"
        sed 's/^/        /' "$log"
    fi
done <<EOF
$suites
EOF

echo
echo "scripts/lib suites: $total total, $passed passed, $failed failed"
if [ "$failed" -ne 0 ]; then
    echo "failed:${failed_suites}"
    exit 1
fi
