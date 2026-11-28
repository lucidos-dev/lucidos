#!/bin/bash
# Tests for the touched-spec selector: scripts/lib/touched_specs.sh.
#
# Hermetic. Each case builds a throwaway git repo shaped like the app's src/ and
# e2e/ directories, so nothing reads this checkout. The three "replay" cases
# mirror the three landings behind the main check's "spec missed a UI rename"
# reds (the Home rail, the dashboard rail, the Canvas title).
#
# Run: ./scripts/lib/touched_specs_test.sh
set -u

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
SANDBOX="$(mktemp -d)"
trap 'rm -rf "$SANDBOX"' EXIT

PASS=0
FAIL=0
pass() { echo "  ok:   $*"; PASS=$((PASS + 1)); }
fail() { echo "  FAIL: $*"; FAIL=$((FAIL + 1)); }

# shellcheck source=scripts/lib/touched_specs.sh
source "$SCRIPT_DIR/touched_specs.sh"

APP=crates/lucidos-app
REPO="$SANDBOX/repo"

# new_repo: a repo with one commit holding a small app and four specs.
new_repo() {
    rm -rf "$REPO"
    mkdir -p "$REPO/$APP/src/components" "$REPO/$APP/e2e"
    git -C "$REPO" init -q -b main
    git -C "$REPO" config user.email t@example.com
    git -C "$REPO" config user.name t
    cat >"$REPO/$APP/src/components/Rail.tsx" <<'EOF'
export function Rail() {
  return <button aria-label="Hide home rail" data-testid="home-rail-toggle" className="rail-toggle docked" />;
}
EOF
    cat >"$REPO/$APP/src/components/Title.tsx" <<'EOF'
export const Title = () => <h1 data-testid="canvas-title">Canvas</h1>;
EOF
    cat >"$REPO/$APP/e2e/helpers.ts" <<'EOF'
export async function hideHomeRail(page) {
  await page.getByLabel('Hide home rail').click();
}
export async function hideEverything(page) {
  await hideHomeRail(page);
}
export async function gotoHome(page) {
  await page.goto('/');
}
EOF
    cat >"$REPO/$APP/e2e/rail.spec.ts" <<'EOF'
import { hideHomeRail } from './helpers';
test('rail', async ({ page }) => { await hideHomeRail(page); });
EOF
    cat >"$REPO/$APP/e2e/everything.spec.ts" <<'EOF'
import { hideEverything } from './helpers';
test('everything', async ({ page }) => { await hideEverything(page); });
EOF
    cat >"$REPO/$APP/e2e/title.spec.ts" <<'EOF'
test('title', async ({ page }) => { await page.getByTestId('canvas-title').click(); });
EOF
    cat >"$REPO/$APP/e2e/plain.spec.ts" <<'EOF'
import { gotoHome } from './helpers';
test('plain', async ({ page }) => { await gotoHome(page); });
EOF
    git -C "$REPO" add -A
    git -C "$REPO" commit -q -m base
}

# selected [<coverage-map>]: what the selector says for the working tree against
# the first commit. Its stderr note lands in $SANDBOX/note.
selected() {
    local base paths="$SANDBOX/paths"
    base="$(git -C "$REPO" rev-list --max-parents=0 HEAD)"
    { git -C "$REPO" diff --no-renames --name-only "$base" --; git -C "$REPO" ls-files -o --exclude-standard; } >"$paths"
    ts_touched_specs "$REPO" "$base" "$paths" "${1:-}" 2>"$SANDBOX/note" | paste -sd' ' -
}

expect_with_map() { # <label> <map-file> <expected> <note-fragment>
    local got
    got="$(selected "$2")"
    if [ "$got" = "$3" ]; then pass "$1 → ${got:-nothing}"; else fail "$1: expected '$3', got '$got'"; fi
    if grep -qF "$4" "$SANDBOX/note"; then pass "$1 says '$4'"; else fail "$1: the note lacks '$4': $(cat "$SANDBOX/note")"; fi
}

expect() { # <label> <expected>
    local got
    got="$(selected)"
    if [ "$got" = "$2" ]; then pass "$1 → ${got:-nothing}"; else fail "$1: expected '$2', got '$got'"; fi
}

test_a_changed_spec_is_selected() {
    echo "test: a changed spec is always selected"
    new_repo
    echo '// touched' >>"$REPO/$APP/e2e/plain.spec.ts"
    expect "one changed spec" "plain.spec.ts"
    echo 'test("new", () => {});' >"$REPO/$APP/e2e/added.spec.ts"
    expect "plus an untracked spec" "added.spec.ts plain.spec.ts"
    git -C "$REPO" rm -q -f "$REPO/$APP/e2e/plain.spec.ts"
    expect "a deleted spec is not selected" "added.spec.ts"
}

