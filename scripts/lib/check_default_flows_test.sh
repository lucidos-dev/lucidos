#!/usr/bin/env bash
# Tests for scripts/check-default-flows.sh, the /harden gate that keeps each
# copy of a default coding-agent flow identical to its knowhow (ADR 0505).
#
# Each case builds a throwaway git repo with both defaults and Lucidos's two
# required copies, changes one thing, and runs the check against it. The real
# tree must pass too.
#
# Hermetic: the check only reads tracked files, and every fixture lives under
# mktemp.
#
# Run: ./scripts/lib/check_default_flows_test.sh
set -u

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
CHECK="$SCRIPT_DIR/../check-default-flows.sh"

PASS=0
FAIL=0
pass() { echo "  ok:   $*"; PASS=$((PASS + 1)); }
fail() { echo "  FAIL: $*"; FAIL=$((FAIL + 1)); }

SANDBOX="$(mktemp -d)"
trap 'rm -rf "$SANDBOX"' EXIT

PLAN_SKILL=".claude/skills/implementation-plan/SKILL.md"
HARDEN=".claude/commands/harden.md"
FLOWS="system-knowhow/coding-agent-flows"

# flow_block <name> <body>: a marked block, as a default and its copies carry it.
# Fixtures build every marker with printf: a raw one at the start of a line
# here would make this file a copy the check reads.
flow_block() {
    printf '<!-- default-flow: %s -->\n%s\n<!-- /default-flow: %s -->' "$1" "$2" "$1"
}

PLAN_BLOCK="$(flow_block implementation-plan 'Run the plan-approval checkpoint.')"
HARDEN_BLOCK="$(flow_block hardening 'Run the pre-harden checkpoint.')"

# put <repo> <path> <content>: write one file.
put() {
    mkdir -p "$(dirname "$1/$2")"
    printf '%s\n' "$3" >"$1/$2"
}

commit() { # <repo>
    git -C "$1" add -A
    git -C "$1" -c user.email=dev@example.com -c user.name=Dev commit -qm fixture
}

new_fixture() { # -> prints the repo path
    local repo
    repo="$(mktemp -d "$SANDBOX/repo.XXXXXX")"
    git init -q "$repo"
    put "$repo" "$FLOWS/implementation-plan.md" "# Default flow
$PLAN_BLOCK"
    put "$repo" "$FLOWS/hardening.md" "# Default flow
$HARDEN_BLOCK
After the block, the repository's own steps."
    put "$repo" "$PLAN_SKILL" "# Plan skill
$PLAN_BLOCK
Lucidos's own steps."
    put "$repo" "$HARDEN" "# Harden
$HARDEN_BLOCK"
    put "$repo" docs/notes.md 'Prose may name the marker inline: <!-- default-flow: x --> is fine.'
    commit "$repo"
    printf '%s' "$repo"
}

# expect <label> <want: pass|fail> <repo> [text-the-failure-must-name]
expect() {
    local label="$1" want="$2" repo="$3" text="${4:-}" out rc
    out="$("$CHECK" "$repo" 2>&1)" && rc=0 || rc=$?
    if [ "$want" = pass ] && [ "$rc" -eq 0 ]; then
        pass "$label"
    elif [ "$want" = fail ] && [ "$rc" -ne 0 ] && [[ "$out" == *"$text"* ]]; then
        pass "$label"
    else
        fail "$label (rc=$rc): $out"
    fi
}

# expect_change <label> <path> <content> <text>: a fixture with one file
# rewritten fails, naming <text>.
expect_change() {
    local repo
    repo="$(new_fixture)"
    put "$repo" "$2" "$3"
    commit "$repo"
    expect "$1" fail "$repo" "$4"
}

echo "test: matching copies pass"
expect "both required copies match, and inline prose is ignored" pass "$(new_fixture)"

repo="$(new_fixture)"
put "$repo" AGENTS.md "# A third copy
$PLAN_BLOCK"
commit "$repo"
expect "an extra matching copy passes" pass "$repo"

echo "test: a drifted copy fails"
expect_change "a reworded copy fails, naming the file" "$PLAN_SKILL" "# Plan skill
$(flow_block implementation-plan 'Run the plan checkpoint.')" "$PLAN_SKILL: its 'implementation-plan' block differs"

expect_change "a copy that never closes fails" "$HARDEN" "# Harden
$(printf '<!-- default-flow: %s -->' hardening)
Run the pre-harden checkpoint." "$HARDEN: opens 'hardening' twice, or never closes it"

expect_change "a copy opened twice fails" "$HARDEN" "# Harden
$HARDEN_BLOCK
$HARDEN_BLOCK" "$HARDEN: opens 'hardening' twice"

expect_change "a copy naming an unknown flow fails" AGENTS.md "# Copy
$(flow_block review 'Review it.')" "names default flow 'review'"

echo "test: a missing copy or default fails"
expect_change "a required copy without its block fails" "$HARDEN" "# Harden, no block" \
    "$HARDEN: lacks its copy of the 'hardening' default flow"

expect_change "a default without its block fails" "$FLOWS/hardening.md" "# Default flow, no block" \
    "$FLOWS/hardening.md: carries no single closed block"

expect_change "a default no copy carries fails" "$FLOWS/review.md" "# Default flow
$(flow_block review 'Review it.')" "$FLOWS/review.md: ships default flow 'review', which no copy carries"

echo "test: git colour config cannot blind the check"
repo="$(new_fixture)"
put "$repo" "$PLAN_SKILL" "# Plan skill
$(flow_block implementation-plan 'Run the plan checkpoint.')"
commit "$repo"
git -C "$repo" config color.ui always
git -C "$repo" config color.grep always
expect "a drifted copy fails under forced colour" fail "$repo" "$PLAN_SKILL: its 'implementation-plan' block differs"

echo "test: the real tree passes"
expect "the repository's own copies match" pass "$(git -C "$SCRIPT_DIR" rev-parse --show-toplevel)"

echo ""
echo "passed: $PASS, failed: $FAIL"
[ "$FAIL" -eq 0 ]
