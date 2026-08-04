#!/usr/bin/env bash
#
# check-adrs.sh: fail if the ADR directory and its index have drifted apart.
#
#   ./scripts/check-adrs.sh          # verify
#   ./scripts/check-adrs.sh --fix    # restore index order, then verify
#
# Run by `/harden` Phase 4.5 for EVERY diff, not only ones touching docs/adr/.
# It is a whole-tree consistency check costing milliseconds, and running it
# unconditionally is what catches a duplicate number that arrived through a
# MERGE rather than through this branch's own edits. Catching a collision at
# any hardening before Apply is the entire point.
#
# It exists because docs/adr/index.md carries `merge=union` (see
# .gitattributes), which is what stops two branches appending an ADR line from
# conflicting. Union keeps both lines but neither orders nor deduplicates them,
# so this is the half that covers what union does not:
#
#   - two ADR files claiming the same number (silent: git merges them clean,
#     because the filenames differ)
#   - an ADR file with no index line, or an index line with no file
#   - the index out of numeric order (the ordinary result of a union merge)
#   - an ADR missing a required section, or whose heading number disagrees
#     with its filename
#
# A duplicate number is REPORTED, never auto-fixed. Renumbering means renaming
# a file and sweeping its references, and deciding which references are live
# and which are historical narration needs judgment. `--fix` only reorders.
#
# Exit status: 0 clean, 1 problems found OR the check could not run. A gate
# that cannot run must never read as clean.
#
# Targets bash 3.2, the macOS system shell: no `mapfile`, no associative
# arrays. The number-set comparisons go through `comm` instead.

set -uo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
# shellcheck source=scripts/lib/adr_scan.sh
source "$SCRIPT_DIR/lib/adr_scan.sh"

FIX=0
while [ $# -gt 0 ]; do
    case "$1" in
        --fix)
            FIX=1
            shift
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

REPO_ROOT="$(git rev-parse --show-toplevel 2> /dev/null)"
if [ -z "$REPO_ROOT" ]; then
    echo "ERROR: not inside a git checkout, so the ADR check cannot run." >&2
    exit 1
fi
if [ ! -f "$REPO_ROOT/$ADR_INDEX" ]; then
    echo "ERROR: $ADR_INDEX is missing, so the ADR check cannot run." >&2
    exit 1
fi

WORK="$(mktemp -d)"
trap 'rm -rf "$WORK"' EXIT

PROBLEMS="$WORK/problems"
: > "$PROBLEMS"
problem() { printf '%s\n' "$1" >> "$PROBLEMS"; }

# --- --fix: restore index order -------------------------------------------
#
# Rewrites only the ordering of the entry lines, never their text: the index
# wording is hand-written and deliberately richer than each ADR's own heading,
# so there is nothing to regenerate it from. Header prose above the first entry
# is preserved verbatim. Anything that is neither header nor an entry line is
# refused rather than dropped, because silently discarding a line during a
# conflict resolution is exactly the failure this tooling exists to prevent.
if [ "$FIX" -eq 1 ]; then
    INDEX_PATH="$REPO_ROOT/$ADR_INDEX"
    STRAY="$(awk 'seen && !/^- \[/ && NF { print NR ": " $0 } /^- \[/ { seen = 1 }' "$INDEX_PATH")"
    if [ -n "$STRAY" ]; then
        {
            echo "ERROR: --fix refuses to reorder: $ADR_INDEX has non-entry content"
            echo "       below the first entry, and reordering would strand it."
            printf '%s\n' "$STRAY" | sed 's/^/       /'
        } >&2
        exit 1
    fi
    # -s keeps entries sharing a number in their existing relative order, so a
    # re-run cannot keep swapping them. The key is the 4 digits at offset 3.
    {
        awk '/^- \[/ { exit } { print }' "$INDEX_PATH"
        grep -E '^- \[' "$INDEX_PATH" | LC_ALL=C sort -s -k1.4,1.7n
    } > "$WORK/index.new"
    cat "$WORK/index.new" > "$INDEX_PATH"
fi

# --- collect: NUMBER<TAB>NAME for both sides -------------------------------

adr_files "$REPO_ROOT" | sed -E 's/^(([0-9]{4}).*)$/\2\t\1/' > "$WORK/files"
if [ ! -s "$WORK/files" ]; then
    echo "ERROR: no ADR files found under $ADR_DIR, so the check cannot run." >&2
    exit 1
