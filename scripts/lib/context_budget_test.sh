#!/bin/bash
# Tests for the always-loaded-context gate: the shared library
# scripts/lib/context_budget.sh and the CLI scripts/check-context-budget.sh.
#
# Hermetic. Every case runs against a throwaway git repo holding a COPY of the
# script and its library, with the ceiling and the expected-membership list
# patched per case. Nothing reads the real tree, so no test outcome moves when
# CLAUDE.md gains or loses a paragraph. The one exception is the last case,
# which deliberately asserts the real tree passes its own gate.
#
# The classifier cases are the ones that matter. Every way a rule can be
# resident WITHOUT anyone intending it is covered: a 'globs:' key (the Cursor
# convention Claude Code ignores, which put the whole rule set in every session
# until 2026-07-25), a 'path:' singular typo, a 'paths:' of exactly '**', and
# frontmatter that never closes. Each looks like a scoped rule to a reader and
# is resident to Claude Code.
#
# Run: ./scripts/lib/context_budget_test.sh
set -u

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
# shellcheck source=scripts/lib/context_budget.sh
source "$SCRIPT_DIR/context_budget.sh"
CLI="$SCRIPT_DIR/../check-context-budget.sh"

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
# context_budget_is_always_loaded
# ---------------------------------------------------------------------------
echo "context_budget_is_always_loaded:"

# classify <name> <expected: always|scoped> <file body>
classify() {
    local name="$1" expect="$2" body="$3"
    local file="$TMP/rule-$name.md"
    printf '%s' "$body" >"$file"
    local got
    if context_budget_is_always_loaded "$file"; then got="always"; else got="scoped"; fi
    if [ "$got" = "$expect" ]; then
        pass "$name is $expect"
    else
        fail "$name: expected $expect, got $got"
    fi
}

classify "no-frontmatter" always \
    '# Plain rule

No frontmatter at all, so it is resident. The documented, intended way.
'

classify "real-paths" scoped '---
paths:
  - "crates/lucidos-engine/**/*.rs"
  - "Cargo.toml"
---

# Rust
'

classify "globs-key" always '---
globs:
  - "crates/lucidos-engine/**/*.rs"
---

# The 2026-07-25 regression: reads as scoped, is resident.
'

classify "path-singular-typo" always '---
path:
  - "crates/lucidos-app/src/**/*.ts"
---

# Frontend
'

classify "paths-double-star-only" always '---
paths:
  - "**"
---

# Matches everything, so scoping it is the same as not scoping it.
'

classify "paths-double-star-plus-real" scoped '---
paths:
  - "**"
  - "scripts/build.sh"
---

# One real pattern is enough to make it conditional.
'

classify "paths-inline-flow" scoped '---
paths: ["packages/lucidos-sdk/**", "docs/sdk.md"]
---

# SDK
'

classify "paths-inline-flow-double-star" always '---
paths: ["**"]
---

# Inline form of the scopes-nothing case.
'

classify "unterminated-frontmatter" always '---
paths:
  - "scripts/release.sh"

# No closing fence, so Claude Code parses no paths out of it either.
'

# shellcheck disable=SC2016 # the backticked `paths:` is literal fixture prose, not an expansion
classify "paths-only-in-body" always '# Rules about rules

This file explains that the key is `paths:` and not `globs:`, at the start of
a line and everything:
paths:
  - "not-frontmatter/**"

A mention in the body is not frontmatter.
'

classify "other-key-closes-paths" scoped '---
paths:
  - "docs/**"
description: A rule whose paths list is followed by another key.
---

# Docs
'

classify "commented-pattern" scoped '---
paths:
  - "Makefile" # the dev targets
---

# Makefile
'

# ---------------------------------------------------------------------------
# The CLI, against throwaway repos
# ---------------------------------------------------------------------------
echo
echo "check-context-budget.sh:"

