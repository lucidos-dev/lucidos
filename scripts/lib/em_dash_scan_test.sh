#!/bin/bash
# Tests for the no-em-dash gate (`.claude/rules/no-em-dashes.md`): the shared
# library scripts/lib/em_dash_scan.sh, the review-time CLI
# scripts/check-em-dashes.sh, and the write-time hook
# .claude/hooks/no-em-dashes.sh.
#
# Hermetic: the diff-scoped half runs against a throwaway git repo, the hook
# half against synthesized PreToolUse payloads. Nothing reads the real tree, so
# the outcome cannot drift as the repo gains or loses dashes.
#
# The banned characters are never typed as literals here. They come from the
# library's byte escapes, so this file stays clean under the very rule it
# tests and editing it is not blocked by the hook it exercises.
#
# Covered: an added line flagged; a file with PRE-EXISTING dashes on untouched
# lines staying clean (the whole reason the gate is diff-scoped); a reworded
# line that keeps its dash flagged; a deleted dash line clean; U+2015 flagged;
# U+2013 EN DASH never flagged; untracked files scanned whole; the CLI's exit
# status in all three states (clean, hits, cannot-run); and every hook path,
# Edit / Write / `git commit -m` / carried-over lines / unrelated tools.
#
# Run: ./scripts/lib/em_dash_scan_test.sh
set -u

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
# shellcheck source=scripts/lib/em_dash_scan.sh
source "$SCRIPT_DIR/em_dash_scan.sh"
CLI="$SCRIPT_DIR/../check-em-dashes.sh"
HOOK="$SCRIPT_DIR/../../.claude/hooks/no-em-dashes.sh"

EM="$EM_DASH_U2014"
BAR="$EM_DASH_U2015"
EN=$'\xe2\x80\x93' # U+2013 EN DASH, the lookalike that must NOT be flagged.

PASS=0
FAIL=0
pass() { echo "  ok:   $*"; PASS=$((PASS + 1)); }
fail() { echo "  FAIL: $*"; FAIL=$((FAIL + 1)); }

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

# Run the CLI inside the fixture repo. Captures combined output in CLI_OUT and
# the exit code in CLI_RC. Assertions read CLI_RC, never a bare `$?`, which
# survives exactly one command.
run_cli() {
    CLI_OUT=$(cd "$REPO" && bash "$CLI" "$@" 2>&1)
    CLI_RC=$?
}

# Run the hook with a synthesized PreToolUse payload. HOOK_RC of 2 means it
# blocked; 0 means it let the write through.
run_hook() { # <jq-built-payload>
    HOOK_OUT=$(printf '%s' "$1" | "$HOOK" 2>&1 > /dev/null)
    HOOK_RC=$?
}
edit_payload() { # <file> <old_string> <new_string>
    jq -n --arg f "$1" --arg o "$2" --arg n "$3" \
        '{tool_name: "Edit", tool_input: {file_path: $f, old_string: $o, new_string: $n}}'
}
write_payload() { # <file> <content>
    jq -n --arg f "$1" --arg c "$2" \
        '{tool_name: "Write", tool_input: {file_path: $f, content: $c}}'
}
bash_payload() { # <command>
    jq -n --arg c "$1" '{tool_name: "Bash", tool_input: {command: $c}}'
}

# ── Fixture history ──────────────────────────────────────────────────────
# `legacy.md` stands in for the ~29,000 lines already in the real tree: it is
# committed WITH dashes on the base commit, so nothing about it may ever be
# reported unless a later commit touches those lines.
write legacy.md "A settled line ${EM} with a dash already in it.
A second settled line ${EM} likewise.
A clean line nobody has touched."
write clean.md "Nothing to see here."
commit "base"
git -C "$REPO" checkout -qb feature

# ── Diff-scoped scanner ──────────────────────────────────────────────────

test_flags_added_line() {
    echo "test: an added line carrying U+2014 is flagged"
    write added.md "Fresh prose ${EM} with a dash."
    commit "add"
    run_cli
    if [ "$CLI_RC" -eq 1 ] && printf '%s' "$CLI_OUT" | grep -q "added.md:1:"; then
        pass "flagged with path and line"
    else
        fail "expected a hit on added.md:1, rc=$CLI_RC out=$CLI_OUT"
    fi
    if printf '%s' "$CLI_OUT" | grep -q "Fresh prose"; then
        pass "message shows the offending text"
    else
        fail "message must show the offending text: $CLI_OUT"
    fi
    if printf '%s' "$CLI_OUT" | grep -q "comma, a colon, parentheses"; then
        pass "message states the replacement options"
    else
        fail "message must state the replacements: $CLI_OUT"
    fi
    git -C "$REPO" rm -q added.md && commit "undo"
}

