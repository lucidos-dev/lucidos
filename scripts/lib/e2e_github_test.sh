#!/bin/bash
# Tests for e2e GitHub mode: scripts/lib/e2e_github.sh, the plan half of
# scripts/e2e-github-shard.sh, and its fake Claude Code install (ADR 0382).
# Also the driver's --commit selection and the main check's (ADR 0464).
#
# Hermetic. A throwaway git repo, an invented denylist, and stubbed
# release_tree_push, release_tree_delete_ref and `gh`, so nothing here reaches a
# remote, the network or a real token.
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

unset LUCIDOS_E2E_LOCAL
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
# A screenshot switch or debug knob in the caller's shell must not steer the hand-off.
while stray="$(e2e_github_local_only_env)" && [ -n "$stray" ]; do unset "$stray"; done

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

test_a_named_commit_is_what_gets_stripped() {
    echo "test: --commit strips the commit it names, not HEAD"
    local named commit
    named="$(git -C "$REPO" rev-parse HEAD)"
    echo "fn later() {}" >"$REPO/src/later.rs"
    git -C "$REPO" add -A && git -C "$REPO" commit -qm "a later commit"
    commit="$(e2e_github_build_commit "$REPO" "$REQUEST" 20261007-120000-ab12 "$named" 2>/dev/null)"
    if [ -n "$commit" ] && git -C "$REPO" cat-file -e "$commit:src/main.rs" 2>/dev/null \
        && ! git -C "$REPO" cat-file -e "$commit:src/later.rs" 2>/dev/null; then
        pass "the named commit's tree was pushed, without HEAD's later file"
    else
        fail "the named commit was not what got built ($commit)"
    fi
    commit="$(e2e_github_build_commit "$REPO" "$REQUEST" 20261007-120000-ab12 2>/dev/null)"
    if [ -n "$commit" ] && git -C "$REPO" cat-file -e "$commit:src/later.rs" 2>/dev/null; then
        pass "with no commit named, HEAD is built"
    else
        fail "HEAD was not built by default ($commit)"
    fi
    git -C "$REPO" rm -q src/later.rs && git -C "$REPO" commit -qm "drop the later file"
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

# shellcheck disable=SC2317 # the stub is called by the function under test
test_delete_refuses_anything_but_a_run_branch() {
    echo "test: e2e_github_delete_branch never deletes a ref outside e2e/"
    local calls="$SANDBOX/delete-calls"
    : >"$calls"
    (
        # The mirror's one door, stubbed: release_tree_push_test.sh drives the
        # real one against a local bare repo.
        release_tree_delete_ref() { echo "$*" >>"$calls"; }
        e2e_github_delete_branch "$REPO" main 2>/dev/null
        e2e_github_delete_branch "$REPO" rc/0.30.0 2>/dev/null
        e2e_github_delete_branch "$REPO" e2e/20261007-120000-ab12
    )
    if [ "$(cat "$calls")" = "$REPO $E2E_GITHUB_REMOTE refs/heads/e2e/20261007-120000-ab12" ]; then
        pass "only the e2e run branch reached the mirror's door"
    else
        fail "delete calls were: $(cat "$calls")"
    fi
}

# shellcheck disable=SC2317,SC2329 # gh and the door are stubbed for the function under test
test_stopping_a_run_survives_a_broken_stderr() {
    echo "test: e2e_github_stop_run cancels the run and deletes its branch even when stderr is a broken pipe"
    local calls="$SANDBOX/stop-calls" fifo="$SANDBOX/stop-fifo"
    : >"$calls"
    mkfifo "$fifo"
    (
        gh() { echo "gh $*" >>"$calls"; }
        release_tree_delete_ref() { echo "delete $3" >>"$calls"; }
        # A pipe whose only reader has gone, as the main check's log reader
        # does when engine shutdown signals its whole process group.
        exec 4<>"$fifo"
        exec 5>"$fifo"
        exec 4<&-
        e2e_github_stop_run "$REPO" e2e/20261007-120000-ab12 4242 1 2>&5
    ) 2>/dev/null
    if grep -q "^gh run cancel 4242 " "$calls" && grep -q "^delete refs/heads/e2e/20261007-120000-ab12$" "$calls"; then
        pass "both calls ran after the first write hit the broken pipe"
    else
        fail "calls were: $(cat "$calls")"
    fi
    : >"$calls"
    (
        gh() { echo "gh $*" >>"$calls"; }
        release_tree_delete_ref() { echo "delete $3" >>"$calls"; }
        e2e_github_stop_run "$REPO" e2e/20261007-120000-ab12 "" "" 2>/dev/null
    )
    if [ ! -s "$calls" ]; then
        pass "a finished, unpushed run needs neither call"
    else
        fail "calls were: $(cat "$calls")"
    fi
}

# shellcheck disable=SC2317 # the stub is called by the function under test
test_the_push_goes_through_the_mirror_door() {
    echo "test: e2e_github_push hands only a run branch to release_tree_push"
    local calls="$SANDBOX/push-calls" out
    : >"$calls"
    (
        release_tree_push() { echo "$*" >>"$calls"; }
        e2e_github_push "$REPO" abc123 main 2>/dev/null
        e2e_github_push "$REPO" abc123 e2e/20261007-120000-ab12
    )
    if [ "$(cat "$calls")" = "$REPO $E2E_GITHUB_REMOTE abc123 refs/heads/e2e/20261007-120000-ab12" ]; then
        pass "only the e2e run branch reached the mirror's door"
    else
        fail "push calls were: $(cat "$calls")"
    fi
    if out="$(
        release_tree_push() { echo "mirror push refused (scan): planted" >&2; return 1; }
        e2e_github_push "$REPO" abc123 e2e/20261007-120000-ab12 2>&1
    )"; then
        fail "a refused push reported success"
    elif [[ "$out" == *"mirror push refused (scan)"* ]]; then
        pass "a refusal fails the push and surfaces its reason"
    else
        fail "a refusal was swallowed: $out"
    fi
}

