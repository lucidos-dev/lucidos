#!/usr/bin/env bash
#
# check-comments.sh: fail if this branch ADDS a comment to a stripped language.
# The commissioned comment strip removed every comment that no tool or test
# reads. This gate keeps them from growing back.
#
#   ./scripts/check-comments.sh                 # vs the merge-base with main
#   ./scripts/check-comments.sh --base <ref>    # vs another base
#
# Run by `/harden` Phase 4.5 for every diff. It reads each changed file in a
# stripped language and asks the stripper what it would remove. What stays is
# defined once, in scripts/comment-strip/kept-classes.tsv. Which languages are
# stripped is defined once, as STRIPPED_LANGUAGES in strip_tree.py.
#
# Exit status: 0 clean, 1 a comment to remove OR the check could not run.

set -uo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"

BASE_REF="main"
while [ $# -gt 0 ]; do
    case "$1" in
        --base)
            [ $# -ge 2 ] || {
                echo "ERROR: --base needs a ref." >&2
                exit 1
            }
            BASE_REF="$2"
            shift 2
            ;;
        -h | --help)
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
    echo "ERROR: not inside a git checkout, so the comment gate cannot run." >&2
    exit 1
fi

BASE_COMMIT="$(git -C "$REPO_ROOT" merge-base "$BASE_REF" HEAD 2>/dev/null)"
if [ -z "$BASE_COMMIT" ]; then
    echo "ERROR: no merge base between '$BASE_REF' and HEAD, so the comment gate cannot run." >&2
    exit 1
fi

OUT="$(python3 "$SCRIPT_DIR/comment-strip/strip_tree.py" --root "$REPO_ROOT" regrowth --base "$BASE_COMMIT" 2>&1)"
RC=$?
if [ "$RC" -eq 0 ]; then
    echo "✓ no comment added to a stripped language: $(printf '%s\n' "$OUT" | tail -1)"
    exit 0
fi
{
    echo
    echo "✗ BLOCKED: this branch adds a comment the strip removes."
    echo
    printf '%s\n' "$OUT" | sed 's/^/  /'
    echo
    echo "Say it in code: a name, a type or a test. A reason belongs in the commit"
    echo "message or an ADR. The classes that may stay are in"
    echo "scripts/comment-strip/kept-classes.tsv."
    echo "To remove them: python3 scripts/comment-strip/strip_tree.py run --write <path>"
} >&2
exit 1