test_ignores_untouched_preexisting() {
    echo "test: touching a file with pre-existing dashes on untouched lines is clean"
    write legacy.md "A settled line ${EM} with a dash already in it.
A second settled line ${EM} likewise.
A clean line nobody has touched, now reworded."
    commit "touch the clean line only"
    run_cli
    if [ "$CLI_RC" -eq 0 ]; then
        pass "clean, the pre-existing dashes are not this branch's problem"
    else
        fail "expected clean, rc=$CLI_RC out=$CLI_OUT"
    fi
}

test_flags_reworded_line_that_keeps_its_dash() {
    echo "test: rewording a line that KEEPS its dash is flagged (touch it, own it)"
    write legacy.md "A settled line ${EM} now reworded but still dashed.
A second settled line ${EM} likewise.
A clean line nobody has touched, now reworded."
    commit "reword a dashed line"
    run_cli
    if [ "$CLI_RC" -eq 1 ] && printf '%s' "$CLI_OUT" | grep -q "legacy.md:1:"; then
        pass "flagged"
    else
        fail "expected a hit on legacy.md:1, rc=$CLI_RC out=$CLI_OUT"
    fi
    git -C "$REPO" checkout -q main -- legacy.md && commit "restore legacy.md"
}

test_deleting_a_dash_line_is_clean() {
    echo "test: deleting a dashed line is clean"
    write legacy.md "A second settled line ${EM} likewise.
A clean line nobody has touched."
    commit "delete the first dashed line"
    run_cli
    if [ "$CLI_RC" -eq 0 ]; then
        pass "removals are never hits"
    else
        fail "expected clean, rc=$CLI_RC out=$CLI_OUT"
    fi
    git -C "$REPO" checkout -q main -- legacy.md && commit "restore legacy.md"
}

test_flags_horizontal_bar() {
    echo "test: U+2015 HORIZONTAL BAR is flagged too"
    write bar.md "A lookalike ${BAR} not just U+2014."
    commit "bar"
    run_cli
    if [ "$CLI_RC" -eq 1 ] && printf '%s' "$CLI_OUT" | grep -q "bar.md:1:"; then
        pass "flagged"
    else
        fail "expected a hit on bar.md:1, rc=$CLI_RC out=$CLI_OUT"
    fi
    git -C "$REPO" rm -q bar.md && commit "undo bar"
}

test_en_dash_is_not_flagged() {
    echo "test: U+2013 EN DASH is NOT flagged (legitimate in numeric ranges)"
    write range.md "Valid for 2024${EN}2026, pages 3${EN}5."
    commit "en dash"
    run_cli
    if [ "$CLI_RC" -eq 0 ]; then
        pass "left alone"
    else
        fail "en dash must never be flagged, rc=$CLI_RC out=$CLI_OUT"
    fi
    git -C "$REPO" rm -q range.md && commit "undo en dash"
}

test_uncommitted_and_untracked_are_scanned() {
    echo "test: uncommitted edits and untracked files are scanned"
    printf '%s\n' "Uncommitted ${EM} prose." >> "$REPO/clean.md"
    run_cli
    if [ "$CLI_RC" -eq 1 ] && printf '%s' "$CLI_OUT" | grep -q "clean.md:"; then
        pass "uncommitted working-tree edit flagged"
    else
        fail "expected a hit on clean.md, rc=$CLI_RC out=$CLI_OUT"
    fi
    git -C "$REPO" checkout -q -- clean.md

    printf '%s\n' "Brand new ${EM} file." > "$REPO/untracked.md"
    run_cli
    if [ "$CLI_RC" -eq 1 ] && printf '%s' "$CLI_OUT" | grep -q "untracked.md:1:"; then
        pass "untracked file scanned whole"
    else
        fail "expected a hit on untracked.md:1, rc=$CLI_RC out=$CLI_OUT"
    fi
    rm -f "$REPO/untracked.md"
}

test_clean_branch_exits_zero() {
    echo "test: a clean branch exits 0"
    write ok.md "Prose with a comma, a colon: and parentheses (like so)."
    commit "clean addition"
    run_cli
    if [ "$CLI_RC" -eq 0 ]; then
        pass "exit 0"
    else
        fail "expected exit 0, rc=$CLI_RC out=$CLI_OUT"
    fi
}

test_untracked_enumeration_failure_is_not_clean() {
    echo "test: a failed untracked-file enumeration reports failure, not clean"
    # Fed from a process substitution, git's exit code would vanish: the loop
    # body just would not run and the function would return 0, claiming a clean
    # scan of files it never listed.
    local out rc
    out="$(em_dash_scan_untracked "$REPO/no-such-directory" 2>&1)"
    rc=$?
    if [ "$rc" -ne 0 ] && printf '%s' "$out" | grep -q "ls-files failed"; then
        pass "non-zero with a diagnostic"
    else
        fail "expected a non-zero status, rc=$rc out=$out"
    fi
}