fi

: > "$WORK/index"
PREVIOUS=""
while IFS= read -r line; do
    [ -n "$line" ] || continue
    number="$(adr_entry_number "$line")"
    target="$(adr_entry_target "$line")"
    if [ "$number" = "$line" ] || [ "$target" = "$line" ] || [ -z "$target" ]; then
        problem "malformed index entry: $line"
        continue
    fi
    printf '%s\t%s\n' "$number" "$target" >> "$WORK/index"
    if [ ! -f "$REPO_ROOT/$ADR_DIR/$target" ]; then
        problem "index entry $number links to $target, which does not exist"
    elif [ "$(printf '%s' "$target" | cut -c1-4)" != "$number" ]; then
        problem "index entry $number links to $target, whose number disagrees"
    fi
    if [ -n "$PREVIOUS" ] && [ "$((10#$number))" -lt "$((10#$PREVIOUS))" ]; then
        problem "index out of order: $number follows $PREVIOUS"
    fi
    PREVIOUS="$number"
done < <(adr_index_entries "$REPO_ROOT")

# --- duplicates, and one side missing from the other -----------------------

cut -f1 "$WORK/files" | LC_ALL=C sort > "$WORK/file-numbers"
cut -f1 "$WORK/index" | LC_ALL=C sort > "$WORK/index-numbers"

while IFS= read -r number; do
    [ -n "$number" ] || continue
    problem "duplicate number $number: $(grep "^$number	" "$WORK/files" | cut -f2 | tr '\n' ' ')"
done < <(LC_ALL=C uniq -d "$WORK/file-numbers")

while IFS= read -r number; do
    [ -n "$number" ] || continue
    problem "duplicate index entry for $number"
done < <(LC_ALL=C uniq -d "$WORK/index-numbers")

while IFS= read -r number; do
    [ -n "$number" ] || continue
    problem "$(grep "^$number	" "$WORK/files" | cut -f2 | tr '\n' ' ')has no line in $ADR_INDEX"
done < <(LC_ALL=C comm -23 <(LC_ALL=C uniq "$WORK/file-numbers") <(LC_ALL=C uniq "$WORK/index-numbers"))

# --- each ADR's own shape --------------------------------------------------

while IFS= read -r file; do
    [ -n "$file" ] || continue
    path="$REPO_ROOT/$ADR_DIR/$file"
    number="$(printf '%s' "$file" | cut -c1-4)"
    case "$(head -1 "$path")" in
        "# $number"*) ;;
        *) problem "$file: first line should be a heading opening with $number" ;;
    esac
    # Three spellings in the tree: "- **Status:** Accepted", "- **Status**:
    # Accepted", and 0029's unbulleted "**Status:** Accepted (date)".
    grep -qE '^(- )?\*\*Status' "$path" || problem "$file: no **Status** line"
    for section in "${ADR_REQUIRED_SECTIONS[@]}"; do
        grep -qxF "$section" "$path" || problem "$file: no '$section' section"
    done
done < <(cut -f2 "$WORK/files")

# --- report ----------------------------------------------------------------

COUNT="$(grep -c '' "$PROBLEMS")"
if [ "$COUNT" -eq 0 ]; then
    echo "✓ $(grep -c '' "$WORK/files") ADRs, index consistent"
    exit 0
fi

{
    echo
    echo "✗ BLOCKED: $COUNT problem(s) in $ADR_DIR."
    echo
    sed 's/^/  /' "$PROBLEMS"
    echo
    if grep -q '^duplicate number' "$PROBLEMS"; then
        NEXT="$(adr_next_number "$REPO_ROOT" 2> /dev/null)"
        echo "Two branches claimed the same number. Renumber the one that has NOT"
        echo "reached main yet: rename its file, update its heading and its index"
        echo "line, and sweep references to the old number on this branch."
        [ -n "$NEXT" ] && echo "The next free number is $NEXT."
        echo "Then use ./scripts/adr-new.sh next time, which cannot collide."
        echo
    fi
    if grep -q 'out of order' "$PROBLEMS"; then
        echo "Out-of-order entries are the ordinary result of a union merge of"
        echo "$ADR_INDEX. Run ./scripts/check-adrs.sh --fix to restore the order."
        echo
    fi
} >&2
exit 1
