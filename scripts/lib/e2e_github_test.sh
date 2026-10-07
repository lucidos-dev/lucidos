#!/bin/bash
# Tests for e2e GitHub mode: scripts/lib/e2e_github.sh and the plan half of
# scripts/e2e-github-shard.sh (ADR 0382).
#
# Hermetic. A throwaway git repo, an invented denylist, and stubbed `git push`
# and `gh`, so nothing here reaches a remote, the network or a real token.
#
# Run: ./scripts/lib/e2e_github_test.sh
set -u

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
SANDBOX="$(mktemp -d)"
trap 'rm -rf "$SANDBOX"' EXIT

PASS=0
FAIL=0
pass() { echo "  ok:   $*"; PASS=$((PASS + 1)); }
fail() { echo "  FAIL: $*"; FAIL=$((FAIL + 1)); }

export E2E_WORKSPACE="$SANDBOX/e2e-test"
mkdir -p "$E2E_WORKSPACE/.lucidos/worktrees"
# The public mirror does not ship the release libraries, so the commit tests
# run only in the maintainer checkout.
HAVE_RELEASE_TREE=""
if [ -f "$SCRIPT_DIR/release_tree.sh" ]; then
    # shellcheck source=scripts/lib/release_tree.sh
    source "$SCRIPT_DIR/release_tree.sh"
    HAVE_RELEASE_TREE=1
fi
# shellcheck source=scripts/lib/e2e.sh
source "$SCRIPT_DIR/e2e.sh"
# shellcheck source=scripts/lib/e2e_github.sh
source "$SCRIPT_DIR/e2e_github.sh"

REPO="$SANDBOX/repo"
git init -q "$REPO"
git -C "$REPO" config user.email "dev@example.com"
git -C "$REPO" config user.name "A Developer"
mkdir -p "$REPO/docs/plans" "$REPO/src"
echo "plan notes" >"$REPO/docs/plans/a-plan.md"
echo "machine notes" >"$REPO/WORKSPACES.md"
echo "fn main() {}" >"$REPO/src/main.rs"
git -C "$REPO" add -A
git -C "$REPO" commit -qm "a private commit subject"

DENYLIST="$SANDBOX/denylist.md"
cat >"$DENYLIST" <<'FIXTURE'
<!-- BEGIN private-data-denylist -->
```
# an invented internal project name
widget-pipeline
```
<!-- END private-data-denylist -->

<!-- BEGIN private-data-exceptions -->
```
# no carve-outs
```
<!-- END private-data-exceptions -->
FIXTURE
export PRIVATE_DATA_DENYLIST_SOURCE="$DENYLIST"

REQUEST="$SANDBOX/request.json"
echo '{"run_id": "20261007-120000-ab12", "suites": ["wasm"]}' >"$REQUEST"

test_the_pushed_commit_is_stripped_parentless_and_generic() {
    echo "test: the commit GitHub mode pushes is stripped, parentless and carries no private identity"
    local commit
    commit="$(e2e_github_build_commit "$REPO" "$REQUEST" 20261007-120000-ab12 2>/dev/null)"
    if [ -z "$commit" ]; then
        fail "no commit was built"
        return
    fi
    if [ "$(git -C "$REPO" rev-list --parents -n1 "$commit")" = "$commit" ]; then
        pass "the commit has no parent"
    else
        fail "the commit has a parent: $(git -C "$REPO" rev-list --parents -n1 "$commit")"
    fi
    local ident
    ident="$(git -C "$REPO" show -s --format='%an <%ae> / %cn <%ce> / %s' "$commit")"
    if [ "$ident" = "Lucidos e2e <lucidos-e2e@users.noreply.github.com> / Lucidos e2e <lucidos-e2e@users.noreply.github.com> / e2e run 20261007-120000-ab12" ]; then
        pass "author, committer and message are generic"
    else
        fail "identity leaked: $ident"
    fi
    if git -C "$REPO" cat-file -e "$commit:docs/plans/a-plan.md" 2>/dev/null; then
        fail "docs/plans survived the strip"
    else
        pass "docs/plans is stripped"
    fi
    if [ "$(git -C "$REPO" show "$commit:WORKSPACES.md")" = "$(release_tree_workspaces_stub)" ]; then
        pass "WORKSPACES.md is the public stub"
    else
        fail "WORKSPACES.md was not stubbed"
    fi
    if [ "$(git -C "$REPO" show "$commit:.github/e2e-request.json")" = "$(cat "$REQUEST")" ]; then
        pass "the request rides in the tree"
    else
        fail "the request is missing from the tree"
    fi
}