test_a_diff_with_no_frontend_change_selects_nothing() {
    echo "test: a diff with no src or spec change selects no spec"
    new_repo
    mkdir -p "$REPO/crates/lucidos-engine/src"
    echo 'fn main() {}' >"$REPO/crates/lucidos-engine/src/main.rs"
    echo 'rename' >>"$REPO/$APP/e2e/helpers.ts"
    expect "a backend file and a helper edit" ""
}

test_a_vanished_literal_selects_the_spec_that_names_it() {
    echo "test: a removed literal that vanished selects the spec naming it"
    new_repo
    sed -i.bak 's/canvas-title/board-title/' "$REPO/$APP/src/components/Title.tsx"
    rm -f "$REPO/$APP/src/components/Title.tsx.bak"
    expect "a testid renamed in src" "title.spec.ts"
}

test_a_literal_that_survives_selects_nothing() {
    echo "test: a literal that still appears in src is not vanished"
    new_repo
    cat >>"$REPO/$APP/src/components/Title.tsx" <<'EOF'
export const Alias = () => <h1 data-testid="canvas-title">Moved</h1>;
EOF
    sed -i.bak 's/>Canvas</>Board</' "$REPO/$APP/src/components/Title.tsx"
    rm -f "$REPO/$APP/src/components/Title.tsx.bak"
    expect "the testid moved, not removed" ""
}

test_a_helper_selects_the_specs_that_call_it() {
    echo "test: a vanished literal in a helper selects its callers, not every spec"
    new_repo
    sed -i.bak 's/Hide home rail/Hide dashboard rail/' "$REPO/$APP/src/components/Rail.tsx"
    rm -f "$REPO/$APP/src/components/Rail.tsx.bak"
    expect "the helper's label renamed" "everything.spec.ts rail.spec.ts"
}

test_a_class_list_counts_word_by_word() {
    echo "test: one name leaving a class list is a vanished literal"
    new_repo
    printf 'test("toggle", async ({ page }) => { await page.locator(".rail-toggle").click(); });\n' \
        >"$REPO/$APP/e2e/toggle.spec.ts"
    git -C "$REPO" add -A && git -C "$REPO" commit -q -m spec
    sed -i.bak 's/"rail-toggle docked"/"rail-button docked"/' "$REPO/$APP/src/components/Rail.tsx"
    rm -f "$REPO/$APP/src/components/Rail.tsx.bak"
    expect "rail-toggle renamed" "toggle.spec.ts"
}

# The three landings behind the "spec missed a UI rename" reds. Each replays the
# shape of its UI change and expects the spec that broke.
test_replay_the_home_rail_rename() {
    echo "test: replay, the Home rail label moved under a helper"
    new_repo
    sed -i.bak 's/Hide home rail/Hide the Home rail/' "$REPO/$APP/src/components/Rail.tsx"
    rm -f "$REPO/$APP/src/components/Rail.tsx.bak"
    expect "specs reach the label through hideHomeRail" "everything.spec.ts rail.spec.ts"
}

test_replay_the_dashboard_rail_rename() {
    echo "test: replay, the testid renamed with the rail"
    new_repo
    sed -i.bak 's/home-rail-toggle/dashboard-rail-toggle/' "$REPO/$APP/src/components/Rail.tsx"
    rm -f "$REPO/$APP/src/components/Rail.tsx.bak"
    printf 'test("t", async ({ page }) => { await page.getByTestId("home-rail-toggle").click(); });\n' \
        >"$REPO/$APP/e2e/rail-testid.spec.ts"
    git -C "$REPO" add -A && git -C "$REPO" commit -q -m spec
    expect "the spec that names the old testid" "rail-testid.spec.ts"
}

test_replay_the_canvas_title_rename() {
    echo "test: replay, the Canvas title testid"
    new_repo
    sed -i.bak 's/canvas-title/board-title/; s/>Canvas</>Board</' "$REPO/$APP/src/components/Title.tsx"
    rm -f "$REPO/$APP/src/components/Title.tsx.bak"
    expect "the title spec" "title.spec.ts"
}

