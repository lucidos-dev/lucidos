#!/bin/bash
# PreToolUse hook: refuse to WRITE a U+2014 EM DASH or U+2015 HORIZONTAL BAR.
# The primary gate for `.claude/rules/no-em-dashes.md`, and the only layer that
# stops the text before it exists: prose alone is what already failed, and the
# review-time scanner (scripts/check-em-dashes.sh, /harden Phase 4.5) can only
# catch it after the fact.
#
# Wired into `.claude/settings.json` on three tools:
#   Edit / Write  the text being written into a file.
#   Bash          the message of a `git commit -m`, which is in scope for the
#                 rule but reaches disk through neither Edit nor Write, and is
#                 invisible to a diff-scoped scanner afterwards.
#
# ADDED LINES ONLY. For Edit, a line carried over from old_string is not a hit;
# for Write, neither is a line already in the file on disk. Roughly 29,000 lines
# in this repo already carry the character and the rule is deliberately not
# retroactive, so touching such a file must not be blocked. Rewording a line
# that keeps its dash IS a hit: touch a line and you own it.
#
# Blocks with exit 2 and an actionable stderr message, matching its neighbours
# pre-push.sh and pre-kill.sh. The message names the file, the line and the
# offending text, so the fix needs no further questions.
#
# FAILS OPEN on anything infrastructural (no jq, unparseable payload, missing
# library) so a hook bug cannot brick every Edit in the session. The /harden
# gate is the backstop, exactly as cc-plan-gate leans on the Apply-time floor.
# Claude Code only; Codex has no hooks and is covered by the always-loaded rule
# plus that same /harden gate.

set -uo pipefail

INPUT=$(cat)

command -v jq > /dev/null 2>&1 || exit 0

TOOL=$(printf '%s' "$INPUT" | jq -r '.tool_name // empty' 2> /dev/null) || exit 0
case "$TOOL" in
    Edit | Write | Bash) ;;
    *) exit 0 ;;
esac

HOOK_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
LIB="$HOOK_DIR/../../scripts/lib/em_dash_scan.sh"
[ -r "$LIB" ] || exit 0
# shellcheck source=../../scripts/lib/em_dash_scan.sh
source "$LIB"

# ── Bash: the commit-message case ────────────────────────────────────────
# Scoped to `git commit` and nothing else, so an agent auditing the tree
# (`git grep`, `git log --grep`) is never blocked by its own search string.
#
# Within a `git commit` the whole command line is checked, NOT just an
# argument matched as the message. Every attempt to recognise the message
# argument leaks: `-m "text"`, `-m"text"`, `-am "text"`, `--message=text` and a
# `-F` file all reach the same place, and a pattern for the first form misses
# the rest. A `git commit` invocation has no legitimate reason to carry one of
# these characters anywhere, and a message read from a file was already gated
# when that file was written.
if [ "$TOOL" = "Bash" ]; then
    COMMAND=$(printf '%s' "$INPUT" | jq -r '.tool_input.command // empty' 2> /dev/null) || exit 0
    [ -n "$COMMAND" ] || exit 0
    printf '%s' "$COMMAND" | grep -qE '(^|[^[:alnum:]_./-])git([[:space:]]|$)' || exit 0
    printf '%s' "$COMMAND" | grep -qE '(^|[^[:alnum:]_./-])commit([[:space:]]|$)' || exit 0
    em_dash_text_has "$COMMAND" || exit 0
    {
        echo "BLOCKED: no em dashes in commit messages (.claude/rules/no-em-dashes.md)."
        echo
        echo "This commit message carries U+2014 EM DASH or U+2015 HORIZONTAL BAR:"
        echo
        printf '%s\n' "$COMMAND" | sed 's/^/  /'
        echo
        echo "$EM_DASH_ADVICE"
        echo "Rewrite the message and re-run the commit. The rule has no exemptions:"
        echo "commit messages are in scope exactly like file content and chat replies."
    } >&2
    exit 2
fi

# ── Edit / Write: the file-content case ──────────────────────────────────
FILE=$(printf '%s' "$INPUT" | jq -r '.tool_input.file_path // empty' 2> /dev/null) || exit 0
[ -n "$FILE" ] || exit 0

TMP_BASE="$(mktemp "${TMPDIR:-/tmp}/em-dash-base.XXXXXX")" || exit 0
TMP_CAND="$(mktemp "${TMPDIR:-/tmp}/em-dash-cand.XXXXXX")" || exit 0
trap 'rm -f "$TMP_BASE" "$TMP_CAND"' EXIT

if [ "$TOOL" = "Edit" ]; then
    printf '%s' "$INPUT" | jq -r '.tool_input.old_string // ""' > "$TMP_BASE" 2> /dev/null || exit 0
    printf '%s' "$INPUT" | jq -r '.tool_input.new_string // ""' > "$TMP_CAND" 2> /dev/null || exit 0
    WHERE="new_string line"
else
    # The file on disk is the baseline. A Write to a path that does not exist
    # yet leaves it empty, so every line counts as added.
    [ -f "$FILE" ] && cat -- "$FILE" > "$TMP_BASE" 2> /dev/null
    printf '%s' "$INPUT" | jq -r '.tool_input.content // ""' > "$TMP_CAND" 2> /dev/null || exit 0
    WHERE="line"
fi

OFFENDERS="$(em_dash_added_lines "$TMP_BASE" "$TMP_CAND")" || exit 0
[ -n "$OFFENDERS" ] || exit 0

{
    echo "BLOCKED: no em dashes (.claude/rules/no-em-dashes.md)."
    echo
    echo "This $TOOL adds U+2014 EM DASH or U+2015 HORIZONTAL BAR to $FILE:"
    echo
    printf '%s\n' "$OFFENDERS" | awk -v w="$WHERE" '{ n = $0; sub(/:.*$/, "", n); sub(/^[0-9]+:/, ""); printf "  %s %s: %s\n", w, n, $0 }'
    echo
    echo "$EM_DASH_ADVICE"
    echo "Rewrite those lines and retry. There is no file type and no context that"
    echo "is exempt: code comments, doc prose, and error strings all count."
    echo
    echo "Only text this write ADDS is checked. A line that already carried the"
    echo "character and is passing through unchanged will never bring you here."
} >&2
exit 2
