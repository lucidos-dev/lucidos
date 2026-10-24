#!/usr/bin/env bash
#
# harden-gates.sh: run `/harden`'s cheap gates, the checks every diff runs.
#
#   ./scripts/harden-gates.sh
#
# Each gate takes seconds, so `/harden` runs them before it starts the test
# suites and again at Phase 4.5. A failure caught first costs no suite run
# (ADR 0536). harden.md Phase 4.5 says what each gate guards.
#
# The gates run side by side, and only read the tree. Prints one line per gate,
# PASS or FAIL with its log, so one pass shows every failure. Logs:
# .lucidos/harden-gates/ in the checkout (gitignored).
#
# Exit status: 0 every gate passed, 1 any gate failed.

set -uo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
ROOT="$(dirname "$SCRIPT_DIR")"
LOGS="$ROOT/.lucidos/harden-gates"

# One "<name> <command>" line per gate, in harden.md's order.
GATES='em-dash ./scripts/check-em-dashes.sh
prose ./scripts/check-prose.sh
adr ./scripts/check-adrs.sh
context-budget ./scripts/check-context-budget.sh
prompt-mirror ./scripts/check-prompt-mirror.sh
build-script-paths ./scripts/check-build-script-paths.sh
e2e-workflow ./scripts/check-e2e-workflow.sh
mirror-push-gate ./scripts/mirror-push-gate.sh
mirror-pushers ./scripts/check-mirror-pushers.sh
knowhow-refs ./scripts/check-knowhow-refs.sh
default-flows ./scripts/check-default-flows.sh
audit-scans ./scripts/check-audit-scans.sh
retired-terms ./scripts/lib/check_retired_terms_test.sh && ./scripts/check-retired-terms.sh
hooks-registered ./scripts/lib/hooks_registered_test.sh'

case "${1:-}" in
    "") ;;
    -h | --help)
        awk 'NR == 1 { next } /^#/ { sub(/^# ?/, ""); print; next } { exit }' "${BASH_SOURCE[0]}"
        exit 0
        ;;
    *)
        echo "harden-gates: takes no arguments." >&2
        exit 1
        ;;
esac

rm -rf "$LOGS"
mkdir -p "$LOGS"
while read -r name cmd; do
    ( (cd "$ROOT" && bash -c "$cmd") > "$LOGS/$name.log" 2>&1; echo $? > "$LOGS/$name.exit" ) &
done <<EOF
$GATES
EOF
wait

failed=0
while read -r name _; do
    if [ "$(cat "$LOGS/$name.exit" 2>/dev/null)" = 0 ]; then
        echo "PASS $name"
    else
        echo "FAIL $name, see $LOGS/$name.log"
        failed=1
    fi
done <<EOF
$GATES
EOF
exit "$failed"