# Build a repo with a copy of the gate, then patch the ceiling and the
# expected-membership list inside that copy. Patching the copy rather than
# exporting an override is deliberate: the real script reads plain constants,
# and a test that needed an env hook would be testing a seam that only exists
# for the test.
#
# make_repo <dir> <ceiling> <expected...>
make_repo() {
    local dir="$1" ceiling="$2"
    shift 2
    mkdir -p "$dir/scripts/lib" "$dir/.claude/rules"
    cp "$CLI" "$dir/scripts/check-context-budget.sh"
    cp "$SCRIPT_DIR/context_budget.sh" "$dir/scripts/lib/context_budget.sh"
    chmod +x "$dir/scripts/check-context-budget.sh"

    local list=""
    local want
    for want in "$@"; do list="$list    \"$want\"\n"; done

    # Replace the ceiling and rewrite the array body between its own delimiters.
    awk -v ceiling="$ceiling" -v list="$list" '
        /^CONTEXT_BUDGET_CEILING=/ { print "CONTEXT_BUDGET_CEILING=" ceiling; next }
        /^CONTEXT_BUDGET_EXPECTED_ALWAYS=\(/ {
            print "CONTEXT_BUDGET_EXPECTED_ALWAYS=("
            printf "%s", list
            skipping = 1
            next
        }
        skipping && /^\)/ { print ")"; skipping = 0; next }
        skipping { next }
        { print }
    ' "$SCRIPT_DIR/context_budget.sh" >"$dir/scripts/lib/context_budget.sh"

    git -C "$dir" init -q -b main
    git -C "$dir" config user.email "t@t"
    git -C "$dir" config user.name "t"
}

commit_all() {
    git -C "$1" add -A
    git -C "$1" commit -qm "fixture"
}

# run_cli <dir> [args...] -> sets CLI_OUT and CLI_RC in the CALLER.
# Not a command substitution at the call site: that runs the function in a
# subshell, and the exit status assigned there never reaches the caller.
CLI_OUT=""
CLI_RC=0
run_cli() {
    local dir="$1"
    shift
    CLI_OUT="$(cd "$dir" && ./scripts/check-context-budget.sh "$@" 2>&1)"
    CLI_RC=$?
}

# 1. Under ceiling, membership exact: clean.
R="$TMP/clean"
make_repo "$R" 1000 "CLAUDE.md" ".claude/rules/always.md"
printf 'short root instructions\n' >"$R/CLAUDE.md"
printf '# Always\n\nresident on purpose\n' >"$R/.claude/rules/always.md"
printf -- '---\npaths:\n  - "src/**"\n---\n\n# Scoped\n' >"$R/.claude/rules/scoped.md"
commit_all "$R"
run_cli "$R"
OUT="$CLI_OUT"
if [ "$CLI_RC" -eq 0 ]; then
    pass "clean tree exits 0"
else
    fail "clean tree exited $CLI_RC: $OUT"
fi
case "$OUT" in
    *"scoped.md"*) fail "a path-scoped rule was counted as resident" ;;
    *) pass "a path-scoped rule is not counted" ;;
esac

# 2. Over ceiling: blocked, and the message names the overshoot.
R="$TMP/over"
make_repo "$R" 40 "CLAUDE.md"
printf 'x%.0s' $(seq 1 200) >"$R/CLAUDE.md"
commit_all "$R"
run_cli "$R"
OUT="$CLI_OUT"
if [ "$CLI_RC" -ne 0 ]; then
    pass "over ceiling exits non-zero"
else
    fail "over ceiling exited 0"
fi
case "$OUT" in
    *"over its 40 ceiling"*) pass "over-ceiling message names the ceiling" ;;
    *) fail "over-ceiling message missing: $OUT" ;;
esac

# 3. An undeclared resident rule: blocked even though the total is tiny. This
#    is the globs-regression arm, and the one the size arm cannot catch.
R="$TMP/undeclared"
make_repo "$R" 100000 "CLAUDE.md"
printf 'root\n' >"$R/CLAUDE.md"
printf -- '---\nglobs:\n  - "src/**"\n---\n\n# Meant to be scoped\n' >"$R/.claude/rules/leaky.md"
commit_all "$R"
run_cli "$R"
OUT="$CLI_OUT"
if [ "$CLI_RC" -ne 0 ]; then
    pass "undeclared resident rule exits non-zero"
else
    fail "undeclared resident rule exited 0"
fi
case "$OUT" in
    *"leaky.md"*) pass "undeclared rule is named in the failure" ;;
    *) fail "undeclared rule not named: $OUT" ;;
esac
case "$OUT" in
    *"globs"*) pass "failure explains the globs trap" ;;
    *) fail "failure does not mention globs: $OUT" ;;
esac

# 4. A declared file that is no longer resident: also blocked, so the expected
#    list cannot quietly go stale.
R="$TMP/missing"
make_repo "$R" 100000 "CLAUDE.md" ".claude/rules/retired.md"
printf 'root\n' >"$R/CLAUDE.md"
commit_all "$R"
run_cli "$R"
OUT="$CLI_OUT"
if [ "$CLI_RC" -ne 0 ]; then
    pass "vanished declared file exits non-zero"
else
    fail "vanished declared file exited 0"
fi
case "$OUT" in
    *"retired.md"*) pass "vanished declared file is named" ;;
    *) fail "vanished declared file not named: $OUT" ;;
esac