test_unresolvable_base_fails_closed() {
    echo "test: an unresolvable base ref fails, it does not read as clean"
    run_cli --base no-such-ref
    if [ "$CLI_RC" -ne 0 ] && printf '%s' "$CLI_OUT" | grep -q "merge base"; then
        pass "refused with a diagnostic"
    else
        fail "a gate that cannot run must not exit 0, rc=$CLI_RC out=$CLI_OUT"
    fi
}

# ── Write-time hook ──────────────────────────────────────────────────────

test_hook_blocks_edit_adding_a_dash() {
    echo "test: hook blocks an Edit whose new_string adds a dash"
    run_hook "$(edit_payload "/tmp/x.md" "old text" "new text ${EM} with a dash")"
    if [ "$HOOK_RC" -eq 2 ]; then
        pass "blocked"
    else
        fail "expected exit 2, got $HOOK_RC"
    fi
    if printf '%s' "$HOOK_OUT" | grep -q "/tmp/x.md"; then
        pass "names the file"
    else
        fail "message must name the file: $HOOK_OUT"
    fi
    if printf '%s' "$HOOK_OUT" | grep -q "new text"; then
        pass "shows the offending text"
    else
        fail "message must show the offending text: $HOOK_OUT"
    fi
    if printf '%s' "$HOOK_OUT" | grep -q "comma, a colon, parentheses"; then
        pass "states the replacement options"
    else
        fail "message must state the replacements: $HOOK_OUT"
    fi
}

test_hook_allows_carried_over_line() {
    echo "test: hook allows an Edit that CARRIES an existing dashed line along"
    local shared="A settled line ${EM} with a dash already in it."
    run_hook "$(edit_payload "/tmp/x.md" "${shared}"$'\n'"old tail" "${shared}"$'\n'"new tail")"
    if [ "$HOOK_RC" -eq 0 ]; then
        pass "allowed, the dash is not being added"
    else
        fail "expected exit 0, got $HOOK_RC: $HOOK_OUT"
    fi
}

test_hook_blocks_a_duplicated_dashed_line() {
    echo "test: hook blocks a SECOND copy of a line the baseline already had"
    # The baseline vouches for one copy, not two. Plain set membership would
    # let the new copy through; the baseline is a multiset for this reason.
    local shared="A settled line ${EM} with a dash already in it."
    run_hook "$(edit_payload "/tmp/x.md" "${shared}" "${shared}"$'\n'"${shared}")"
    if [ "$HOOK_RC" -eq 2 ]; then
        pass "blocked, the duplicate is an added line"
    else
        fail "expected exit 2, got $HOOK_RC: $HOOK_OUT"
    fi
}

test_hook_allows_clean_edit() {
    echo "test: hook allows a clean Edit"
    run_hook "$(edit_payload "/tmp/x.md" "old" "new text, with a comma")"
    if [ "$HOOK_RC" -eq 0 ]; then
        pass "allowed"
    else
        fail "expected exit 0, got $HOOK_RC: $HOOK_OUT"
    fi
}

test_hook_allows_en_dash_edit() {
    echo "test: hook allows an Edit carrying U+2013 EN DASH"
    run_hook "$(edit_payload "/tmp/x.md" "old" "valid for 2024${EN}2026")"
    if [ "$HOOK_RC" -eq 0 ]; then
        pass "allowed"
    else
        fail "expected exit 0, got $HOOK_RC: $HOOK_OUT"
    fi
}

test_hook_blocks_write_of_new_file() {
    echo "test: hook blocks a Write creating a file with a dash"
    run_hook "$(write_payload "$REPO/brand-new.md" "line one"$'\n'"line two ${EM} dashed")"
    if [ "$HOOK_RC" -eq 2 ] && printf '%s' "$HOOK_OUT" | grep -q "line 2"; then
        pass "blocked, with the line number"
    else
        fail "expected exit 2 naming line 2, got $HOOK_RC: $HOOK_OUT"
    fi
}

test_hook_allows_write_preserving_existing_dashes() {
    echo "test: hook allows a Write that preserves an existing file's dashed lines"
    run_hook "$(write_payload "$REPO/legacy.md" "A settled line ${EM} with a dash already in it."$'\n'"A second settled line ${EM} likewise."$'\n'"A rewritten clean line.")"
    if [ "$HOOK_RC" -eq 0 ]; then
        pass "allowed, no dash is being introduced"
    else
        fail "expected exit 0, got $HOOK_RC: $HOOK_OUT"
    fi
}

