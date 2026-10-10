#!/bin/bash
# Tests for the prose gate (`.claude/rules/prose.md`): the shared library
# scripts/lib/prose_scan.sh, the review-time CLI scripts/check-prose.sh, and the
# write-time hook .claude/hooks/prose.sh.
#
# Hermetic: the diff-scoped half runs against a throwaway git repo, the hook
# half against synthesized PreToolUse payloads. Nothing reads the real tree, so
# the outcome cannot drift as the repo gains or loses long sentences.
#
# Every limit is read from the library rather than typed here, so a threshold
# change cannot leave a test asserting the old number. Fixtures are GENERATED to
# sit either side of the limit for the same reason.
#
# Covered: each of the four limits firing; the three false positives that would
# make the gate untrustworthy (a table row, a fenced code block, a dated PATH);
# hard-wrapped sentences joined before measuring; non-adjacent added lines NOT
# reading as one run; pre-existing over-limit prose on untouched lines staying
# clean; a reworded line still over the limit flagged; untracked files scanned
# whole; unhandled file types ignored; the CLI's exit status in all three states
# (clean, hits, cannot-run); and every hook path.
#
# Run: ./scripts/lib/prose_scan_test.sh
set -u

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
# shellcheck source=scripts/lib/prose_scan.sh
source "$SCRIPT_DIR/prose_scan.sh"
CLI="$SCRIPT_DIR/../check-prose.sh"
HOOK="$SCRIPT_DIR/../../.claude/hooks/prose.sh"

PASS=0
FAIL=0
pass() { echo "  ok:   $*"; PASS=$((PASS + 1)); }
fail() { echo "  FAIL: $*"; FAIL=$((FAIL + 1)); }

# Sentences built to order, so no test hardcodes a word count. `words n` yields
# n words followed by a full stop.
words() { # <n>
    local i out=""
    for ((i = 1; i <= $1; i++)); do out="$out word$i"; done
    printf '%s.' "${out# }"
}
OVER_SENTENCE="$(words $((PROSE_MAX_SENTENCE_WORDS + 5)))"
UNDER_SENTENCE="$(words $((PROSE_MAX_SENTENCE_WORDS - 5)))"

# n one-word sentences, for the paragraph limit.
sentences() { # <n>
    local i out=""
    for ((i = 1; i <= $1; i++)); do out="$out s$i."; done
    printf '%s' "${out# }"
}
OVER_PARA="$(sentences $((PROSE_MAX_PARAGRAPH_SENTENCES + 2)))"
UNDER_PARA="$(sentences $((PROSE_MAX_PARAGRAPH_SENTENCES - 1)))"

# A comment block one line past the limit, and one line under it.
comment_block() { # <n>
    local i
    for ((i = 1; i <= $1; i++)); do printf '// b%d\n' "$i"; done
}

REPO="$(mktemp -d)"
trap 'rm -rf "$REPO"' EXIT
git -C "$REPO" init -q -b main
git -C "$REPO" config user.email "t@t"
git -C "$REPO" config user.name "t"

write() { # <relpath> <content>
    mkdir -p "$REPO/$(dirname "$1")"
    printf '%s\n' "$2" > "$REPO/$1"
}
commit() { git -C "$REPO" add -A && git -C "$REPO" commit -qm "$1"; }

# Feed whole-file content through the checker as if every line were added.
check_text() { # <kind> <text>
    printf '%s\n' "$2" | awk '{ printf "%d\t%s\n", FNR, $0 }' | prose_check_lines "$1"
}

run_cli() {
    CLI_OUT=$(cd "$REPO" && bash "$CLI" "$@" 2>&1)
    CLI_RC=$?
}
run_hook() { # <payload>
    HOOK_OUT=$(printf '%s' "$1" | "$HOOK" 2>&1 > /dev/null)
    HOOK_RC=$?
}
edit_payload() { # <file> <old> <new>
    jq -n --arg f "$1" --arg o "$2" --arg n "$3"  '{tool_name: "Edit", tool_input: {file_path: $f, old_string: $o, new_string: $n}}'
}
write_payload() { # <file> <content>
    jq -n --arg f "$1" --arg c "$2"  '{tool_name: "Write", tool_input: {file_path: $f, content: $c}}'
}