test_the_scan_refuses_a_planted_token() {
    echo "test: a denylisted token anywhere in the tree, or in the request, stops the commit"
    echo "see widget-pipeline" >"$REPO/src/leak.txt"
    git -C "$REPO" add -A && git -C "$REPO" commit -qm "plant"
    local out
    out="$(e2e_github_build_commit "$REPO" "$REQUEST" 20261007-120000-ab12 2>/dev/null)"
    if [ -z "$out" ]; then pass "a planted token in the tree refuses"; else fail "built $out over a planted token"; fi
    git -C "$REPO" rm -q src/leak.txt && git -C "$REPO" commit -qm "unplant"

    local leaky="$SANDBOX/leaky-request.json"
    echo '{"run_id": "x", "api_args": ["widget-pipeline"]}' >"$leaky"
    out="$(e2e_github_build_commit "$REPO" "$leaky" 20261007-120000-ab12 2>/dev/null)"
    if [ -z "$out" ]; then pass "a planted token in the request refuses"; else fail "built $out over a leaky request"; fi
}

test_an_unloadable_denylist_refuses() {
    echo "test: a denylist that will not load refuses rather than reading clean"
    local broken="$SANDBOX/broken.md" out
    echo "no markers here" >"$broken"
    out="$(PRIVATE_DATA_DENYLIST_SOURCE="$broken" e2e_github_build_commit "$REPO" "$REQUEST" 20261007-120000-ab12 2>/dev/null)"
    if [ -z "$out" ]; then pass "an unloadable denylist refuses"; else fail "built $out with no denylist"; fi
}

test_only_run_branches_qualify() {
    echo "test: only e2e/<run-id> names an e2e run branch"
    local b ok=1
    for b in e2e/20261007-120000-ab12 e2e/20261007-120000-0f; do
        e2e_github_is_run_branch "$b" || { ok=""; fail "rejected $b"; }
    done
    for b in main rc/0.30.0 e2e/feature e2e/ refs/heads/e2e/20261007-120000-ab12 e2e/20261007-120000-ab12/x v1.0.0; do
        if e2e_github_is_run_branch "$b"; then ok=""; fail "accepted $b"; fi
    done
    [ -z "$ok" ] || pass "e2e run branches accepted, every other ref refused"
}

# shellcheck disable=SC2317 # the git stub is called by the function under test
test_delete_refuses_anything_but_a_run_branch() {
    echo "test: e2e_github_delete_branch never deletes a ref outside e2e/"
    local calls="$SANDBOX/git-calls"
    : >"$calls"
    (
        # shellcheck disable=SC2032 # a subshell stub, never reached through xargs
        git() { echo "git $*" >>"$calls"; }
        e2e_github_delete_branch "$REPO" main 2>/dev/null
        e2e_github_delete_branch "$REPO" rc/0.30.0 2>/dev/null
        e2e_github_delete_branch "$REPO" e2e/20261007-120000-ab12
    )
    if [ "$(grep -c . "$calls")" = 1 ] && grep -q -- "--delete refs/heads/e2e/20261007-120000-ab12" "$calls"; then
        pass "only the e2e run branch reached git"
    else
        fail "git calls were: $(cat "$calls")"
    fi
}

# shellcheck disable=SC2317 # the stubs are called by e2e_github_sweep
test_the_sweep_removes_only_old_finished_run_branches() {
    echo "test: the sweep deletes e2e run branches over a day old whose runs finished, and nothing else"
    local deletes="$SANDBOX/deletes"
    : >"$deletes"
    (
        e2e_github_now() { e2e_github_branch_started e2e/20261008-120000-ffff; }
        git() {
            case "$*" in
                *ls-remote*)
                    printf 'a\trefs/heads/main\n'
                    printf 'b\trefs/heads/rc/0.30.0\n'
                    printf 'c\trefs/heads/e2e/20261006-090000-aaaa\n'
                    printf 'd\trefs/heads/e2e/20261006-100000-bbbb\n'
                    printf 'e\trefs/heads/e2e/20261008-110000-cccc\n'
                    printf 'f\trefs/heads/e2e/not-a-run\n'
                    ;;
                *--delete*) echo "$*" >>"$deletes" ;;
            esac
        }
        gh() {
            case "$*" in
                *20261006-090000-aaaa*) echo 0 ;;
                *20261006-100000-bbbb*) echo 1 ;;
                *) echo "" ;;
            esac
        }
        e2e_github_sweep "$REPO" >/dev/null
    )
    if [ "$(grep -c . "$deletes")" = 1 ] && grep -q "e2e/20261006-090000-aaaa" "$deletes"; then
        pass "only the old, finished e2e run branch was deleted"
    else
        fail "deletes were: $(cat "$deletes")"
    fi
}

