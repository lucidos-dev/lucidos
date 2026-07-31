#!/usr/bin/env bash
#
# check-em-dashes.sh: fail if this branch ADDS a U+2014 EM DASH or U+2015
# HORIZONTAL BAR. The deterministic review-time half of
# `.claude/rules/no-em-dashes.md`; the write-time half is the Claude Code hook
# `.claude/hooks/no-em-dashes.sh`, which stops the text before it is ever
# written. This one is what covers Codex (no hooks), a hand edit, and anything
# that reached disk some other way.
#
#   ./scripts/check-em-dashes.sh                 # vs the merge-base with main
#   ./scripts/check-em-dashes.sh --base <ref>    # vs another base
#
# Run by `/harden` Phase 4.5 for EVERY diff, including docs-only ones (prose is
# exactly where em dashes come from, so the docs-only skip must not apply).
#
# DIFF-SCOPED, ADDED LINES ONLY. Roughly 29,000 lines in the tree already carry
# the character and the rule is deliberately not retroactive, so a file that is
# merely touched reports nothing for the dashes it already had. A line this
# branch adds or rewords is a hit: touch a line and you own it.
#
# Exit status: 0 clean, 1 hits found OR the scan could not run. A gate that
# cannot run must never read as clean.

set -uo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
# shellcheck source=scripts/lib/em_dash_scan.sh
source "$SCRIPT_DIR/lib/em_dash_scan.sh"

BASE_REF="main"
while [ $# -gt 0 ]; do
    case "$1" in
        --base)
            [ $# -ge 2 ] || { echo "ERROR: --base needs a ref." >&2; exit 1; }
            BASE_REF="$2"
            shift 2
            ;;
        -h | --help)
            # The header block, stopping at the first non-comment line. A fixed
            # line range drifts the moment the header grows or shrinks, and
            # prints `set -uo pipefail` at the reader.
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
    echo "ERROR: not inside a git checkout, so the em-dash gate cannot run." >&2
    exit 1
fi

# The merge base, not the base ref itself: a branch cut days ago must be judged
# on what IT added, never on what main gained in the meantime.
BASE_COMMIT="$(git -C "$REPO_ROOT" merge-base "$BASE_REF" HEAD 2>/dev/null)"
if [ -z "$BASE_COMMIT" ]; then
    echo "ERROR: no merge base between '$BASE_REF' and HEAD, so the em-dash gate cannot run." >&2
    echo "       Pass an existing ref with --base <ref>." >&2
    exit 1
fi

HITS="$( { em_dash_scan_diff "$BASE_COMMIT" "$REPO_ROOT" && em_dash_scan_untracked "$REPO_ROOT"; } )"
SCAN_RC=$?
if [ "$SCAN_RC" -ne 0 ]; then
    echo "ERROR: the em-dash scan failed (status $SCAN_RC), so nothing was verified." >&2
    exit 1
fi

if [ -z "$HITS" ]; then
    echo "✓ no em dashes added since $(git -C "$REPO_ROOT" rev-parse --short "$BASE_COMMIT")"
    exit 0
fi

COUNT="$(printf '%s\n' "$HITS" | wc -l | tr -d ' ')"
{
    echo
    echo "✗ BLOCKED: this branch adds $COUNT line(s) carrying a banned dash character."
    echo
    printf '%s\n' "$HITS" | sed 's/^/  /'
    echo
    echo "$EM_DASH_ADVICE"
    echo "Banned: U+2014 EM DASH and U+2015 HORIZONTAL BAR. U+2013 EN DASH is fine."
    echo "No file type and no context is exempt, code comments and error strings included."
    echo
    echo "Only lines this branch ADDS or REWORDS are checked. Lines that already"
    echo "carried the character and were not touched are not your problem here."
} >&2
exit 1