echo "prose_kind"
if [ "$(prose_kind a.md)" = "md" ]; then pass "markdown"; else fail "markdown"; fi
if [ "$(prose_kind a.rs)" = "src" ]; then pass "rust"; else fail "rust"; fi
if [ "$(prose_kind a.tsx)" = "src" ]; then pass "tsx"; else fail "tsx"; fi
if [ -z "$(prose_kind a.css)" ]; then pass "css is out of scope"; else fail "css is out of scope"; fi
if [ -z "$(prose_kind a.sh)" ]; then pass "shell is out of scope"; else fail "shell is out of scope"; fi

echo "the four limits fire"
if check_text md "$OVER_SENTENCE" | grep -q 'sentence of'; then pass "sentence over the limit"; else fail "sentence over the limit"; fi
if check_text md "$OVER_PARA" | grep -q 'paragraph of'; then pass "paragraph over the limit"; else fail "paragraph over the limit"; fi
if check_text src "$(comment_block $((PROSE_MAX_COMMENT_BLOCK + 1)))" | grep -q 'comment block'; then pass "comment block over the limit"; else fail "comment block over the limit"; fi
if check_text src '// it broke until 2026-08-12 and was fixed' | grep -q 'ISO date'; then pass "ISO date in a comment"; else fail "ISO date in a comment"; fi

echo "the date check sees a TRAILING comment, not just a full-line one"
# `let x = 1; // broke on <date>` is narration too, and a check anchored at the
# line start misses every one of them.
if check_text src 'let x = 1; // it broke until 2026-08-12' | grep -q 'ISO date'; then pass "trailing comment carrying a date"; else fail "trailing comment carrying a date"; fi
# The `//` of a URL is always preceded by a colon, which is what keeps a dated
# query string or path out of the gate.
if [ -z "$(check_text src 'const u = "https://example.com/a?d=2026-08-13";')" ]; then pass "a URL is not a comment"; else fail "a URL is not a comment"; fi
if [ -z "$(check_text src 'let x = 1; // see docs/plans/2026-08-13-a-plan.md')" ]; then pass "dated path in a trailing comment"; else fail "dated path in a trailing comment"; fi

echo "at the limit is clean"
if [ -z "$(check_text md "$UNDER_SENTENCE")" ]; then pass "short sentence"; else fail "short sentence"; fi
if [ -z "$(check_text md "$UNDER_PARA")" ]; then pass "short paragraph"; else fail "short paragraph"; fi
if [ -z "$(check_text src "$(comment_block "$PROSE_MAX_COMMENT_BLOCK")")" ]; then pass "block exactly at the limit"; else fail "block exactly at the limit"; fi
if [ -z "$(check_text md "$(words "$PROSE_MAX_SENTENCE_WORDS")")" ]; then pass "sentence exactly at the limit"; else fail "sentence exactly at the limit"; fi

echo "false positives that would make the gate untrustworthy"
# A table row is not a sentence. Without this the gate fires on every wide table.
if [ -z "$(check_text md "| $(words 40) | b |")" ]; then pass "markdown table row"; else fail "markdown table row"; fi
# A fenced block is code, whatever its line length.
# shellcheck disable=SC2016 # literal markdown fence, nothing to expand
if [ -z "$(check_text md "$(printf '```\n%s\n```' "$(words 40)")")" ]; then pass "fenced code block"; else fail "fenced code block"; fi
# A dated PATH is the behaviour the rule ASKS for, so it must not be a hit.
if [ -z "$(check_text src '// see docs/plans/2026-08-13-a-plan.md for the reasoning')" ]; then pass "dated path in a comment"; else fail "dated path in a comment"; fi
# Non-prose lines in source must not accumulate into a comment block.
if [ -z "$(check_text src "$(printf 'let a = 1;\n%.0s' $(seq 1 30)))")" ]; then pass "code lines are not a comment block"; else fail "code lines are not a comment block"; fi
# A run of list items is a run of units, not one giant paragraph.
if [ -z "$(check_text md "$(printf -- '- s%d. \n' $(seq 1 10))")" ]; then pass "list items are separate units"; else fail "list items are separate units"; fi