test_the_coverage_map_adds_the_specs_that_ran_a_changed_file() {
    echo "test: the coverage map adds every spec whose run met a changed file"
    new_repo
    local map="$SANDBOX/map.json"
    printf '{"commit": "abc123", "specs": {"plain.spec.ts": ["%s"], "title.spec.ts": ["%s"], "gone.spec.ts": ["%s"]}}\n' \
        "$APP/src/components/Rail.tsx" "$APP/src/components/Title.tsx" "$APP/src/components/Rail.tsx" >"$map"
    echo '// a comment, no literal leaves' >>"$REPO/$APP/src/components/Rail.tsx"
    expect_with_map "a comment in Rail.tsx" "$map" "plain.spec.ts" "from abc123, 1 spec(s)"
    sed -i.bak 's/Hide home rail/Hide dashboard rail/' "$REPO/$APP/src/components/Rail.tsx"
    rm -f "$REPO/$APP/src/components/Rail.tsx.bak"
    expect_with_map "it only adds to the literal rule" "$map" \
        "everything.spec.ts plain.spec.ts rail.spec.ts" "1 spec(s) meet the diff"
}

test_a_file_nearly_every_spec_runs_selects_nothing() {
    echo "test: a startup file that nearly every spec runs selects no spec by coverage"
    new_repo
    local map="$SANDBOX/startup-map.json" boot="$APP/src/boot.ts"
    printf '{"specs": {"plain.spec.ts": ["%s", "%s"], "title.spec.ts": ["%s"], "rail.spec.ts": ["%s"]}}\n' \
        "$boot" "$APP/src/components/Rail.tsx" "$boot" "$boot" >"$map"
    echo 'export const boot = 1;' >"$REPO/$boot"
    expect_with_map "a change to the startup file" "$map" "" "1 changed startup file(s) ignored"
    echo '// a comment' >>"$REPO/$APP/src/components/Rail.tsx"
    expect_with_map "beside a file one spec runs" "$map" "plain.spec.ts" "1 spec(s) meet the diff"
}

test_a_missing_or_corrupt_map_falls_back_and_says_so() {
    echo "test: a missing or corrupt map falls back to today's rule, and says so"
    new_repo
    sed -i.bak 's/canvas-title/board-title/' "$REPO/$APP/src/components/Title.tsx"
    rm -f "$REPO/$APP/src/components/Title.tsx.bak"
    expect_with_map "no map" "$SANDBOX/absent.json" "title.spec.ts" "none yet"
    echo '{"specs": [' >"$SANDBOX/corrupt.json"
    expect_with_map "a truncated map" "$SANDBOX/corrupt.json" "title.spec.ts" "unreadable"
    echo '{"specs": ["plain.spec.ts"]}' >"$SANDBOX/wrong.json"
    expect_with_map "a map of the wrong shape" "$SANDBOX/wrong.json" "title.spec.ts" "unreadable"
    echo '{"specs": {"plain.spec.ts": [["nested"]]}}' >"$SANDBOX/nested.json"
    expect_with_map "a map listing lists" "$SANDBOX/nested.json" "title.spec.ts" "unreadable"
}

test_the_map_lives_in_the_git_common_dir() {
    echo "test: every worktree reads the one map, in the git common dir"
    new_repo
    git -C "$REPO" worktree add -q "$SANDBOX/wt" -b side
    local main wt
    main="$(ts_coverage_map_path "$REPO")"
    wt="$(ts_coverage_map_path "$SANDBOX/wt")"
    if [ "$main" = "$wt" ] && [ "$main" = "$(cd "$REPO/.git" && pwd -P)/lucidos/e2e-coverage-map.json" ]; then
        pass "both name $main"
    else
        fail "the checkout names '$main', the worktree '$wt'"
    fi
    git -C "$REPO" worktree remove --force "$SANDBOX/wt"
}

test_a_changed_spec_is_selected
test_the_coverage_map_adds_the_specs_that_ran_a_changed_file
test_a_file_nearly_every_spec_runs_selects_nothing
test_a_missing_or_corrupt_map_falls_back_and_says_so
test_the_map_lives_in_the_git_common_dir
test_a_diff_with_no_frontend_change_selects_nothing
test_a_vanished_literal_selects_the_spec_that_names_it
test_a_literal_that_survives_selects_nothing
test_a_helper_selects_the_specs_that_call_it
test_a_class_list_counts_word_by_word
test_replay_the_home_rail_rename
test_replay_the_dashboard_rail_rename
test_replay_the_canvas_title_rename

echo ""
echo "Passed: $PASS  Failed: $FAIL"
[ "$FAIL" -eq 0 ]