# shellcheck disable=SC2317 # the stubs are called by e2e_github_sweep
test_the_sweep_removes_only_old_finished_run_branches() {
    echo "test: the sweep deletes e2e run branches over a day old whose runs finished, and nothing else"
    local deletes="$SANDBOX/deletes"
    : >"$deletes"
    (
        e2e_github_now() { e2e_github_branch_started e2e/20261008-120000-ffff; }
        # shellcheck disable=SC2032 # a subshell stub, never reached through xargs
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
            esac
        }
        release_tree_delete_ref() { echo "$*" >>"$deletes"; }
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

# handoff_outcome <unavailable-reason> <suite> [args]: run the hand-off with
# exec and the precondition check stubbed. Prints "github <suite> <args>" when
# it handed over, or "local <args> env=<LUCIDOS_E2E_LOCAL>" when it stayed.
# shellcheck disable=SC2317 # exec and the reason check are shadowed to observe the hand-off
handoff_outcome() {
    local stub_reason="$1"
    shift
    (
        exec() { shift; echo "github $*"; exit 0; }
        e2e_github_unavailable_reason() { printf '%s' "$stub_reason"; }
        e2e_github_handoff "$@"
        echo "local ${E2E_ARGS[*]-} env=${LUCIDOS_E2E_LOCAL:-}"
    ) 2>&1
}

expect_outcome() { # <what> <expected glob> <actual>
    # shellcheck disable=SC2053 # $2 is a glob on purpose
    if [[ "$3" == $2 ]]; then pass "$1"; else fail "$1: got '$3'"; fi
}

test_the_handoff_defaults_to_github() {
    echo "test: e2e_github_handoff hands over by default, keeping every other argument"
    expect_outcome "a bare run goes to GitHub" "*running on GitHub*github all" "$(handoff_outcome "" all)"
    expect_outcome "filters ride along" "*running on GitHub*github browser -f chat.spec.ts -- --grep x" \
        "$(handoff_outcome "" browser -f chat.spec.ts -- --grep x)"
    expect_outcome "--github is stripped" "*running on GitHub*github api -f health" "$(handoff_outcome "" api --github -f health)"
    expect_outcome "--local after -- belongs to Playwright" "*running on GitHub*github browser -- --local" \
        "$(handoff_outcome "" browser -- --local)"
}

# shellcheck disable=SC2030,SC2031 # each export is scoped to its own $(...) on purpose
test_the_handoff_stays_local_when_asked_or_inherited() {
    echo "test: --local, a local-only flag, or an inherited LUCIDOS_E2E_LOCAL keeps the run here"
    expect_outcome "--local stays, minus the flag, and exports the mark" "local -f health env=1" \
        "$(handoff_outcome "" api --local -f health)"
    local flag
    for flag in "${E2E_GITHUB_LOCAL_ONLY_FLAGS[@]}"; do
        expect_outcome "$flag stays local" "local $flag -f chat.spec.ts env=1" \
            "$(handoff_outcome "" browser "$flag" -f chat.spec.ts)"
    done
    expect_outcome "cargo args to wasm stay local" "local -- --nocapture env=1" \
        "$(handoff_outcome "" wasm -- --nocapture)"
    expect_outcome "a bare embedder run goes to GitHub" "*running on GitHub*github embedder" \
        "$(handoff_outcome "" embedder)"
    expect_outcome "LUCIDOS_E2E_PACKAGED stays local and says why" "*LUCIDOS_E2E_PACKAGED is set*local  env=1" \
        "$(export LUCIDOS_E2E_PACKAGED=1; handoff_outcome "" all)"
    expect_outcome "a screenshot switch stays local" "*THREAD_SHOTS is set*local -f thread-shots.spec.ts env=1" \
        "$(export THREAD_SHOTS=1; handoff_outcome "" browser -f thread-shots.spec.ts)"
    expect_outcome "a switch set to 0 is off" "*running on GitHub*github api" \
        "$(export LUCIDOS_E2E_DEBUG=0; handoff_outcome "" api)"
    expect_outcome "--github with a local-only variable is refused" "*contradict*" \
        "$(export LUCIDOS_E2E_PACKAGED=1; handoff_outcome "" all --github)"
    expect_outcome "an inherited mark wins even over --github" "local -f chat.spec.ts env=1" \
        "$(LUCIDOS_E2E_LOCAL=1 handoff_outcome "" browser --github -f chat.spec.ts)"
    expect_outcome "--github with --local is refused" "*contradict*" \
        "$(handoff_outcome "" browser --github --local)"
}

test_the_handoff_falls_back_only_without_github() {
    echo "test: an unmet precondition runs here and says why, unless --github insists"
    expect_outcome "the fallback names its reason" "*running on this host: the tree is dirty*local -f x env=1" \
        "$(handoff_outcome "the tree is dirty" api -f x)"
    expect_outcome "--github hands over so the driver refuses loudly" "*running on GitHub*github api -f x" \
        "$(handoff_outcome "the tree is dirty" api --github -f x)"
}

# shellcheck disable=SC2317,SC2329 # gh is shadowed so the check never depends on this host
test_the_unavailable_reason_names_each_precondition() {
    echo "test: e2e_github_unavailable_reason names each unmet precondition, and nothing when all hold"
    local repo="$SANDBOX/maintainer" out
    git init -q "$repo"
    mkdir -p "$repo/scripts/lib"
    echo "# stand-in" >"$repo/scripts/lib/release_tree.sh"
    git -C "$repo" add -A
    git -C "$repo" -c user.email=dev@example.com -c user.name=Dev commit -qm init
    gh() { :; }

    expect_outcome "a mirror checkout lacks the release libraries" "*release_tree.sh*" \
        "$(e2e_github_unavailable_reason "$REPO")"
    expect_outcome "a missing remote is named" "*'lucidos' remote*" "$(e2e_github_unavailable_reason "$repo")"
    git -C "$repo" remote add lucidos https://example.invalid/mirror.git
    out="$(e2e_github_unavailable_reason "$repo")"
    if [ -z "$out" ]; then pass "a clean maintainer checkout can use GitHub"; else fail "refused with: $out"; fi
    echo "scratch" >"$repo/untracked.txt"
    expect_outcome "a dirty tree is named" "*uncommitted*" "$(e2e_github_unavailable_reason "$repo")"
    out="$(e2e_github_unavailable_reason "$repo" "$(git -C "$repo" rev-parse HEAD)")"
    if [ -z "$out" ]; then pass "a named commit ignores the dirty tree beside it"; else fail "refused with: $out"; fi
    expect_outcome "a commit that does not resolve is named" "*names no commit*" \
        "$(e2e_github_unavailable_reason "$repo" 0000000000000000000000000000000000000000)"
    unset -f gh
}

test_the_main_check_selection_has_no_fallback() {
    echo "test: main-check needs --commit, and an unmet precondition is no verdict"
    local driver="$SCRIPT_DIR/../e2e-github.sh" rc out
    out="$("$driver" main-check 2>&1)"; rc=$?
    if [ "$rc" = 1 ] && [[ "$out" == *"pass --commit"* ]]; then
        pass "main-check without --commit is a usage error"
    else
        fail "rc=$rc: $out"
    fi
    out="$("$driver" main-check --commit 2>&1)"; rc=$?
    if [ "$rc" = 1 ] && [[ "$out" == *"--commit needs a commit"* ]]; then
        pass "--commit with no value is a usage error"
    else
        fail "rc=$rc: $out"
    fi
    # A commit nobody has fails the preflight, before anything is built or pushed.
    out="$("$driver" main-check --commit 0000000000000000000000000000000000000000 2>&1)"; rc=$?
    if [ "$rc" = "$E2E_GITHUB_NO_VERDICT_EXIT" ] && [[ "$out" == *"GitHub mode cannot run"* ]]; then
        pass "an unmet precondition exits 76, with no local fallback"
    else
        fail "rc=$rc: $out"
    fi
    out="$("$driver" api --commit 0000000000000000000000000000000000000000 2>&1)"; rc=$?
    if [ "$rc" = 1 ]; then
        pass "any other selection keeps exit 1 for an unmet precondition"
    else
        fail "rc=$rc: $out"
    fi
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

test_the_report_counts_the_engine_suite() {
    echo "test: the engine suite reports first, and its failure fails the run"
    local d="$SANDBOX/report-engine" out rc
    mkdir -p "$d/e2e-plan"
    printf '%s\n' "engine engine - 1 1" "api api - 1 1" >"$d/e2e-plan/entries"
    fake_shard "$d" engine 101 "test result: FAILED. 1 failed"
    fake_shard "$d" api 0 "test result: ok"
    out="$(e2e_github_report "$d" 2>&1)"; rc=$?
    if [ "$rc" = 101 ] && [[ "$out" == *"── engine exit code: 101 ──"* ]] \
        && [ "$(printf '%s\n' "$out" | grep -n '══ engine' | cut -d: -f1)" -lt "$(printf '%s\n' "$out" | grep -n '══ api' | cut -d: -f1)" ]; then
        pass "engine reports before api and fails the run with its code"
    else
        fail "rc=$rc: $out"
    fi
}

test_a_refused_build_is_no_verdict_and_a_failure_outranks_it() {
    echo "test: a shard whose build the memory gate refused is no verdict, and a real failure still wins"
    local d="$SANDBOX/report-refused" out rc
    mkdir -p "$d/e2e-plan"
    printf '%s\n' "engine engine - 1 1" "api api - 1 1" >"$d/e2e-plan/entries"
    fake_shard "$d" engine "$HOST_MEMORY_BUILD_REFUSED_EXIT" "[build-memory-gate] the build never started"
    fake_shard "$d" api 0 "test result: ok"
    out="$(e2e_github_report "$d" 2>&1)"; rc=$?
    if [ "$rc" = "$E2E_GITHUB_NO_VERDICT_EXIT" ] && [[ "$out" == *"ERROR: the engine shard's build never started"* ]]; then
        pass "a refused engine build is no verdict, and says why"
    else
        fail "rc=$rc: $out"
    fi

    d="$SANDBOX/report-refused-red"
    mkdir -p "$d/e2e-plan"
    printf '%s\n' "engine engine - 1 1" "api api - 1 1" "chromium-1-1 browser chromium 1 1" >"$d/e2e-plan/entries"
    fake_shard "$d" engine "$HOST_MEMORY_BUILD_REFUSED_EXIT" "[build-memory-gate] the build never started"
    fake_shard "$d" api 0 "test result: ok"
    fake_shard "$d" chromium-1-1 1 "$PW_FAIL_LOG"
    e2e_github_report "$d" >/dev/null 2>&1; rc=$?
    if [ "$rc" = 1 ]; then pass "a later red phase outranks an earlier no verdict"; else fail "rc=$rc"; fi
}

# shellcheck disable=SC2317,SC2329 # gh, the clock and sleep are stubbed for the function under test
test_the_wait_gives_up_with_no_verdict() {
    echo "test: e2e_github_wait ends with no verdict when the run never completes in time"
    local clock="$SANDBOX/wait-clock" out rc
    echo 1000 >"$clock"
    out="$(
        sleeps=0
        gh() { return 1; }
        e2e_github_now() { cat "$clock"; }
        # Each sleep moves the fake clock, so no time passes. A wait with no
        # deadline runs out of sleeps instead of hanging the suite.
        sleep() {
            sleeps=$((sleeps + 1))
            [ "$sleeps" -le 50 ] || exit 99
            echo $(($(cat "$clock") + $1)) >"$clock"
        }
        E2E_GITHUB_RUN_WAIT_SECS=120 E2E_GITHUB_POLL_SECS=30 e2e_github_wait 4242 2>&1
    )"; rc=$?
    if [ "$rc" = "$E2E_GITHUB_NO_VERDICT_EXIT" ] && [[ "$out" == *"ERROR: run 4242 did not complete within 120s"* ]]; then
        pass "a gh that keeps failing ends in no verdict at the deadline"
    else
        fail "rc=$rc: $out"
    fi
    echo 1000 >"$clock"
    out="$(
        gh() { echo '{"status": "completed", "conclusion": "success", "jobs": []}'; }
        e2e_github_now() { cat "$clock"; }
        sleep() { exit 99; }
        E2E_GITHUB_RUN_WAIT_SECS=0 e2e_github_wait 4242 2>/dev/null
    )"; rc=$?
    if [ "$rc" = 0 ] && [ "$out" = success ]; then
        pass "a completed run still reports its conclusion"
    else
        fail "rc=$rc: $out"
    fi
}