test_hook_blocks_commit_message() {
    echo "test: hook blocks a 'git commit -m' whose message carries a dash"
    run_hook "$(bash_payload "git commit -m \"fix(gate): block the thing ${EM} it was noisy\"")"
    if [ "$HOOK_RC" -eq 2 ]; then
        pass "blocked"
    else
        fail "expected exit 2, got $HOOK_RC: $HOOK_OUT"
    fi
    if printf '%s' "$HOOK_OUT" | grep -q "commit message"; then
        pass "message says it is about the commit message"
    else
        fail "message must name the commit message: $HOOK_OUT"
    fi
}

test_hook_blocks_combined_and_attached_commit_flags() {
    echo "test: hook blocks 'git commit -am' and an attached -m\"...\" too"
    # Recognising the message ARGUMENT is what leaks: a pattern for a standalone
    # -m misses both of these. Inside a git commit the whole line is checked.
    run_hook "$(bash_payload "git commit -am \"fix: the thing ${EM} it was noisy\"")"
    if [ "$HOOK_RC" -eq 2 ]; then
        pass "combined -am blocked"
    else
        fail "expected exit 2 for -am, got $HOOK_RC: $HOOK_OUT"
    fi
    run_hook "$(bash_payload "git commit -m\"fix: the thing ${EM} it was noisy\"")"
    if [ "$HOOK_RC" -eq 2 ]; then
        pass "attached -m\"...\" blocked"
    else
        fail "expected exit 2 for attached -m, got $HOOK_RC: $HOOK_OUT"
    fi
    run_hook "$(bash_payload "git commit --message=\"fix: the thing ${EM} it was noisy\"")"
    if [ "$HOOK_RC" -eq 2 ]; then
        pass "--message= blocked"
    else
        fail "expected exit 2 for --message=, got $HOOK_RC: $HOOK_OUT"
    fi
}

test_hook_allows_clean_commit_message() {
    echo "test: hook allows a clean 'git commit -m'"
    run_hook "$(bash_payload 'git commit -m "fix(gate): block the thing, it was noisy"')"
    if [ "$HOOK_RC" -eq 0 ]; then
        pass "allowed"
    else
        fail "expected exit 0, got $HOOK_RC: $HOOK_OUT"
    fi
}

test_hook_allows_searching_for_the_character() {
    echo "test: hook does NOT block a Bash command that merely searches for a dash"
    run_hook "$(bash_payload "git grep -n '${EM}' -- docs/")"
    if [ "$HOOK_RC" -eq 0 ]; then
        pass "allowed, auditing the tree must stay possible"
    else
        fail "expected exit 0, got $HOOK_RC: $HOOK_OUT"
    fi
}

test_hook_ignores_other_tools() {
    echo "test: hook ignores tools it does not gate"
    run_hook "$(jq -n --arg c "read this ${EM} file" '{tool_name: "Read", tool_input: {file_path: $c}}')"
    if [ "$HOOK_RC" -eq 0 ]; then
        pass "passthrough"
    else
        fail "expected exit 0, got $HOOK_RC: $HOOK_OUT"
    fi
}

test_hook_fails_open_on_a_junk_payload() {
    echo "test: hook fails OPEN on an unparseable payload"
    run_hook 'not json at all'
    if [ "$HOOK_RC" -eq 0 ]; then
        pass "allowed, a hook bug must not brick every Edit"
    else
        fail "expected exit 0, got $HOOK_RC: $HOOK_OUT"
    fi
}

echo "── diff-scoped scanner ──"
test_flags_added_line
test_ignores_untouched_preexisting
test_flags_reworded_line_that_keeps_its_dash
test_deleting_a_dash_line_is_clean
test_flags_horizontal_bar
test_en_dash_is_not_flagged
test_uncommitted_and_untracked_are_scanned
test_clean_branch_exits_zero
test_untracked_enumeration_failure_is_not_clean
test_unresolvable_base_fails_closed

echo "── write-time hook ──"
test_hook_blocks_edit_adding_a_dash
test_hook_allows_carried_over_line
test_hook_blocks_a_duplicated_dashed_line
test_hook_allows_clean_edit
test_hook_allows_en_dash_edit
test_hook_blocks_write_of_new_file
test_hook_allows_write_preserving_existing_dashes
test_hook_blocks_commit_message
test_hook_blocks_combined_and_attached_commit_flags
test_hook_allows_clean_commit_message
test_hook_allows_searching_for_the_character
test_hook_ignores_other_tools
test_hook_fails_open_on_a_junk_payload

echo
echo "passed: $PASS   failed: $FAIL"
[ "$FAIL" -eq 0 ]
