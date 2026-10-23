#!/bin/bash
# Tests for the mirrored-rule gate: the shared library
# scripts/lib/prompt_mirror_scan.sh and the CLI scripts/check-prompt-mirror.sh.
#
# Hermetic. Every CLI case runs against a throwaway git repo holding a COPY of
# the script and its library, with fixture files at the two real paths. Nothing
# reads the real tree, so no outcome moves when CLAUDE.md or prompts.rs is
# reworded. The one exception is the last case, which deliberately asserts the
# real tree passes its own gate.
#
# The cases that matter are the two fail arms, one per surface, because the
# whole point of the gate is that neither half can vanish quietly. The
# CLAUDE.md-only arm is the one no `cargo test` would ever reach.
#
# The split-line case is not padding: in prompts.rs the prohibition is a Rust
# string literal broken across continuation lines, so "NEVER use" and "pkill"
# genuinely sit on different source lines. A single-line grep would report the
# real tree as broken.
#
# Run: ./scripts/lib/prompt_mirror_scan_test.sh
set -u

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
# shellcheck source=scripts/lib/prompt_mirror_scan.sh
source "$SCRIPT_DIR/prompt_mirror_scan.sh"
CLI="$SCRIPT_DIR/../check-prompt-mirror.sh"

ENGINE_PATH="crates/lucidos-engine/src/engine/agent_session/prompts.rs"

PASS=0
FAIL=0
pass() {
    echo "  ok:   $*"
    PASS=$((PASS + 1))
}
fail() {
    echo "  FAIL: $*"
    FAIL=$((FAIL + 1))
}

TMP="$(mktemp -d)"
trap 'rm -rf "$TMP"' EXIT

# ---------------------------------------------------------------------------
# prompt_mirror_missing_tokens
# ---------------------------------------------------------------------------
echo "prompt_mirror_missing_tokens:"

# tokens_case <name> <expected comma list, empty for none> <body>
tokens_case() {
    local name="$1" expect="$2" body="$3"
    local file="$TMP/tokens-$name.txt"
    printf '%s' "$body" >"$file"
    local got
    got="$(prompt_mirror_missing_tokens "$file" | paste -sd, - | tr -d ' ')"
    if [ "$got" = "$expect" ]; then
        pass "$name reports '${expect:-none}'"
    else
        fail "$name: expected '${expect:-none}', got '${got:-none}'"
    fi
}

tokens_case "all-present" "" \
    'NEVER use pkill or killall on lucidos-engine.
'
tokens_case "killall-dropped" "killall" \
    'NEVER use pkill on lucidos-engine.
'
tokens_case "process-name-dropped" "lucidos-engine" \
    'NEVER use pkill or killall broadly.
'
tokens_case "all-dropped" "pkill,killall,lucidos-engine" \
    'Stop a workspace with the stop script.
'

# ---------------------------------------------------------------------------
# prompt_mirror_has_prohibition
# ---------------------------------------------------------------------------
echo
echo "prompt_mirror_has_prohibition:"

# phrase_case <name> <expected: yes|no> <body>
phrase_case() {
    local name="$1" expect="$2" body="$3"
    local file="$TMP/phrase-$name.txt"
    printf '%s' "$body" >"$file"
    local got
    if prompt_mirror_has_prohibition "$file"; then got="yes"; else got="no"; fi
    if [ "$got" = "$expect" ]; then
        pass "$name is $expect"
    else
        fail "$name: expected $expect, got $got"
    fi
}

phrase_case "same-line" yes \
    'NEVER use pkill against lucidos-engine.
'
phrase_case "lowercase" yes \
    'You should never reach for pkill here.
'

# The real shape in prompts.rs: a Rust string literal split with trailing
# backslashes, so the negation and the command land on different lines.
# shellcheck disable=SC2016 # fixture prose: the backticks are literal markdown, not command substitution
phrase_case "split-across-lines" yes \
    '    format!(
        "PROCESS SAFETY: Multiple Lucidos workspaces run concurrently. NEVER use \
         `pkill -f lucidos-engine`, `killall lucidos-engine`, or any broad kill."
    )
'

phrase_case "described-not-forbidden" no \
    'Some people stop the engine with pkill and it works fine for them.
'

# A negation elsewhere in the file must not vouch for an unguarded mention.
phrase_case "negation-out-of-range" no \
    'You should never hardcode a database URL.
line 2
line 3
line 4
line 5
Stopping a workspace is usually done with pkill these days.
'

# ---------------------------------------------------------------------------
# prompt_mirror_scan
# ---------------------------------------------------------------------------
echo
echo "prompt_mirror_scan:"

# shellcheck disable=SC2016 # fixture prose: the backticks are literal, mirroring how the real rule is written
GOOD_ENGINE='// PROCESS SAFETY rule.
const PROCESS_SAFETY: &str = "NEVER use \
    `pkill -f lucidos-engine`, `killall lucidos-engine`, or any broad kill.";
'
# shellcheck disable=SC2016 # fixture prose: the backticks are literal markdown, not command substitution
GOOD_CLAUDE='# CLAUDE.md

- **Never kill broadly.** NEVER use `pkill`, `killall`, or a broad kill on
  `lucidos-engine` (ADR 0025).
'

# make_tree <dir> <engine body> <claude body>; omit a body with the literal SKIP
make_tree() {
    local dir="$1" engine="$2" claude="$3"
    mkdir -p "$dir/$(dirname "$ENGINE_PATH")"
    [ "$engine" = "SKIP" ] || printf '%s' "$engine" >"$dir/$ENGINE_PATH"
    [ "$claude" = "SKIP" ] || printf '%s' "$claude" >"$dir/CLAUDE.md"
}

