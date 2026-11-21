#!/usr/bin/env bash
#
# check-prompt-mirror.sh: fail if the one rule deliberately stated on BOTH
# instruction surfaces has lost either half. The deterministic guard on the
# single sanctioned exception to "one rule, one surface".
#
#   ./scripts/check-prompt-mirror.sh            # gate
#   ./scripts/check-prompt-mirror.sh --report   # print the verdicts, never fail
#
# Run by `/harden` Phase 4.5 for EVERY diff, including docs-only ones. A
# docs-only diff is precisely how the CLAUDE.md half disappears, and no
# `cargo test` runs for it.
#
# WHOLE-TREE, NOT DIFF-SCOPED, matching check-context-budget.sh and for the same
# reason: the question is what a session is told today, and the answer does not
# depend on which branch removed it. A merge that drops one half is then caught
# by the next branch to run the gate.
#
# The needle, the file list, and why exactly one rule qualifies all live in
# scripts/lib/prompt_mirror_scan.sh. Read that header before adding a second
# mirror; `docs/agent-config.md` has the reasoning in prose.
#
# Exit status: 0 clean, 1 a half is missing OR the scan could not run. A gate
# that cannot run must never read as clean.

set -uo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
# shellcheck source=scripts/lib/prompt_mirror_scan.sh
source "$SCRIPT_DIR/lib/prompt_mirror_scan.sh"

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
            # convention as scripts/check-context-budget.sh.
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
    echo "ERROR: not inside a git checkout, so the prompt-mirror gate cannot run." >&2
    exit 1
fi

SCAN="$(prompt_mirror_scan "$REPO_ROOT")"
SCAN_RC=$?
if [ "$SCAN_RC" -ne 0 ]; then
    echo "ERROR: the prompt-mirror scan failed (status $SCAN_RC), so nothing was verified." >&2
    exit 1
fi
if [ -z "$SCAN" ]; then
    # The file list is a non-empty constant, so an empty scan means the scan
    # itself broke rather than that there is nothing to check.
    echo "ERROR: the prompt-mirror scan produced no verdicts at all, so discovery is broken." >&2
    exit 1
fi

if [ "$REPORT_ONLY" -eq 1 ]; then
    echo "Mirrored process-safety prohibition (ADR 0025):"
    printf '%s\n' "$SCAN" | while IFS=$'\t' read -r verdict path detail; do
        printf '  %-7s %s%s\n' "$verdict" "$path" "${detail:+  ($detail)}"
    done
    exit 0
fi

FAILED=0
CHECKED=0

while IFS=$'\t' read -r verdict path detail; do
    CHECKED=$((CHECKED + 1))
    case "$verdict" in
        ok) ;;
        absent)
            {
                echo
                echo "✗ BLOCKED: $path does not exist, so its half of the mirror cannot be checked."
            } >&2
            FAILED=1
            ;;
        tokens)
            {
                echo
                echo "✗ BLOCKED: $path no longer names: $detail"
                echo
                echo "  $PROMPT_MIRROR_ADVICE"
            } >&2
            FAILED=1
            ;;
        phrase)
            {
                echo
                echo "✗ BLOCKED: $path mentions $PROMPT_MIRROR_ANCHOR_TOKEN but no longer forbids it."
                echo
                echo "  The tokens are still there, but no negation sits within"
                echo "  $PROMPT_MIRROR_WINDOW lines of the mention, so the text reads as description"
                echo "  rather than prohibition."
                echo
                echo "  $PROMPT_MIRROR_ADVICE"
            } >&2
            FAILED=1
            ;;
        *)
            echo "ERROR: unrecognised scan verdict '$verdict' for $path." >&2
            FAILED=1
            ;;
    esac
done <<<"$SCAN"

if [ "$FAILED" -ne 0 ]; then
    exit 1
fi

echo "✓ process-safety prohibition present on both surfaces ($CHECKED checked)"
exit 0