# shellcheck disable=SC2317 # exec is shadowed to observe the hand-off
test_the_handoff_strips_github_and_ignores_it_after_dashes() {
    echo "test: e2e_github_handoff hands over only when --github comes before --"
    local out
    out="$(exec() { echo "exec $*"; }; e2e_github_handoff browser -f chat.spec.ts --github -- --grep x; echo none)"
    case "$out" in
        *"e2e-github.sh browser -f chat.spec.ts -- --grep x"*) pass "--github hands over, minus the flag" ;;
        *) fail "hand-off printed: $out" ;;
    esac
    out="$(exec() { echo "exec $*"; }; e2e_github_handoff browser -- --github; echo none)"
    if [ "$out" = none ]; then pass "--github after -- belongs to Playwright"; else fail "handed over on: $out"; fi
}

# A fake download: the plan, then one directory per shard.
fake_shard() { # <dir> <id> <rc|-> <log text>
    mkdir -p "$1/e2e-out-$2"
    printf '%s\n' "$4" >"$1/e2e-out-$2/$2.log"
    [ "$3" = - ] || echo "$3" >"$1/e2e-out-$2/$2.rc"
}

PW_PASS_LOG="Running 3 tests using 1 worker
  3 passed (10s)"
PW_FAIL_LOG="Running 3 tests using 1 worker
  1 failed
  2 passed (10s)"

test_the_report_matches_the_local_tally_and_exit_codes() {
    echo "test: e2e_github_report tallies like a local run and returns the first failing phase"
    local d="$SANDBOX/report-green" out rc
    mkdir -p "$d/e2e-plan"
    printf '%s\n' "api api - 1 1" "chromium-1-2 browser chromium 1 2" "chromium-2-2 browser chromium 2 2" >"$d/e2e-plan/entries"
    fake_shard "$d" api 0 "test result: ok. 12 passed"
    fake_shard "$d" chromium-1-2 0 "$PW_PASS_LOG"
    fake_shard "$d" chromium-2-2 0 "$PW_PASS_LOG"
    out="$(e2e_github_report "$d" 2>&1)"; rc=$?
    if [ "$rc" = 0 ] && [[ "$out" == *"── chromium total: 6 tests over 2 invocation(s) ──"* ]]; then
        pass "a green run adds the shards up and exits 0"
    else
        fail "green run rc=$rc: $out"
    fi

    d="$SANDBOX/report-red"
    mkdir -p "$d/e2e-plan"
    printf '%s\n' "api api - 1 1" "chromium-1-1 browser chromium 1 1" >"$d/e2e-plan/entries"
    fake_shard "$d" api 0 "test result: ok"
    fake_shard "$d" chromium-1-1 1 "$PW_FAIL_LOG"
    e2e_github_report "$d" >/dev/null 2>&1; rc=$?
    if [ "$rc" = 1 ]; then pass "a failed shard fails the run with its code"; else fail "red run rc=$rc"; fi

    d="$SANDBOX/report-missing"
    mkdir -p "$d/e2e-plan"
    printf '%s\n' "wasm wasm - 1 1" "mobile-webkit-1-2 browser mobile-webkit 1 2" "mobile-webkit-2-2 browser mobile-webkit 2 2" >"$d/e2e-plan/entries"
    fake_shard "$d" wasm 0 "test result: ok"
    fake_shard "$d" mobile-webkit-1-2 0 "$PW_PASS_LOG"
    e2e_github_report "$d" >/dev/null 2>&1; rc=$?
    if [ "$rc" = "$E2E_GITHUB_NO_VERDICT_EXIT" ]; then
        pass "a shard with no output is no verdict, never green"
    else
        fail "missing shard rc=$rc"
    fi

    d="$SANDBOX/report-noplan"
    mkdir -p "$d"
    e2e_github_report "$d" >/dev/null 2>&1; rc=$?
    if [ "$rc" = "$E2E_GITHUB_NO_VERDICT_EXIT" ]; then pass "no plan is no verdict"; else fail "no plan rc=$rc"; fi
}

