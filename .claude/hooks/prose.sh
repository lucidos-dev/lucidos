#!/bin/bash
# PreToolUse hook: refuse to WRITE prose that breaks one of the four limits in
# `.claude/rules/prose.md`. The primary gate for that rule, and the only layer
# that stops the text before it exists: prose alone is what already failed, and
# the review-time scanner (scripts/check-prose.sh, /harden Phase 4.5) can only
# catch it after the fact.
#
# Wired into `.claude/settings.json` on two tools, Edit and Write. Deliberately
# NOT on Bash: unlike an em dash, a long sentence in a commit message is not
# worth blocking a commit over, and the four limits are about files.
#
# ADDED LINES ONLY. For Edit, a line carried over from old_string is not a hit;
# for Write, neither is a line already in the file on disk. The tree carries
# 143,575 comment lines and the rule is deliberately not retroactive, so
# touching such a file must not be blocked. Rewording a line that stays too long
# IS a hit: touch a line and you own it.
#
# Blocks with exit 2 and an actionable stderr message, matching its neighbours
# pre-push.sh, pre-kill.sh and no-em-dashes.sh. The message names the file, the
# line and the limit, so the fix needs no further questions.
#
# FAILS OPEN on anything infrastructural (no jq, unparseable payload, missing
# library) so a hook bug cannot brick every Edit in the session. The /harden
# gate is the backstop. Claude Code only; Codex has no hooks and is covered by
# the always-loaded rule plus that same /harden gate.

set -uo pipefail

INPUT=$(cat)

command -v jq > /dev/null 2>&1 || exit 0

TOOL=$(printf '%s' "$INPUT" | jq -r '.tool_name // empty' 2> /dev/null) || exit 0
case "$TOOL" in
    Edit | Write) ;;
    *) exit 0 ;;
esac

HOOK_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
LIB="$HOOK_DIR/../../scripts/lib/prose_scan.sh"
[ -r "$LIB" ] || exit 0
# shellcheck source=../../scripts/lib/prose_scan.sh
source "$LIB"

FILE=$(printf '%s' "$INPUT" | jq -r '.tool_input.file_path // empty' 2> /dev/null) || exit 0
[ -n "$FILE" ] || exit 0

# Nothing to say about this file type, so do not pay for a scan.
[ -n "$(prose_kind "$FILE")" ] || exit 0

TMP_BASE="$(mktemp "${TMPDIR:-/tmp}/prose-base.XXXXXX")" || exit 0
TMP_CAND="$(mktemp "${TMPDIR:-/tmp}/prose-cand.XXXXXX")" || exit 0
trap 'rm -f "$TMP_BASE" "$TMP_CAND"' EXIT

if [ "$TOOL" = "Edit" ]; then
    printf '%s' "$INPUT" | jq -r '.tool_input.old_string // ""' > "$TMP_BASE" 2> /dev/null || exit 0
    printf '%s' "$INPUT" | jq -r '.tool_input.new_string // ""' > "$TMP_CAND" 2> /dev/null || exit 0

    # An Edit is a FRAGMENT, so it carries no fence opener and a line added
    # inside an existing code block would read as prose and be blocked. That
    # false block is the failure that gets a gate switched off, so anchor the
    # fragment in the real file: find where old_string starts and stand down if
    # that point is inside a fence. Anchoring on the first line of old_string is
    # enough, and a fragment that cannot be located falls through to the plain
    # check rather than being skipped.
    ANCHOR="$(head -n 1 "$TMP_BASE")"
    if [ -n "$ANCHOR" ] && [ -f "$FILE" ]; then
        if awk -v anchor="$ANCHOR" '
            index($0, anchor) && !found { found = 1; exit infence ? 0 : 1 }
            /^[[:space:]]*(```|~~~)/ { infence = !infence }
            END { if (!found) exit 1 }
        ' "$FILE"; then
            exit 0
        fi
    fi
else
    # The file on disk is the baseline. A Write to a path that does not exist
    # yet leaves it empty, so every line counts as added.
    [ -f "$FILE" ] && cat -- "$FILE" > "$TMP_BASE" 2> /dev/null
    printf '%s' "$INPUT" | jq -r '.tool_input.content // ""' > "$TMP_CAND" 2> /dev/null || exit 0
fi

OFFENDERS="$(prose_added_lines "$TMP_BASE" "$TMP_CAND" "$FILE")" || exit 0
[ -n "$OFFENDERS" ] || exit 0

{
    echo "BLOCKED: prose limit (.claude/rules/prose.md)."
    echo
    echo "This $TOOL adds text that breaks a limit in $FILE:"
    echo
    printf '%s\n' "$OFFENDERS" | sed 's/^/  line /'
    echo
    echo "$PROSE_ADVICE"
    echo "Limits: $PROSE_MAX_COMMENT_BLOCK-line comment block, $PROSE_MAX_SENTENCE_WORDS-word sentence,"
    echo "$PROSE_MAX_PARAGRAPH_SENTENCES-sentence paragraph, and no ISO date in a comment."
    echo "A dated PATH is fine: link the plan instead of narrating the history."
    echo
    echo "Only text this write ADDS is checked. A line that was already over a"
    echo "limit and is passing through unchanged will never bring you here."
} >&2
exit 2