echo "wrapped and split lines"
# One sentence hard-wrapped across lines is still one sentence. This is the case
# a per-line check misses, and it is most of the repo's rule files.
WRAPPED="$(printf 'word1 word2 word3 word4 word5 word6 word7\nword8 word9 word10 word11 word12 word13\nword14 word15 word16 word17 word18 word19\nword20 word21 word22 word23 word24 word25 word26 word27.')"
if check_text md "$WRAPPED" | grep -q 'sentence of 27 words'; then pass "wrapped sentence joined before measuring"; else fail "wrapped sentence joined before measuring"; fi
# Two separate three-line additions must not read as one six-line run.
NONADJACENT="$(printf '1\t// a\n2\t// b\n3\t// c\n90\t// d\n91\t// e\n92\t// f\n')"
if [ -z "$(printf '%s' "$NONADJACENT" | prose_check_lines src)" ]; then pass "non-adjacent runs stay separate"; else fail "non-adjacent runs stay separate"; fi

echo "diff-scoped: pre-existing prose is not this branch's problem"
# The base lands on main and every case below runs on a BRANCH off it. On main
# itself the merge-base is HEAD, so the diff is empty and every assertion would
# pass vacuously; two of them did before this was fixed.
write "a.md" "$OVER_SENTENCE"
commit "base with an over-long sentence"

git -C "$REPO" checkout -q -b untouched main
write "b.md" "$UNDER_SENTENCE"
commit "add an unrelated short file"
run_cli
if [ "$CLI_RC" -eq 0 ]; then pass "untouched over-long line stays clean"; else fail "untouched over-long line stays clean ($CLI_OUT)"; fi

git -C "$REPO" checkout -q -b appended main
write "a.md" "$(printf '%s\nand a new short line.' "$OVER_SENTENCE")"
commit "touch the file without touching the long line"
run_cli
if [ "$CLI_RC" -eq 0 ]; then pass "touching the file does not flag its old prose"; else fail "touching the file does not flag its old prose ($CLI_OUT)"; fi

git -C "$REPO" checkout -q -b reworded main
write "a.md" "$(printf '%s extra.' "$OVER_SENTENCE")"
commit "reword the long line, still long"
run_cli
if [ "$CLI_RC" -eq 1 ]; then pass "rewording a long line owns it"; else fail "rewording a long line owns it ($CLI_OUT)"; fi

git -C "$REPO" checkout -q -b shortened main
write "a.md" "$UNDER_SENTENCE"
commit "shorten the long line"
run_cli
if [ "$CLI_RC" -eq 0 ]; then pass "shortening a long line clears it"; else fail "shortening a long line clears it ($CLI_OUT)"; fi

echo "a line added inside an EXISTING fence is code, not prose"
# The diff carries no fence opener, so the in-stream toggle cannot see one. The
# mask taken from the file on disk is what keeps this from being a false BLOCK,
# and a false block is what gets a gate switched off. This was the very first
# line the gate ever judged, and it got it wrong.
git -C "$REPO" checkout -q -b fenced main
# shellcheck disable=SC2016 # literal markdown fence, nothing to expand
write "f.md" "$(printf 'intro.\n\n```bash\n./a.sh   # short\n```\n')"
commit "a file with a fenced block"
# shellcheck disable=SC2016 # literal markdown fence, nothing to expand
write "f.md" "$(printf 'intro.\n\n```bash\n./a.sh   # short\n./b.sh   # %s\n```\n' "$OVER_SENTENCE")"
commit "add a long line inside the fence"
run_cli
if [ "$CLI_RC" -eq 0 ]; then pass "added line inside a fence is not flagged"; else fail "added line inside a fence is not flagged ($CLI_OUT)"; fi

# The same file's real prose is still judged, so the mask is not a blanket skip.
# shellcheck disable=SC2016 # literal markdown fence, nothing to expand
write "f.md" "$(printf 'intro.\n\n```bash\n./a.sh   # short\n```\n\n%s\n' "$OVER_SENTENCE")"
commit "add long prose outside the fence"
run_cli
if [ "$CLI_RC" -eq 1 ]; then pass "prose outside the fence is still flagged"; else fail "prose outside the fence is still flagged"; fi