test_the_plan_puts_webkit_on_macos_and_respects_test_ignore() {
    echo "test: the shard plan sends mobile-webkit to macOS and drops specs a project ignores"
    local req="$SANDBOX/plan-request.json" out="$SANDBOX/plan-out" ghout="$SANDBOX/gh-output" specs="" i
    for i in $(seq 1 17); do specs="$specs\"s$i.spec.ts\","; done
    echo "{\"suites\": [\"api\", \"browser\"], \"api_args\": [], \"browser\": {\"projects\": [\"chromium\", \"mobile\", \"mobile-webkit\"], \"specs\": [${specs}\"only-desktop.spec.ts\", \"only-mobile.spec.ts\"], \"pw_args\": []}}" >"$req"
    : >"$ghout"
    E2E_GITHUB_REQUEST="$req" E2E_GITHUB_OUT="$out" GITHUB_OUTPUT="$ghout" \
        "$SCRIPT_DIR/../e2e-github-shard.sh" plan >/dev/null
    local linux macos
    linux="$(sed -n 's/^linux=//p' "$ghout")"
    macos="$(sed -n 's/^macos=//p' "$ghout")"
    if [[ "$macos" == *'"project":"mobile-webkit"'* ]] && [[ "$linux" != *mobile-webkit* ]] && [[ "$macos" != *'"project":"chromium"'* ]]; then
        pass "mobile-webkit shards only in the macOS matrix"
    else
        fail "linux=$linux macos=$macos"
    fi
    # chromium runs 17 + desktop = 18 specs, so 3 shards; mobile runs 17 + mobile = 18, so 3.
    if grep -q "^chromium-3-3 browser chromium 3 3$" "$out/plan/entries" \
        && grep -q "^mobile-3-3 browser mobile 3 3$" "$out/plan/entries" \
        && grep -q "^api api - 1 1$" "$out/plan/entries"; then
        pass "shards follow the specs each project runs"
    else
        fail "entries were: $(tr '\n' ';' <"$out/plan/entries")"
    fi
}

test_the_lock_holder_is_named_only_while_alive() {
    echo "test: the local leg sees the e2e lock as held only while its holder lives"
    local lock dead
    lock="$(_e2e_lock_path)"
    mkdir -p "$(dirname "$lock")"
    rm -f "$lock"
    if [ -z "$(e2e_github_e2e_lock_holder)" ]; then pass "no lock file reads free"; else fail "a missing lock read held"; fi

    printf 'PID=%s\nTHREAD_ID=t-holder\n' "$$" >"$lock"
    if [ "$(e2e_github_e2e_lock_holder)" = "thread t-holder" ]; then
        pass "a live holder is named by its thread"
    else
        fail "live holder read as '$(e2e_github_e2e_lock_holder)'"
    fi

    sleep 0 & dead=$!
    wait "$dead"
    printf 'PID=%s\nTHREAD_ID=t-gone\n' "$dead" >"$lock"
    if [ -z "$(e2e_github_e2e_lock_holder)" ]; then pass "a dead holder reads free"; else fail "a dead holder read held"; fi
    rm -f "$lock"
}

test_the_pinned_postgres_versions_resolve() {
    echo "test: the native Postgres reads its versions from build-headless.sh"
    local pg pgv
    local shard="$SCRIPT_DIR/../e2e-github-shard.sh"
    pg="$(bash -c 'source "$1"; pinned_version PG_VERSION' _ "$shard")"
    pgv="$(bash -c 'source "$1"; pinned_version PGVECTOR_VERSION' _ "$shard")"
    if [[ "$pg" =~ ^[0-9]+\.[0-9]+\.[0-9]+$ ]] && [[ "$pgv" =~ ^[0-9]+\.[0-9]+\.[0-9]+$ ]]; then
        pass "PG $pg, pgvector $pgv"
    else
        fail "read PG '$pg', pgvector '$pgv'"
    fi
}

if [ -n "$HAVE_RELEASE_TREE" ]; then
    test_the_pushed_commit_is_stripped_parentless_and_generic
    test_the_scan_refuses_a_planted_token
    test_an_unloadable_denylist_refuses
else
    echo "skipped: the commit tests need scripts/lib/release_tree.sh, which the mirror does not ship"
fi
test_only_run_branches_qualify
test_delete_refuses_anything_but_a_run_branch
test_the_sweep_removes_only_old_finished_run_branches
test_the_handoff_strips_github_and_ignores_it_after_dashes
test_the_report_matches_the_local_tally_and_exit_codes
test_the_plan_puts_webkit_on_macos_and_respects_test_ignore
test_the_lock_holder_is_named_only_while_alive
test_the_pinned_postgres_versions_resolve

echo ""
echo "Passed: $PASS  Failed: $FAIL"
[ "$FAIL" -eq 0 ]
