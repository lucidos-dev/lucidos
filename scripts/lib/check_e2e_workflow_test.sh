#!/bin/bash
# Tests for scripts/check-e2e-workflow.sh: the real e2e.yml keeps its limits,
# and a copy that breaks any one of them is refused (ADRs 0382, 0464).
#
# Hermetic: every case checks a doctored copy in a sandbox workflows folder.
#
# Run: ./scripts/lib/check_e2e_workflow_test.sh
set -u

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
CHECK="$SCRIPT_DIR/../check-e2e-workflow.sh"
REAL="$SCRIPT_DIR/../../.github/workflows/e2e.yml"
SANDBOX="$(mktemp -d)"
trap 'rm -rf "$SANDBOX"' EXIT

PASS=0
FAIL=0
pass() { echo "  ok:   $*"; PASS=$((PASS + 1)); }
fail() { echo "  FAIL: $*"; FAIL=$((FAIL + 1)); }

# doctored <name> <python over `s`>: a copy of the real workflow, edited, alone
# in its own workflows folder. Prints its path.
doctored() {
    local dir="$SANDBOX/$1"
    mkdir -p "$dir"
    python3 - "$REAL" "$dir/e2e.yml" "$2" <<'PY'
import sys
src, dst, edit = sys.argv[1:4]
s = open(src).read()
before = s
exec(edit)
assert s != before, "the edit changed nothing"
open(dst, "w").write(s)
PY
    printf '%s\n' "$dir/e2e.yml"
}

# expect_refused <what> <expected message fragment> <workflow>
expect_refused() {
    local out
    if out="$("$CHECK" "$3" 2>&1)"; then
        fail "$1: accepted"
    elif [[ "$out" == *"$2"* ]]; then
        pass "$1"
    else
        fail "$1: refused for another reason: $out"
    fi
}

test_the_real_workflow_keeps_its_limits() {
    echo "test: the e2e workflow in the tree keeps every limit"
    local dir="$SANDBOX/real"
    mkdir -p "$dir"
    cp "$REAL" "$dir/e2e.yml"
    if "$CHECK" "$dir/e2e.yml" >/dev/null; then pass "accepted"; else fail "refused: $("$CHECK" "$dir/e2e.yml")"; fi
}

test_each_broken_limit_is_refused() {
    echo "test: a copy that breaks one limit is refused, naming it"
    # shellcheck disable=SC2016 # the planted ${{ }} is a GitHub expression, not shell
    expect_refused "a secret" "references a secret" "$(doctored secret \
        's = s.replace("CARGO_TERM_COLOR: never", "CARGO_TERM_COLOR: ${{ secrets.TOKEN }}")')"
    expect_refused "a write permission" "contents: read" "$(doctored write \
        's = s.replace("  contents: read", "  contents: write")')"
    expect_refused "a third trigger" "must trigger only" "$(doctored trigger \
        "s = s.replace(\"branches: ['e2e/**', 'main']\", \"branches: ['e2e/**', 'main', 'rc/**']\")")"
    expect_refused "the engine job saving a cache" "job engine may save a cache" "$(doctored engine-save \
        'i = s.index("  engine:"); j = s.index("save-if: false", i); s = s[:j] + "save-if: true" + s[j + 14:]')"
    expect_refused "the engine job running without the plan" "job engine does not need the plan job" "$(doctored engine-needs \
        'i = s.index("  engine:"); j = s.index("    needs: plan\n", i); s = s[:j] + s[j + 16:]')"
    expect_refused "the engine job running unasked" "only when the plan asks for it" "$(doctored engine-unasked \
        "s = s.replace(\"&& needs.plan.outputs.has_engine == 'true'\", '')")"
    expect_refused "a job on main pushes other than seed-cache" "seed-cache job must run only on main" "$(doctored seed-anywhere \
        "s = s.replace(\"&& github.ref == 'refs/heads/main'\", '')")"
    expect_refused "the plan on any branch" "plan job must run only on an e2e/ branch" "$(doctored plan-anywhere \
        "s = s.replace(\"&& startsWith(github.ref, 'refs/heads/e2e/')\", '')")"
    expect_refused "an action pinned by tag" "action not pinned by commit" "$(doctored tag-pin \
        's = s.replace("actions/checkout@11d5960a326750d5838078e36cf38b85af677262", "actions/checkout@v4", 1)')"
    expect_refused "an artifact kept longer" "retention-days: 1" "$(doctored retention \
        's = s.replace("retention-days: 1", "retention-days: 7", 1)')"
    expect_refused "webkit off macOS" "macos-15" "$(doctored webkit \
        's = s.replace("fromJSON(needs.plan.outputs.macos)", "fromJSON(needs.plan.outputs.linux)")')"
}

test_another_workflow_on_e2e_branches_is_refused() {
    echo "test: no other workflow may trigger on an e2e/ branch"
    local dir="$SANDBOX/sibling"
    mkdir -p "$dir"
    cp "$REAL" "$dir/e2e.yml"
    printf 'on:\n  push:\n    branches: [%se2e/**%s]\n' "'" "'" >"$dir/other.yml"
    expect_refused "a sibling workflow on e2e/**" "other.yml names an e2e/ branch" "$dir/e2e.yml"
}

test_the_real_workflow_keeps_its_limits
test_each_broken_limit_is_refused
test_another_workflow_on_e2e_branches_is_refused

echo ""
echo "Passed: $PASS  Failed: $FAIL"
[ "$FAIL" -eq 0 ]
