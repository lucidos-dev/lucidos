#!/usr/bin/env bash
#
# check-context-budget.sh: fail if the always-loaded instruction set has grown
# past its ceiling, or if a rule that should be path-scoped has silently become
# resident. The deterministic half of "keep CLAUDE.md small"; the semantic half
# is a judgment call this script cannot make, so it only measures.
#
#   ./scripts/check-context-budget.sh            # gate
#   ./scripts/check-context-budget.sh --report   # print the set, never fail
#
# Run by `/harden` Phase 4.5 for EVERY diff, including docs-only ones. Docs-only
# is precisely the diff that grows this set, so a docs-only skip would exempt
# the only change that can break it.
#
# WHOLE-TREE, NOT DIFF-SCOPED, which is the opposite of check-em-dashes.sh and
# deliberate. Em dashes are a per-line rule with 29,000 pre-existing violations,
# so scoping to the diff is what makes it enforceable. This is a per-TREE
# property: the question is what a session loads today, and the answer does not
# depend on which branch grew it. It also means a merge that pushes the total
# over is caught by the next branch to run the gate, the same reason
# check-adrs.sh runs unconditionally.
#
# Exit status: 0 clean, 1 over budget or unexpected membership OR the scan could
# not run. A gate that cannot run must never read as clean.

set -uo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
# shellcheck source=scripts/lib/context_budget.sh
source "$SCRIPT_DIR/lib/context_budget.sh"

REPORT_ONLY=0
while [ $# -gt 0 ]; do
    case "$1" in
        --report)
            REPORT_ONLY=1
            shift
            ;;
        -h | --help)
            # The header block, stopping at the first non-comment line, so the
            # help text cannot drift from the header the way a fixed line range
            # does. Same convention as scripts/check-em-dashes.sh.
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
    echo "ERROR: not inside a git checkout, so the context-budget gate cannot run." >&2
    exit 1
fi

SCAN="$(context_budget_scan "$REPO_ROOT")"
SCAN_RC=$?
if [ "$SCAN_RC" -ne 0 ]; then
    echo "ERROR: the context-budget scan failed (status $SCAN_RC), so nothing was verified." >&2
    exit 1
fi
if [ -z "$SCAN" ]; then
    # CLAUDE.md alone guarantees a non-empty result in this repo, so an empty
    # scan means discovery broke, not that the tree is admirably lean.
    echo "ERROR: the context-budget scan found no always-loaded files at all." >&2
    echo "       CLAUDE.md should always be one, so discovery is broken." >&2
    exit 1
fi

TOTAL=0
FOUND=()
while IFS=$'\t' read -r bytes path; do
    TOTAL=$((TOTAL + bytes))
    FOUND+=("$path")
done <<<"$SCAN"

# Membership, computed both ways: a resident file nobody declared, and a
# declared file that is no longer resident (renamed, deleted, or newly scoped).
# The second direction matters as much as the first, because a stale expected
# list is what lets the next real surprise look like a known one.
UNEXPECTED=()
for path in "${FOUND[@]}"; do
    declared=0
    for want in "${CONTEXT_BUDGET_EXPECTED_ALWAYS[@]}"; do
        [ "$path" = "$want" ] && declared=1 && break
    done
    [ "$declared" -eq 0 ] && UNEXPECTED+=("$path")
done

MISSING=()
for want in "${CONTEXT_BUDGET_EXPECTED_ALWAYS[@]}"; do
    present=0
    for path in "${FOUND[@]}"; do
        [ "$path" = "$want" ] && present=1 && break
    done
    [ "$present" -eq 0 ] && MISSING+=("$want")
done

print_set() {
    printf '%s\n' "$SCAN" | while IFS=$'\t' read -r bytes path; do
        printf '  %7s  %s\n' "$bytes" "$path"
    done
    printf '  %7s  TOTAL (ceiling %s, est. ~%sk tokens)\n' \
        "$TOTAL" "$CONTEXT_BUDGET_CEILING" "$((TOTAL / 4000))"
}

if [ "$REPORT_ONLY" -eq 1 ]; then
    echo "Always-loaded instruction set, in bytes:"
    print_set
    exit 0
fi

FAILED=0

if [ ${#UNEXPECTED[@]} -gt 0 ]; then
    {
        echo
        echo "✗ BLOCKED: ${#UNEXPECTED[@]} rule file(s) load in EVERY session but are not declared:"
        printf '    %s\n' "${UNEXPECTED[@]}"
        echo
        echo "  A rule is resident unless its frontmatter carries a usable 'paths:' key."
        echo "  The key is 'paths:', NOT 'globs:'. A 'globs:' key is a Cursor convention"
        echo "  that Claude Code silently ignores, which is how the entire rule set sat in"
        echo "  every session until 2026-07-25. A 'paths:' of exactly '**' scopes nothing."
        echo
        echo "  Either scope the rule, or add it to CONTEXT_BUDGET_EXPECTED_ALWAYS in"
        echo "  scripts/lib/context_budget.sh and say in the commit message why it has to"
        echo "  be in front of the agent before any file is touched."
    } >&2
    FAILED=1
fi

if [ ${#MISSING[@]} -gt 0 ]; then
    {
        echo
        echo "✗ BLOCKED: ${#MISSING[@]} declared always-loaded file(s) are no longer resident:"
        printf '    %s\n' "${MISSING[@]}"
        echo
        echo "  Renamed, deleted, or newly path-scoped. All three are fine on purpose and"
        echo "  none is fine by accident, so update CONTEXT_BUDGET_EXPECTED_ALWAYS in"
        echo "  scripts/lib/context_budget.sh to match."
    } >&2
    FAILED=1
fi

if [ "$TOTAL" -gt "$CONTEXT_BUDGET_CEILING" ]; then
    {
        echo
        echo "✗ BLOCKED: the always-loaded set is $TOTAL bytes, over its $CONTEXT_BUDGET_CEILING ceiling by $((TOTAL - CONTEXT_BUDGET_CEILING))."
        echo
        print_set
        echo
        echo "  Every byte here is paid on every request of every session, before the"
        echo "  agent has read a line of code."
        echo
        echo "  $CONTEXT_BUDGET_ADVICE"
        echo
        echo "  Raising CONTEXT_BUDGET_CEILING is allowed and is a deliberate act: say in"
        echo "  the commit message what became worth paying for on every request."
    } >&2
    FAILED=1
fi

if [ "$FAILED" -ne 0 ]; then
    exit 1
fi

echo "✓ always-loaded context $TOTAL/$CONTEXT_BUDGET_CEILING bytes across ${#FOUND[@]} files"
exit 0