# scan_case <name> <expected verdict line, tab-joined> <engine> <claude>
scan_case() {
    local name="$1" expect="$2" engine="$3" claude="$4"
    local dir="$TMP/scan-$name"
    mkdir -p "$dir"
    make_tree "$dir" "$engine" "$claude"
    local got
    got="$(prompt_mirror_scan "$dir")"
    if [ "$got" = "$(printf '%b' "$expect")" ]; then
        pass "$name"
    else
        fail "$name: expected [$(printf '%b' "$expect")], got [$got]"
    fi
}

scan_case "both-halves-present" "ok\t$ENGINE_PATH\nok\tCLAUDE.md" \
    "$GOOD_ENGINE" "$GOOD_CLAUDE"
scan_case "claude-half-deleted" "ok\t$ENGINE_PATH\ntokens\tCLAUDE.md\tpkill,killall,lucidos-engine" \
    "$GOOD_ENGINE" '# CLAUDE.md

Nothing about process safety at all.
'
scan_case "claude-file-missing" "ok\t$ENGINE_PATH\nabsent\tCLAUDE.md" \
    "$GOOD_ENGINE" "SKIP"
# shellcheck disable=SC2016 # fixture prose: the backticks are literal, mirroring how the real rule is written
scan_case "engine-half-softened" "phrase\t$ENGINE_PATH\nok\tCLAUDE.md" \
    'const NOTE: &str = "You can stop things with `pkill -f lucidos-engine` or `killall`.";
' "$GOOD_CLAUDE"

# ---------------------------------------------------------------------------
# The CLI, against throwaway repos
# ---------------------------------------------------------------------------
echo
echo "check-prompt-mirror.sh:"

# The library holds a constant file list and a constant needle, so unlike the
# context-budget gate there is nothing to patch: the fixture just puts files at
# the two real paths.
make_repo() {
    local dir="$1" engine="$2" claude="$3"
    mkdir -p "$dir/scripts/lib"
    cp "$CLI" "$dir/scripts/check-prompt-mirror.sh"
    cp "$SCRIPT_DIR/prompt_mirror_scan.sh" "$dir/scripts/lib/prompt_mirror_scan.sh"
    chmod +x "$dir/scripts/check-prompt-mirror.sh"
    make_tree "$dir" "$engine" "$claude"
    git -C "$dir" init -q -b main
    git -C "$dir" config user.email "t@t"
    git -C "$dir" config user.name "t"
    git -C "$dir" add -A
    git -C "$dir" commit -qm "fixture"
}

# run_cli <dir> [args...] -> sets CLI_OUT and CLI_RC in the CALLER. Not a
# command substitution at the call site: that runs the function in a subshell,
# and the exit status assigned there never reaches the caller.
CLI_OUT=""
CLI_RC=0
run_cli() {
    local dir="$1"
    shift
    CLI_OUT="$(cd "$dir" && ./scripts/check-prompt-mirror.sh "$@" 2>&1)"
    CLI_RC=$?
}

# 1. Both halves present: clean.
R="$TMP/cli-clean"
make_repo "$R" "$GOOD_ENGINE" "$GOOD_CLAUDE"
run_cli "$R"
if [ "$CLI_RC" -eq 0 ]; then
    pass "both halves present exits 0"
else
    fail "both halves present exited $CLI_RC: $CLI_OUT"
fi

# 2. The CLAUDE.md half deleted. THE case that motivates a shell gate: a
#    docs-only edit, which never runs cargo test.
R="$TMP/cli-no-claude"
make_repo "$R" "$GOOD_ENGINE" '# CLAUDE.md

Someone deduplicated a little too enthusiastically.
'
run_cli "$R"
if [ "$CLI_RC" -ne 0 ]; then
    pass "missing CLAUDE.md half is blocked"
else
    fail "missing CLAUDE.md half passed: $CLI_OUT"
fi
case "$CLI_OUT" in
    *"CLAUDE.md"*) pass "the message names the surface that lost it" ;;
    *) fail "the message does not name CLAUDE.md: $CLI_OUT" ;;
esac

# 3. The engine half deleted.
R="$TMP/cli-no-engine"
make_repo "$R" 'const NOTHING: &str = "no process-safety rule here";
' "$GOOD_CLAUDE"
run_cli "$R"
if [ "$CLI_RC" -ne 0 ]; then
    pass "missing engine half is blocked"
else
    fail "missing engine half passed: $CLI_OUT"
fi

# 4. --report never fails, even when a half is missing.
run_cli "$R" --report
if [ "$CLI_RC" -eq 0 ]; then
    pass "--report exits 0 with a half missing"
else
    fail "--report exited $CLI_RC: $CLI_OUT"
fi

# 5. An unknown argument is refused rather than ignored.
run_cli "$R" --bogus
if [ "$CLI_RC" -ne 0 ]; then
    pass "unknown argument is refused"
else
    fail "unknown argument was accepted"
fi

# 6. The real tree passes its own gate. The only non-hermetic case, and the
#    reason it is here: everything above proves the gate can fail, this proves
#    it is satisfied right now.
echo
echo "the real tree:"
REAL_ROOT="$(cd "$SCRIPT_DIR/../.." && pwd)"
REAL_OUT=""
if REAL_OUT="$(cd "$REAL_ROOT" && ./scripts/check-prompt-mirror.sh 2>&1)"; then
    pass "this repo satisfies the mirror gate"
else
    fail "this repo fails its own mirror gate: $REAL_OUT"
fi

echo
echo "passed: $PASS, failed: $FAIL"
[ "$FAIL" -eq 0 ]