# 5. A rule that becomes scoped is caught by the same arm: the declared entry
#    stops being resident, which is a real change and must be acknowledged.
R="$TMP/newly-scoped"
make_repo "$R" 100000 "CLAUDE.md" ".claude/rules/was-always.md"
printf 'root\n' >"$R/CLAUDE.md"
printf -- '---\npaths:\n  - "src/**"\n---\n\n# Now scoped\n' >"$R/.claude/rules/was-always.md"
commit_all "$R"
run_cli "$R"
OUT="$CLI_OUT"
if [ "$CLI_RC" -ne 0 ]; then
    pass "newly scoped declared file exits non-zero"
else
    fail "newly scoped declared file exited 0"
fi

# 6. --report never fails, even over ceiling.
R="$TMP/report"
make_repo "$R" 10 "CLAUDE.md"
printf 'x%.0s' $(seq 1 500) >"$R/CLAUDE.md"
commit_all "$R"
run_cli "$R" --report
OUT="$CLI_OUT"
if [ "$CLI_RC" -eq 0 ]; then
    pass "--report exits 0 while over ceiling"
else
    fail "--report exited $CLI_RC"
fi
case "$OUT" in
    *TOTAL*) pass "--report prints a total" ;;
    *) fail "--report printed no total: $OUT" ;;
esac

# 7. A tree with NO matching files scans cleanly and says so through the
#    caller's own message, rather than dying inside the library.
#
#    This is the bash-3.2 regression guard. `bash` on macOS is the system
#    3.2.57, where expanding an empty array as "${arr[@]}" under `set -u` is an
#    `unbound variable` abort (fixed in 4.4). The scan used to collect
#    candidates into an array, so the empty-tree case, which is precisely the
#    discovery-is-broken case the gate has an actionable message for, aborted
#    one line before that message could print. It still exited non-zero, so the
#    gate stayed fail-closed, but with the wrong diagnosis.
EMPTY="$TMP/empty-tree"
make_repo "$EMPTY" 100000 "CLAUDE.md"
rm -f "$EMPTY/CLAUDE.md"
printf 'not an instruction file\n' >"$EMPTY/other.txt"
commit_all "$EMPTY"
SCAN_OUT="$(cd "$EMPTY" && bash -c 'set -uo pipefail; source scripts/lib/context_budget.sh; context_budget_scan "$PWD"' 2>&1)"
case "$SCAN_OUT" in
    *"unbound variable"*) fail "context_budget_scan aborts on an empty candidate list: $SCAN_OUT" ;;
    "") pass "empty tree scans to empty output, no bash abort" ;;
    *) fail "empty tree produced unexpected output: $SCAN_OUT" ;;
esac
run_cli "$EMPTY"
OUT="$CLI_OUT"
if [ "$CLI_RC" -ne 0 ]; then
    pass "empty tree exits non-zero"
else
    fail "empty tree exited 0"
fi
case "$OUT" in
    *"discovery is broken"*) pass "empty tree gets the actionable discovery message" ;;
    *) fail "empty tree did not name broken discovery: $OUT" ;;
esac

# 8. Outside a git checkout the gate refuses rather than reporting clean. The
#    fail-closed arm: a gate that cannot run must never read as a pass.
NOGIT="$TMP/nogit"
mkdir -p "$NOGIT/scripts/lib"
cp "$CLI" "$NOGIT/scripts/check-context-budget.sh"
cp "$SCRIPT_DIR/context_budget.sh" "$NOGIT/scripts/lib/context_budget.sh"
chmod +x "$NOGIT/scripts/check-context-budget.sh"
OUT="$(cd "$NOGIT" && GIT_CEILING_DIRECTORIES="$TMP" ./scripts/check-context-budget.sh 2>&1)"
RC=$?
if [ "$RC" -ne 0 ]; then
    pass "outside a git checkout exits non-zero"
else
    fail "outside a git checkout exited 0: $OUT"
fi

# 9. An unknown flag is an error, not a silent pass.
run_cli "$TMP/clean" --nope
OUT="$CLI_OUT"
if [ "$CLI_RC" -ne 0 ]; then
    pass "unknown flag exits non-zero"
else
    fail "unknown flag exited 0"
fi

# 10. The real tree passes its own gate. Not hermetic on purpose: this is the
#    one assertion that has to move when the repo does.
echo
echo "the real tree:"
if (cd "$SCRIPT_DIR/../.." && ./scripts/check-context-budget.sh >/dev/null 2>&1); then
    pass "this repo is within its own context budget"
else
    fail "this repo is OVER its own context budget (run ./scripts/check-context-budget.sh)"
fi

echo
echo "passed $PASS, failed $FAIL"
[ "$FAIL" -eq 0 ]