test_the_plan_gives_the_engine_suite_its_own_job() {
    echo "test: the plan runs the engine suite in its own job, and only when asked"
    local req="$SANDBOX/engine-request.json" out="$SANDBOX/engine-plan" ghout="$SANDBOX/engine-gh-output"
    echo '{"suites": ["engine", "api"], "api_args": [], "browser": {"projects": [], "specs": [], "pw_args": []}}' >"$req"
    : >"$ghout"
    E2E_GITHUB_REQUEST="$req" E2E_GITHUB_OUT="$out" GITHUB_OUTPUT="$ghout" \
        "$SCRIPT_DIR/../e2e-github-shard.sh" plan >/dev/null
    if grep -q '^has_engine=true$' "$ghout" && grep -q "^engine engine - 1 1$" "$out/plan/entries" \
        && ! grep '^linux=' "$ghout" | grep -q engine; then
        pass "the engine suite gets its own job and stays out of the Linux matrix"
    else
        fail "gh output: $(tr '\n' ';' <"$ghout") entries: $(tr '\n' ';' <"$out/plan/entries")"
    fi
    echo '{"suites": ["api"], "api_args": [], "browser": {"projects": [], "specs": [], "pw_args": []}}' >"$req"
    : >"$ghout"
    E2E_GITHUB_REQUEST="$req" E2E_GITHUB_OUT="$out" GITHUB_OUTPUT="$ghout" \
        "$SCRIPT_DIR/../e2e-github-shard.sh" plan >/dev/null
    if grep -q '^has_engine=false$' "$ghout"; then
        pass "a request without it runs no engine job"
    else
        fail "gh output: $(tr '\n' ';' <"$ghout")"
    fi
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

test_only_tagged_specs_stay_on_the_local_leg() {
    echo "test: a spec stays local only when a test in it is tagged @real-claude-code"
    local spec_dir="$SCRIPT_DIR/../../crates/lucidos-app/e2e" spec wrong=""
    for spec in chat coding-agent-cancel coding-agent-resume-after-exit side-question; do
        e2e_spec_needs_real_claude_code "$spec_dir/$spec.spec.ts" || wrong="$wrong $spec(untagged)"
    done
    for spec in coding-agent coding-agent-follow-ups coding-agent-mid-session-settings \
        coding-agent-model-preselect coding-agent-reasoning-preselect model-switching \
        control-menu control-menu-over-header-mobile; do
        e2e_spec_spawns_claude_code "$spec_dir/$spec.spec.ts" || wrong="$wrong $spec(spawns-nothing)"
        ! e2e_spec_needs_real_claude_code "$spec_dir/$spec.spec.ts" || wrong="$wrong $spec(tagged)"
    done
    if [ -z "$wrong" ]; then pass "four files local, eight on GitHub against the fake"; else fail "misrouted:$wrong"; fi
}

test_a_named_commit_routes_the_specs_it_holds() {
    echo "test: under --commit the browser specs are listed and read from that commit, not the working tree"
    local repo="$SANDBOX/spec-repo" dir commit out
    dir="$repo/$E2E_GITHUB_SPEC_DIR"
    git init -q "$repo"
    mkdir -p "$dir"
    echo "test('resume', { tag: '@real-claude-code' }, () => {})" >"$dir/resume.spec.ts"
    echo "test('send', () => {})" >"$dir/chat.spec.ts"
    git -C "$repo" add -A
    git -C "$repo" -c user.email=dev@example.com -c user.name=Dev commit -qm specs
    commit="$(git -C "$repo" rev-parse HEAD)"
    # The working tree moves on: chat gains the tag, resume goes, a spec arrives.
    echo "test('send', { tag: '@real-claude-code' }, () => {})" >"$dir/chat.spec.ts"
    rm "$dir/resume.spec.ts"
    echo "test('new', () => {})" >"$dir/later.spec.ts"

    out="$(e2e_github_split_specs "$repo" "$commit" | sort | tr '\n' ';')"
    if [ "$out" = "local resume.spec.ts;remote chat.spec.ts;" ]; then
        pass "the commit's specs, routed by the commit's tags"
    else
        fail "routes from the commit: $out"
    fi
    out="$(e2e_github_split_specs "$repo" "" | sort | tr '\n' ';')"
    if [ "$out" = "local chat.spec.ts;remote later.spec.ts;" ]; then
        pass "without a commit, the working tree's specs"
    else
        fail "routes from the working tree: $out"
    fi
    out="$(e2e_github_split_specs "$repo" "$commit" resume.spec.ts)"
    if [ "$out" = "local resume.spec.ts" ]; then
        pass "a named spec is read from the commit"
    else
        fail "named spec: $out"
    fi
    if out="$(e2e_github_split_specs "$repo" "$commit" later.spec.ts 2>&1)"; then
        fail "a spec the commit lacks was routed: $out"
    elif [[ "$out" == *"no spec named later.spec.ts in $commit"* ]]; then
        pass "a spec the commit lacks is refused, naming the commit"
    else
        fail "refusal: $out"
    fi

    # The tag sits on line one of a spec far bigger than a pipe buffer. The
    # driver runs under pipefail, so the check must not lose git to SIGPIPE.
    {
        echo "test('big', { tag: '@real-claude-code' }, () => {})"
        yes "// filler that pads the spec past any pipe buffer" | head -n 50000
    } >"$dir/big.spec.ts"
    git -C "$repo" add -A
    git -C "$repo" -c user.email=dev@example.com -c user.name=Dev commit -qm big
    out="$(set -o pipefail; e2e_github_split_specs "$repo" "$(git -C "$repo" rev-parse HEAD)" big.spec.ts)"
    if [ "$out" = "local big.spec.ts" ]; then
        pass "a large tagged spec in a commit stays local under pipefail"
    else
        fail "large spec: $out"
    fi
}

test_a_spec_file_among_playwright_args_is_a_selection() {
    echo "test: a spec file after -- narrows the run, and an option's value never does"
    local wrong=""
    e2e_github_is_spec_arg "" "chat.spec.ts" || wrong="$wrong bare-name"
    e2e_github_is_spec_arg "e2e/chat.spec.ts" "e2e/side-question.spec.ts" || wrong="$wrong path-after-spec"
    e2e_github_is_spec_arg "--retries" "e2e/chat.spec.ts" || wrong="$wrong after-a-flag"
    ! e2e_github_is_spec_arg "--grep" "chat.spec.ts" || wrong="$wrong grep-value"
    ! e2e_github_is_spec_arg "-g" "chat.spec.ts" || wrong="$wrong g-value"
    ! e2e_github_is_spec_arg "" "--grep" || wrong="$wrong flag"
    ! e2e_github_is_spec_arg "" "send message" || wrong="$wrong title"
    if [ -z "$wrong" ]; then pass "spec files select, option values and titles do not"; else fail "misread:$wrong"; fi
}

test_the_plan_refuses_a_real_claude_code_spec() {
    echo "test: the shard plan refuses a request naming a @real-claude-code spec"
    local req="$SANDBOX/real-cc-request.json" ghout="$SANDBOX/real-cc-gh-output"
    echo '{"suites": ["browser"], "api_args": [], "browser": {"projects": ["chromium"], "specs": ["coding-agent.spec.ts", "chat.spec.ts"], "pw_args": []}}' >"$req"
    : >"$ghout"
    if E2E_GITHUB_REQUEST="$req" E2E_GITHUB_OUT="$SANDBOX/real-cc-out" GITHUB_OUTPUT="$ghout" \
        "$SCRIPT_DIR/../e2e-github-shard.sh" plan >/dev/null 2>&1; then
        fail "the plan accepted chat.spec.ts"
    elif grep -q '^linux=' "$ghout"; then
        fail "the plan refused but still wrote a matrix"
    else
        pass "refused, with no matrix written"
    fi
}

test_the_fake_installs_only_on_a_runner_and_never_over_a_claude() {
    echo "test: the fake Claude Code installs only on a GitHub runner, never over an existing one"
    local shard="$SCRIPT_DIR/../e2e-github-shard.sh" home="$SANDBOX/runner-home" target rel fake rust want
    rel="$(bash -c 'source "$1"; printf %s "$FAKE_CLAUDE_CODE_HOME_PATH"' _ "$shard")"
    fake="$(bash -c 'source "$1"; printf %s "$FAKE_CLAUDE_CODE"' _ "$shard")"
    rust="$SCRIPT_DIR/../../crates/lucidos-engine/src/runtime/claude_code.rs"
    # The engine's first probe, the first home.join() in resolve_claude_binary.
    if [ "$(sed -n '/fn resolve_claude_binary(/,/^}/p' "$rust" | grep -o 'home\.join("[^"]*")' | head -1)" = "home.join(\"$rel\")" ]; then
        pass "the fake goes where resolve_claude_binary looks first"
    else
        fail "FAKE_CLAUDE_CODE_HOME_PATH ($rel) is not resolve_claude_binary's first probe"
    fi
    target="$home/$rel"
    mkdir -p "$home"
    if GITHUB_ACTIONS='' bash -c 'source "$1"; install_fake_claude_code "$2"' _ "$shard" "$home" 2>/dev/null \
        || [ -e "$target" ] || [ -L "$target" ]; then
        fail "installed off a runner"
    else
        pass "refused off a runner, nothing written"
    fi
    mkdir -p "$(dirname "$target")"
    echo "a real claude" >"$target"
    if GITHUB_ACTIONS=true bash -c 'source "$1"; install_fake_claude_code "$2"' _ "$shard" "$home" 2>/dev/null \
        || [ "$(cat "$target")" != "a real claude" ]; then
        fail "replaced an existing claude"
    else
        pass "refused over an existing claude, which is untouched"
    fi
    rm -f "$target"
    if GITHUB_ACTIONS=true bash -c 'source "$1"; install_fake_claude_code "$2"' _ "$shard" "$home" \
        && want="$(node "$fake" --version)" && [ -n "$want" ] \
        && [ "$("$target" --version)" = "$want" ]; then
        pass "installed on a runner, and it runs through the link"
    else
        fail "the installed fake did not answer --version"
    fi
}

test_the_runner_checkout_gets_a_main_and_an_existing_one_never_moves() {
    echo "test: a checkout with no main gets one at HEAD, and an existing main is left alone"
    local shard="$SCRIPT_DIR/../e2e-github-shard.sh" repo="$SANDBOX/runner-checkout" head old
    git init -q -b e2e/20261008-000000-ab12 "$repo"
    git -C "$repo" -c user.email=e2e@example.com -c user.name=e2e commit -q --allow-empty -m one
    head="$(git -C "$repo" rev-parse HEAD)"
    bash -c 'source "$1"; ensure_main_branch "$2"' _ "$shard" "$repo"
    if [ "$(git -C "$repo" rev-parse --verify --quiet refs/heads/main)" = "$head" ]; then
        pass "main names the run branch's HEAD"
    else
        fail "no main at HEAD after ensure_main_branch"
    fi
    old="$head"
    git -C "$repo" -c user.email=e2e@example.com -c user.name=e2e commit -q --allow-empty -m two
    bash -c 'source "$1"; ensure_main_branch "$2"' _ "$shard" "$repo"
    if [ "$(git -C "$repo" rev-parse refs/heads/main)" = "$old" ]; then
        pass "an existing main is never moved"
    else
        fail "ensure_main_branch moved an existing main"
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
    test_a_named_commit_is_what_gets_stripped
else
    echo "skipped: the commit tests need scripts/lib/release_tree.sh, which the mirror does not ship"
fi
test_only_run_branches_qualify
test_delete_refuses_anything_but_a_run_branch
test_stopping_a_run_survives_a_broken_stderr
test_the_push_goes_through_the_mirror_door
test_the_sweep_removes_only_old_finished_run_branches
test_the_handoff_defaults_to_github
test_the_handoff_stays_local_when_asked_or_inherited
test_the_handoff_falls_back_only_without_github
test_the_unavailable_reason_names_each_precondition
test_the_main_check_selection_has_no_fallback
test_the_report_matches_the_local_tally_and_exit_codes
test_the_report_counts_the_engine_suite
test_a_refused_build_is_no_verdict_and_a_failure_outranks_it
test_the_wait_gives_up_with_no_verdict
test_the_plan_gives_the_engine_suite_its_own_job
test_the_plan_puts_webkit_on_macos_and_respects_test_ignore
test_only_tagged_specs_stay_on_the_local_leg
test_a_named_commit_routes_the_specs_it_holds
test_a_spec_file_among_playwright_args_is_a_selection
test_the_plan_refuses_a_real_claude_code_spec
test_the_fake_installs_only_on_a_runner_and_never_over_a_claude
test_the_runner_checkout_gets_a_main_and_an_existing_one_never_moves
test_the_lock_holder_is_named_only_while_alive
test_the_pinned_postgres_versions_resolve

echo ""
echo "Passed: $PASS  Failed: $FAIL"
[ "$FAIL" -eq 0 ]
