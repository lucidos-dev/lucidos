#!/usr/bin/env bash
#
# harden-gates.sh: run `/harden`'s cheap gates, the checks every diff runs.
#
#   ./scripts/harden-gates.sh
#   ./scripts/harden-gates.sh round-start <iteration> <full|fast>
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
comments ./scripts/check-comments.sh
crate-tiers ./scripts/check-crate-tiers.sh
crate-tiers-fixtures ./scripts/lib/check_crate_tiers_test.sh
crate-size ./scripts/check-crate-size.sh
crate-size-fixtures ./scripts/lib/check_crate_size_test.sh
crate-graph ./scripts/check-crate-graph.sh
crate-graph-fixtures ./scripts/lib/check_crate_graph_test.sh
codegen-inputs ./scripts/check-codegen-inputs.sh
adr ./scripts/check-adrs.sh
context-budget ./scripts/check-context-budget.sh
prompt-mirror ./scripts/check-prompt-mirror.sh
build-script-paths ./scripts/check-build-script-paths.sh
e2e-workflow ./scripts/check-e2e-workflow.sh
mirror-push-gate ./scripts/mirror-push-gate.sh
mirror-pushers ./scripts/check-mirror-pushers.sh
knowhow-refs ./scripts/check-knowhow-refs.sh
retired-terms ./scripts/check-retired-terms.sh
audit-scans ./scripts/check-audit-scans.sh
knowhow-coupling ./scripts/check-knowhow-coupling.sh
knowhow-coupling-fixtures ./scripts/lib/check_knowhow_coupling_test.sh
audit-fixtures ./scripts/lib/check_audit_scans_test.sh
knowhow-refs-fixtures ./scripts/lib/check_knowhow_refs_test.sh
knowhow-generated-fixtures ./scripts/lib/check_knowhow_generated_test.sh
default-flows ./scripts/check-default-flows.sh
workspace-client-contract ./scripts/check-workspace-client-contract.sh
workspace-client-contract-fixtures ./scripts/lib/check_workspace_client_contract_test.sh
retired-terms-fixtures ./scripts/lib/check_retired_terms_test.sh
hooks-registered ./scripts/lib/hooks_registered_test.sh
stage-rules node --test scripts/scoreboard/*.test.mjs
comment-strip ./scripts/comment-strip/run-tests.sh
eval-comments-harness ./scripts/eval-comments/run-tests.sh'

record_round_start() {
    local iteration="${1:-}" tier="${2:-}" branch head payload
    if [[ ! "$iteration" =~ ^[1-9][0-9]*$ ]] || [[ "$tier" != full && "$tier" != fast ]]; then
        echo "harden-gates: round-start needs an iteration and tier (full or fast)." >&2
        return 1
    fi
    [ -n "${LUCIDOS_THREAD_ID:-}" ] || return 0
    branch="$(git -C "$ROOT" branch --show-current)" || return 0
    head="$(git -C "$ROOT" rev-parse HEAD)" || return 0
    payload="$(python3 - "$LUCIDOS_THREAD_ID" "$branch" "$head" "$iteration" "$tier" <<'PYTHON'
import json
import sys

thread_id, branch, head, iteration, tier = sys.argv[1:]
print(json.dumps(dict(thread_id=thread_id, branch=branch or None, head_sha=head,
                      iteration=int(iteration), tier=tier)))
PYTHON
    )" || return 0
    lucidos events emit HardeningStarted --summary "harden iteration $iteration started" \
        --payload "$payload" >/dev/null ||
        echo "harden-gates: could not record round start." >&2
}

case "${1:-}" in
    round-start)
        shift
        record_round_start "$@"
        exit $?
        ;;
    "") ;;
    -h | --help)
        awk 'NR == 1 { next } /^#/ { sub(/^# ?/, ""); print; next } { exit }' "${BASH_SOURCE[0]}"
        exit 0
        ;;
    *)
        echo "harden-gates: use round-start <iteration> <full|fast> or run with no arguments." >&2
        exit 1
        ;;
esac

rm -rf "$LOGS"
mkdir -p "$LOGS"
# Conditional checks use the branch diff, including working-tree edits
# during the final gate pass. Discovery failure is its own collected failure.
if base=$(git -C "$ROOT" merge-base main HEAD 2> "$LOGS/change-discovery-input.log") \
    && changed=$(git -C "$ROOT" diff --no-renames --name-only "$base" -- 2>> "$LOGS/change-discovery-input.log") \
    && untracked=$(git -C "$ROOT" ls-files -o --exclude-standard 2>> "$LOGS/change-discovery-input.log"); then
    if printf '%s\n' "$changed" "$untracked" | grep -E '^scripts/recover-billing[^/]*\.py$' > /dev/null; then
        GATES="$GATES
recover-billing python3 scripts/recover-billing_test.py"
    fi
    if printf '%s\n' "$changed" "$untracked" | grep -E '^scripts/main-check-shadow/' > /dev/null; then
        GATES="$GATES
main-check-shadow env PYTHONDONTWRITEBYTECODE=1 python3 -m unittest discover -s scripts/main-check-shadow"
    fi
else
    GATES="$GATES
change-discovery cat .lucidos/harden-gates/change-discovery-input.log; echo 'ERROR: cannot discover changed paths' >&2; false"
fi
while read -r name cmd; do
    # Installing the mirror hook mutates the file inspected by mirror-pushers.
    # Retain both results even when installation fails.
    if [ "$name" = mirror-pushers ]; then
        wait "$mirror_install_pid"
    fi
    ( (cd "$ROOT" && bash -c "$cmd") > "$LOGS/$name.log" 2>&1; echo $? > "$LOGS/$name.exit" ) &
    if [ "$name" = mirror-push-gate ]; then
        mirror_install_pid=$!
    fi
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