if command -v jq > /dev/null 2>&1; then
    # The hook sees a FRAGMENT with no opener, so it anchors old_string in the
    # real file and stands down when that point is inside a fence.
    run_hook "$(edit_payload "$REPO/f.md" './a.sh   # short' "./a.sh   # $OVER_SENTENCE")"
    if [ "$HOOK_RC" -eq 0 ]; then pass "hook stands down inside a fence"; else fail "hook stands down inside a fence ($HOOK_OUT)"; fi
    run_hook "$(edit_payload "$REPO/f.md" 'intro.' "$OVER_SENTENCE")"
    if [ "$HOOK_RC" -eq 2 ]; then pass "hook still blocks outside a fence"; else fail "hook still blocks outside a fence"; fi
fi

echo "CLI exit states"
git -C "$REPO" checkout -q -b clean-branch main
write "c.md" "$UNDER_SENTENCE"
commit "a branch that adds only short prose"
run_cli
if [ "$CLI_RC" -eq 0 ] && printf '%s' "$CLI_OUT" | grep -q '✓'; then pass "clean branch reports clean"; else fail "clean branch reports clean"; fi
run_cli --base does-not-exist
if [ "$CLI_RC" -eq 1 ] && printf '%s' "$CLI_OUT" | grep -q 'cannot run'; then pass "missing base fails closed"; else fail "missing base fails closed"; fi
run_cli --base
if [ "$CLI_RC" -eq 1 ]; then pass "--base with no ref is an error"; else fail "--base with no ref is an error"; fi

echo "untracked files are scanned whole"
write "untracked.md" "$OVER_SENTENCE"
UNTRACKED="$(cd "$REPO" && prose_scan_untracked .)"
if printf '%s' "$UNTRACKED" | grep -q 'untracked.md'; then pass "untracked file flagged"; else fail "untracked file flagged"; fi
rm -f "$REPO/untracked.md"

echo "hook"
if command -v jq > /dev/null 2>&1; then
    run_hook "$(edit_payload /tmp/x.md "old" "$OVER_SENTENCE")"
    if [ "$HOOK_RC" -eq 2 ]; then pass "Edit adding a long sentence is blocked"; else fail "Edit adding a long sentence is blocked"; fi

    run_hook "$(edit_payload /tmp/x.md "$OVER_SENTENCE" "$OVER_SENTENCE")"
    if [ "$HOOK_RC" -eq 0 ]; then pass "carried-over line is not a hit"; else fail "carried-over line is not a hit"; fi

    run_hook "$(write_payload /tmp/x.md "$OVER_SENTENCE")"
    if [ "$HOOK_RC" -eq 2 ]; then pass "Write of a long sentence is blocked"; else fail "Write of a long sentence is blocked"; fi

    run_hook "$(write_payload /tmp/x.md "$UNDER_SENTENCE")"
    if [ "$HOOK_RC" -eq 0 ]; then pass "Write of short prose passes"; else fail "Write of short prose passes"; fi

    run_hook "$(edit_payload /tmp/x.css "old" "$OVER_SENTENCE")"
    if [ "$HOOK_RC" -eq 0 ]; then pass "unhandled file type is ignored"; else fail "unhandled file type is ignored"; fi

    run_hook "$(jq -n '{tool_name: "Read", tool_input: {file_path: "/tmp/x.md"}}')"
    if [ "$HOOK_RC" -eq 0 ]; then pass "an unrelated tool is ignored"; else fail "an unrelated tool is ignored"; fi

    # Fails OPEN on a payload it cannot parse: a hook bug must not brick a session.
    run_hook 'not json at all'
    if [ "$HOOK_RC" -eq 0 ]; then pass "unparseable payload fails open"; else fail "unparseable payload fails open ($HOOK_OUT)"; fi

    run_hook "$(jq -n '{tool_name: "Edit", tool_input: {}}')"
    if [ "$HOOK_RC" -eq 0 ]; then pass "missing file_path fails open"; else fail "missing file_path fails open"; fi
else
    echo "  skip: jq not installed, hook tests skipped"
fi

echo
echo "passed $PASS, failed $FAIL"
[ "$FAIL" -eq 0 ]
