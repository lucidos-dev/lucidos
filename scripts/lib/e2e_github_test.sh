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
    : >"$calls"
    (
        gh() { echo "gh $*" >>"$calls"; }
        release_tree_delete_ref() { echo "delete $3" >>"$calls"; }
        e2e_github_stop_run "$REPO" e2e/20261007-120000-ab12 4242 1 1 2>/dev/null
    )
    if [ ! -s "$calls" ]; then
        pass "a detached driver neither cancels the run nor deletes its branch"
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
    expect_outcome "flags ride along" "*running on GitHub*github browser --no-webkit -- --workers 2" \
        "$(handoff_outcome "" browser --no-webkit -- --workers 2)"
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
        expect_outcome "$flag stays local" "*local --no-webkit $flag -f chat.spec.ts env=1" \
            "$(handoff_outcome "" browser "$flag" -f chat.spec.ts)"
    done
    expect_outcome "cargo args to wasm stay local" "local -- --nocapture env=1" \
        "$(handoff_outcome "" wasm -- --nocapture)"
    expect_outcome "a bare embedder run goes to GitHub" "*running on GitHub*github embedder" \
        "$(handoff_outcome "" embedder)"
    expect_outcome "LUCIDOS_E2E_PACKAGED stays local and says why" "*LUCIDOS_E2E_PACKAGED is set*local  env=1" \
        "$(export LUCIDOS_E2E_PACKAGED=1; handoff_outcome "" all)"
    expect_outcome "a screenshot switch stays local" "*THREAD_SHOTS is set*local --no-webkit -f thread-shots-all.spec.ts env=1" \
        "$(export THREAD_SHOTS=1; handoff_outcome "" browser -f thread-shots-all.spec.ts)"
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
    expect_outcome "the fallback names its reason" "*running on this host: the tree is dirty*local  env=1" \
        "$(handoff_outcome "the tree is dirty" api)"
    expect_outcome "--github hands over so the driver refuses loudly" "*running on GitHub*github api -f x" \
        "$(handoff_outcome "the tree is dirty" api --github -f x)"
}

# lean_outcome <suite> [args]: the hand-off with exec stubbed and no engine
# profile chosen by the caller, unless LEAN_KEEP_DEBUG keeps LUCIDOS_E2E_DEBUG. Prints "github <args>", or "local <args>
# debug=<LUCIDOS_E2E_DEBUG or unset>".
# shellcheck disable=SC2317,SC2031 # exec and the reason check are shadowed; each subshell owns its env
lean_outcome() {
    (
        unset RELEASE
        [ -n "${LEAN_KEEP_DEBUG:-}" ] || unset LUCIDOS_E2E_DEBUG
        exec() { shift; echo "github $*"; exit 0; }
        e2e_github_unavailable_reason() { :; }
        e2e_github_handoff "$@"
        echo "local ${E2E_ARGS[*]-} debug=${LUCIDOS_E2E_DEBUG-unset}"
    ) 2>/dev/null
}

test_a_filtered_run_stays_here_and_runs_lean() {
    echo "test: a filtered run stays on this host, on a debug engine, without mobile-webkit"
    expect_outcome "-f stays, with --no-webkit and a debug engine" "local --no-webkit -f a.spec.ts debug=1" \
        "$(lean_outcome browser -f a.spec.ts)"
    local tail
    for tail in "--grep x" "-g x" "--grep=x" "--grep-invert x" "chat"; do
        # shellcheck disable=SC2086 # the tail splits into its words on purpose
        expect_outcome "-- $tail is a filter" "local --no-webkit -- $tail debug=1" "$(lean_outcome browser -- $tail)"
    done
    expect_outcome "a positional spec before -- is a filter" "local --no-webkit chat.spec.ts debug=1" \
        "$(lean_outcome browser chat.spec.ts)"
    expect_outcome "--webkit keeps WebKit and the release engine" "local --webkit -f a-mobile.spec.ts debug=unset" \
        "$(lean_outcome browser --webkit -f a-mobile.spec.ts)"
    expect_outcome "a --project keeps the release engine" "local -- --project mobile-webkit -g x debug=unset" \
        "$(lean_outcome browser -- --project mobile-webkit -g x)"
    expect_outcome "a filtered API run stays, on a debug engine" "local -f health debug=1" \
        "$(lean_outcome api -f health)"
    expect_outcome "an explicit LUCIDOS_E2E_DEBUG=0 keeps the release engine" "local --no-webkit -f a.spec.ts debug=0" \
        "$(LEAN_KEEP_DEBUG=1 LUCIDOS_E2E_DEBUG=0 lean_outcome browser -f a.spec.ts)"
}

test_an_unfiltered_run_still_goes_to_github() {
    echo "test: a whole suite goes to GitHub, whatever flags it carries"
    local args
    for args in "browser" "browser --webkit" "browser --no-webkit" "browser -- --workers 2" \
        "browser -- --project mobile" "api -- --nocapture" "all --no-webkit"; do
        # shellcheck disable=SC2086 # the case splits into its words on purpose
        expect_outcome "$args goes to GitHub" "github $args" "$(lean_outcome $args)"
    done
    expect_outcome "--github sends a filtered run to GitHub" "github browser -f a.spec.ts" \
        "$(lean_outcome browser --github -f a.spec.ts)"
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

test_attach_takes_a_run_and_its_branch_for_the_main_check_only() {
    echo "test: --attach needs main-check, a run URL and its run branch"
    local driver="$SCRIPT_DIR/../e2e-github.sh" rc out sha=0000000000000000000000000000000000000000
    local url="https://github.com/lucidos-dev/lucidos/actions/runs/4242"
    out="$("$driver" api --commit "$sha" --attach "$url" --branch e2e/20261007-120000-ab12 2>&1)"; rc=$?
    if [ "$rc" = 1 ] && [[ "$out" == *"they need main-check"* ]]; then
        pass "another selection refuses --attach"
    else
        fail "rc=$rc: $out"
    fi
    out="$("$driver" main-check --commit "$sha" --attach "$url" --branch main 2>&1)"; rc=$?
    if [ "$rc" = "$E2E_GITHUB_NO_VERDICT_EXIT" ] && [[ "$out" == *"e2e/<run-id> branch"* ]]; then
        pass "a branch that is not a run branch is no verdict"
    else
        fail "rc=$rc: $out"
    fi
    out="$("$driver" main-check --commit "$sha" --attach not-a-run --branch e2e/20261007-120000-ab12 2>&1)"; rc=$?
    if [ "$rc" = "$E2E_GITHUB_NO_VERDICT_EXIT" ] && [[ "$out" == *"needs a run URL"* ]]; then
        pass "a run that names no run id is no verdict"
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

# fake_api_answer <remaining> <json-body>: one `gh api --include` answer.
fake_api_answer() {
    printf 'HTTP/2.0 200 OK\r\nX-Ratelimit-Remaining: %s\r\nContent-Type: application/json\r\n\r\n%s\n' "$1" "$2"
}

# run_state_fixture <run-status> <test-job-status> [<remaining>]: a run state as
# e2e_github_run_state prints it, with a plan job, a skipped build and one test
# job.
run_state_fixture() {
    printf '{"status": "%s", "conclusion": null, "jobs": [{"name": "plan", "status": "completed"}, {"name": "build-linux", "status": "in_progress"}, {"name": "chromium-1-1", "status": "%s"}], "rate_remaining": %s}' \
        "$1" "$2" "${3:-4000}"
}

test_the_run_state_reads_both_answers_and_the_rate_limit() {
    echo "test: the run state joins the run and its jobs, with the lower rate-limit remainder"
    local got
    got="$(
        gh() {
            case "$*" in
                (*"/jobs?per_page=100"*) fake_api_answer 812 '{"jobs": [{"name": "plan", "status": "completed", "conclusion": "success", "started_at": "2026-01-01T00:00:00Z", "steps": []}]}' ;;
                (*) fake_api_answer 813 '{"status": "in_progress", "conclusion": null, "updated_at": "2026-01-01T00:01:00Z"}' ;;
            esac
        }
        e2e_github_run_state 4242
    )"
    if [ "$(printf '%s' "$got" | python3 -c 'import json, sys; s = json.load(sys.stdin); print(s["status"], s["rate_remaining"], s["jobs"][0]["name"], "steps" in s["jobs"][0])')" = "in_progress 812 plan False" ]; then
        pass "status, jobs and the lower remainder, without the job steps"
    else
        fail "state: $got"
    fi
    if (gh() { return 1; }; e2e_github_run_state 4242 >/dev/null); then
        fail "a failed gh call still gave a state"
    else
        pass "a failed gh call gives no state"
    fi
}

test_the_poll_speeds_up_once_a_test_job_starts() {
    echo "test: the poll waits 30 s until a test job starts, then 5 s, and 30 s again near the rate limit"
    local got
    got="$(e2e_github_poll_secs "$(run_state_fixture in_progress queued)")"
    if [ "$got" = 30 ]; then pass "plan and build jobs alone keep the slow poll"; else fail "before tests: $got"; fi
    got="$(e2e_github_poll_secs "$(run_state_fixture in_progress in_progress)")"
    if [ "$got" = 5 ]; then pass "a running test job makes it fast"; else fail "tests running: $got"; fi
    got="$(e2e_github_poll_secs "$(run_state_fixture in_progress completed 500)")"
    if [ "$got" = 5 ]; then pass "500 remaining calls still poll fast"; else fail "at the floor: $got"; fi
    got="$(e2e_github_poll_secs "$(run_state_fixture in_progress in_progress 499)")"
    if [ "$got" = 30 ]; then pass "below 500 remaining calls it slows down"; else fail "below the floor: $got"; fi
    got="$(e2e_github_poll_secs "$(run_state_fixture in_progress in_progress null)")"
    if [ "$got" = 5 ]; then pass "an answer without the header does not slow it"; else fail "no header: $got"; fi
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
        E2E_GITHUB_RUN_WAIT_SECS=120 e2e_github_wait 4242 2>&1
    )"; rc=$?
    if [ "$rc" = "$E2E_GITHUB_NO_VERDICT_EXIT" ] && [[ "$out" == *"ERROR: run 4242 did not complete within 120s"* ]]; then
        pass "a gh that keeps failing ends in no verdict at the deadline"
    else
        fail "rc=$rc: $out"
    fi
    echo 1000 >"$clock"
    out="$(
        e2e_github_run_state() { echo '{"status": "completed", "conclusion": "success", "jobs": []}'; }
        e2e_github_now() { cat "$clock"; }
        sleep() { exit 99; }
        E2E_GITHUB_RUN_WAIT_SECS=0 e2e_github_wait 4242 2>/dev/null | tail -1
    )"; rc=$?
    if [ "$rc" = 0 ] && [ "$(e2e_github_conclusion "$out")" = success ]; then
        pass "a completed run still reports its conclusion"
    else
        fail "rc=$rc: $out"
    fi
}

# shellcheck disable=SC2317,SC2329 # the run state, the clock and sleep are stubbed
test_the_wait_polls_its_own_run_adaptively() {
    echo "test: e2e_github_wait polls only its own run, slow before the tests and fast once they run"
    local polls="$SANDBOX/wait-polls" out
    : >"$polls"
    out="$(
        e2e_github_run_state() {
            echo "$1" >>"$polls"
            case "$(wc -l <"$polls" | tr -d ' ')" in
                (1) run_state_fixture queued queued ;;
                (2) run_state_fixture in_progress in_progress ;;
                (*) run_state_fixture completed completed ;;
            esac
        }
        sleep() { echo "slept $1" >&2; }
        e2e_github_wait 4242 2>&1 >/dev/null
    )"
    if [ "$(sort -u "$polls")" = 4242 ] && [ "$(wc -l <"$polls" | tr -d ' ')" = 3 ] \
        && [ "$(grep '^slept' <<<"$out" | tr '\n' ' ')" = "slept 30 slept 5 " ]; then
        pass "it sleeps 30 s, then 5 s, and stops at completion"
    else
        fail "polled $(tr '\n' ' ' <"$polls"): $out"
    fi
}

test_the_timings_record_reuse_and_job_times() {
    echo "test: a finished run records push to verdict, job times and engine reuse"
    local dir="$SANDBOX/timings" log="$SANDBOX/timings.jsonl" state out got
    rm -rf "$dir" "$log"
    mkdir -p "$dir/e2e-plan"
    printf '%s\n' "linux 1234" "macos -" >"$dir/e2e-plan/engines"
    state='{"status": "completed", "conclusion": "success", "updated_at": "2026-01-01T00:04:00Z", "jobs": [
        {"name": "plan", "conclusion": "success", "created_at": "2026-01-01T00:00:10Z", "started_at": "2026-01-01T00:00:15Z", "completed_at": "2026-01-01T00:00:40Z"},
        {"name": "build-linux", "conclusion": "skipped", "created_at": null, "started_at": null, "completed_at": null},
        {"name": "chromium-1-1", "conclusion": "success", "created_at": "2026-01-01T00:00:45Z", "started_at": "2026-01-01T00:01:05Z", "completed_at": "2026-01-01T00:03:50Z"}]}'
    out="$(
        e2e_github_now() { echo 1767225900; }
        LUCIDOS_E2E_TIMINGS_LOG="$log" e2e_github_record_timings "$dir" 4242 browser 1767225600 0 "$state" 2>&1
    )"
    got="$(python3 -c '
import json, sys
r = json.load(open(sys.argv[1]))
jobs = {j["name"]: (j["queue_secs"], j["run_secs"]) for j in r["jobs"]}
print(r["push_to_verdict_secs"], r["push_to_complete_secs"], r["engine_reused_from"], jobs)
' "$dir/timings.json" 2>&1)"
    if [ "$got" = "300 240 {'linux': '1234', 'macos': None} {'plan': (5, 25), 'chromium-1-1': (20, 165)}" ]; then
        pass "the record holds both spans, each platform's engine and every job that ran"
    else
        fail "record: $got"
    fi
    if [ "$(wc -l <"$log" | tr -d ' ')" = 1 ] \
        && [[ "$out" == *"push to verdict 5m00s, run finished at 4m00s, linux engine reused from run 1234; macos engine built, slowest job chromium-1-1 2m45s"* ]]; then
        pass "one log line, and one summary line"
    else
        fail "log $(wc -l <"$log" 2>/dev/null), summary: $out"
    fi
    out="$(LUCIDOS_E2E_TIMINGS_LOG="$log" e2e_github_record_timings "$dir" 4242 browser 1767225600 0 'not json' 2>&1)"
    if [[ "$out" == *"timings not recorded"* ]]; then pass "a broken state is one line, not a failure"; else fail "broken state: $out"; fi
}

# detach_outcome <run-db-id>: what the driver's real SIGUSR2 trap leaves.
detach_outcome() {
    bash -c 'source "$1"; run_db_id="$2"; detached=""
        trap "echo detached=\${detached:-no}" EXIT
        e2e_github_trap_detach
        kill -USR2 $$
        sleep 5' _ "$SCRIPT_DIR/e2e_github.sh" "$1"
    echo "exit=$?"
}

test_a_detach_leaves_a_found_run_going_and_cancels_one_not_found() {
    echo "test: SIGUSR2 detaches from a found run, and cancels before one is found"
    local got
    got="$(detach_outcome 123 | tr '\n' ' ')"
    if [ "$got" = "detached=1 exit=$E2E_GITHUB_NO_VERDICT_EXIT " ]; then pass "a found run is left for --attach"; else fail "found run: $got"; fi
    got="$(detach_outcome "" | tr '\n' ' ')"
    if [ "$got" = "detached=no exit=$E2E_GITHUB_NO_VERDICT_EXIT " ]; then pass "no run yet: the cleanup cancels"; else fail "no run: $got"; fi
}

test_a_second_download_replaces_a_partial_one() {
    echo "test: a reattached driver downloads over what a detached one left"
    local dir="$SANDBOX/redownload" rc
    rm -rf "$dir"
    mkdir -p "$dir/e2e-out-api" "$dir/e2e-plan"
    echo stale >"$dir/e2e-out-api/api.rc"
    echo kept >"$dir/e2e-request.json"
    # Like the real gh: it refuses to extract over a file it finds.
    gh() {
        [ ! -e "$dir/e2e-out-api/api.rc" ] || return 1
        mkdir -p "$dir/e2e-out-api" && echo 0 >"$dir/e2e-out-api/api.rc"
    }
    e2e_github_download 1 "$dir"
    rc=$?
    unset -f gh
    if [ "$rc" -eq 0 ] && [ "$(cat "$dir/e2e-out-api/api.rc")" = 0 ] && [ -f "$dir/e2e-request.json" ]; then
        pass "the shard output is replaced, and the request stays"
    else
        fail "rc $rc, api.rc '$(cat "$dir/e2e-out-api/api.rc" 2>/dev/null)'"
    fi
}

test_only_a_coverage_request_turns_coverage_on() {
    echo "test: a shard turns coverage on only when the request asks"
    local shard="$SCRIPT_DIR/../e2e-github-shard.sh" req="$SANDBOX/coverage-request.json" got
    echo '{"coverage": true, "suites": []}' >"$req"
    got="$(E2E_GITHUB_REQUEST="$req" bash -c 'source "$1"; export_coverage_request; echo "${LUCIDOS_E2E_COVERAGE:-off}"' _ "$shard")"
    if [ "$got" = 1 ]; then pass "a coverage request exports it"; else fail "a coverage request gave '$got'"; fi
    for body in '{"coverage": false, "suites": []}' '{"suites": []}'; do
        echo "$body" >"$req"
        got="$(E2E_GITHUB_REQUEST="$req" LUCIDOS_E2E_COVERAGE='' bash -c 'source "$1"; export_coverage_request; echo "${LUCIDOS_E2E_COVERAGE:-off}"' _ "$shard")"
        if [ "$got" = off ]; then pass "$body leaves it off"; else fail "$body gave '$got'"; fi
    done
}

test_every_suite_sees_the_coverage_request() {
    echo "test: a non-browser suite runs with coverage on when the request asks"
    local box="$SANDBOX/cov-suite" got
    mkdir -p "$box/scripts/lib"
    cp "$SCRIPT_DIR/../e2e-github-shard.sh" "$box/scripts/"
    cp "$SCRIPT_DIR/e2e_flaky.sh" "$box/scripts/lib/"
    cp "$SCRIPT_DIR/e2e_warm.sh" "$box/scripts/lib/"
    cat >"$box/scripts/e2e-api.sh" <<'STUB'
#!/bin/bash
echo "coverage=${LUCIDOS_E2E_COVERAGE:-off}"
STUB
    chmod +x "$box/scripts/e2e-api.sh"
    echo '{"coverage": true, "suites": [], "api_args": []}' >"$box/request.json"
    got="$(E2E_GITHUB_REQUEST="$box/request.json" E2E_GITHUB_OUT="$box/out" LUCIDOS_E2E_COVERAGE='' \
        bash "$box/scripts/e2e-github-shard.sh" run api-1 api 2>&1)"
    if [[ "$got" == *coverage=1* ]]; then pass "the api suite sees coverage"; else fail "api suite output: $got"; fi
}

# fake_coverage <dir> <shard-id> <spec> <file...>: one recorded coverage file.
fake_coverage() {
    local dir="$1" id="$2" spec="$3"
    shift 3
    mkdir -p "$dir/e2e-out-$id/coverage"
    python3 -c 'import json, sys; json.dump({"spec": sys.argv[1], "project": "p", "files": sys.argv[2:]}, sys.stdout)' \
        "$spec" "$@" >"$dir/e2e-out-$id/coverage/$spec@p@w0.json"
}

test_the_coverage_map_is_replaced_only_when_every_chromium_shard_reported() {
    echo "test: the main check replaces the coverage map only from a complete run"
    local repo="$SANDBOX/coverage-repo" dir="$SANDBOX/coverage-run" map out
    rm -rf "$repo" "$dir"
    git init -q "$repo"
    map="$(ts_coverage_map_path "$repo")"
    mkdir -p "$dir/e2e-plan"
    printf '%s\n' "chromium-1-2 browser chromium 1 2" "chromium-2-2 browser chromium 2 2" \
        "mobile-1-1 browser mobile 1 1" "mobile-webkit-1-1 browser mobile-webkit 1 1" "api api - 1 1" \
        >"$dir/e2e-plan/entries"
    fake_coverage "$dir" chromium-1-2 chat.spec.ts crates/lucidos-app/src/a.ts
    fake_coverage "$dir" mobile-1-1 chat-all.spec.ts crates/lucidos-app/src/b.ts
    out="$(e2e_github_record_coverage_map "$repo" "$dir" abc123 https://example.test/run/1 2>&1)"
    if [ ! -e "$map" ] && [[ "$out" == *"chromium-2-2"* ]]; then
        pass "a Chromium shard with no coverage leaves no map, and says which"
    else
        fail "map exists: $([ -e "$map" ] && echo yes || echo no); said: $out"
    fi
    fake_coverage "$dir" chromium-2-2 chat.spec.ts crates/lucidos-app/src/c.ts
    e2e_github_record_coverage_map "$repo" "$dir" abc123 https://example.test/run/1 2>/dev/null
    out="$(python3 -c 'import json, sys; m = json.load(open(sys.argv[1])); print(m["commit"], json.dumps(m["specs"], sort_keys=True))' "$map" 2>&1)"
    local want='abc123 {"chat-all.spec.ts": ["crates/lucidos-app/src/b.ts"], "chat.spec.ts": ["crates/lucidos-app/src/a.ts", "crates/lucidos-app/src/c.ts"]}'
    if [ "$out" = "$want" ]; then pass "every Chromium shard reported: the map is their union"; else fail "map: $out"; fi
    echo 'not json' >"$dir/e2e-out-chromium-2-2/coverage/broken.json"
    e2e_github_record_coverage_map "$repo" "$dir" def456 https://example.test/run/2 2>/dev/null
    if grep -q abc123 "$map"; then
        pass "an unreadable coverage file keeps the previous map"
    else
        fail "the map moved on an unreadable file: $(cat "$map")"
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
    for i in $(seq 1 17); do specs="$specs\"s$i-all.spec.ts\","; done
    echo "{\"suites\": [\"api\", \"browser\"], \"api_args\": [], \"browser\": {\"projects\": [\"chromium\", \"mobile\", \"mobile-webkit\"], \"specs\": [${specs}\"only-desktop.spec.ts\", \"plain.spec.ts\", \"only-mobile.spec.ts\"], \"pw_args\": []}}" >"$req"
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
    # chromium runs 17 + desktop + plain = 19 specs, so 3 shards; mobile runs 17 + mobile = 18, so 3.
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
    for spec in chat coding-agent-cancel coding-agent-resume-after-exit side-question-all; do
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
    e2e_github_is_spec_arg "e2e/chat.spec.ts" "e2e/side-question-all.spec.ts" || wrong="$wrong path-after-spec"
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

# A stub of GitHub's artifact API for the engine lookup. ARTIFACT_FIXTURE holds
# one "<id> <run id> <name>" line per artifact, as list_artifacts's --jq prints
# them, and $ARTIFACT_DIR/<id>/manifest is what artifact <id> unzips to.
# shellcheck disable=SC2016 # the stub is source for the child bash
GITHUB_API_STUB='
github_api() {
    case "$1" in
        *"actions/artifacts?"*"page=1") cat "$ARTIFACT_FIXTURE" ;;
        *"actions/artifacts?"*) ;;
        */zip) local id="${1%/zip}"; id="${id##*/}"
            python3 -c "import io, sys, zipfile
b = io.BytesIO(); z = zipfile.ZipFile(b, \"w\"); z.write(sys.argv[1], \"manifest\"); z.close()
sys.stdout.buffer.write(b.getvalue())" "$ARTIFACT_DIR/$id/manifest" ;;
        *) return 1 ;;
    esac
}'

# A manifest naming one real input of this checkout, Cargo.lock, at <blob>.
lookup_manifest() { # <dir> <blob>
    mkdir -p "$1"
    printf 'lucidos-engine-input-manifest\t1\nkey\tprofile\te2e\nkey\tfeatures\te2e-test-hooks\nkey\tplatform\tLinux-x86_64\nkey\trustflags\t-\nkey\ttoolchain-override\t-\nfile\t%s\tCargo.lock\n' \
        "$2" >"$1/manifest"
}

test_the_engine_lookup_names_the_newest_matching_build() {
    echo "test: the plan's engine lookup names the newest build whose manifest matches the tree"
    local shard="$SCRIPT_DIR/../e2e-github-shard.sh" dir="$SANDBOX/artifacts" ghout="$SANDBOX/lookup-out" prekey real
    prekey="$(bash -c 'source "$1"; source "$2"; engine_prekey Linux-x86_64' _ "$shard" "$SCRIPT_DIR/e2e.sh")"
    real="$(git -C "$SCRIPT_DIR/../.." hash-object Cargo.lock)"
    lookup_manifest "$dir/30" 0000000000000000000000000000000000000000
    lookup_manifest "$dir/20" "$real"
    lookup_manifest "$dir/10" "$real"
    lookup_manifest "$dir/5" "$real"
    printf '%s\n' "40 400 e2e-target-$prekey-400" "35 350 e2e-target-000000000000-350" \
        "30 300 e2e-manifest-$prekey-300" "20 200 e2e-manifest-000000000000-200" \
        "10 100 e2e-manifest-$prekey-100" "5 50 e2e-manifest-$prekey-50" "4 40 e2e-target-$prekey-40" \
        >"$SANDBOX/artifact-list"
    : >"$ghout"
    rm -rf "$SANDBOX/lookup-plan"
    ARTIFACT_FIXTURE="$SANDBOX/artifact-list" ARTIFACT_DIR="$dir" GITHUB_OUTPUT="$ghout" GITHUB_REPOSITORY=example-org/example-repo \
        E2E_GITHUB_OUT="$SANDBOX/lookup-plan" \
        bash -c 'source "$1"; eval "$2"; cmd_find_engine linux' _ "$shard" "$GITHUB_API_STUB" >/dev/null 2>&1
    if grep -qx 'linux_engine_run=100' "$ghout" && grep -qx "linux_engine_artifact=e2e-engine-$prekey-100" "$ghout"; then
        pass "a mismatch and another configuration are skipped, the newest match wins"
    else
        fail "lookup wrote: $(tr '\n' ';' <"$ghout")"
    fi
    if grep -qx 'linux_target_run=400' "$ghout" && grep -qx "linux_target_artifact=e2e-target-$prekey-400" "$ghout"; then
        pass "the newest target/ state of this configuration is named"
    else
        fail "lookup wrote: $(tr '\n' ';' <"$ghout")"
    fi
    if [ "$(cat "$SANDBOX/lookup-plan/plan/engines" 2>/dev/null)" = "linux 100" ]; then
        pass "the plan artifact records which run's engine is reused"
    else
        fail "engines file: $(cat "$SANDBOX/lookup-plan/plan/engines" 2>&1)"
    fi
}

test_restored_target_state_ages_its_unchanged_inputs() {
    echo "test: a build restores the target/ state it was handed and ages from its manifest"
    local shard="$SCRIPT_DIR/../e2e-github-shard.sh" root="$SANDBOX/state-root" out
    mkdir -p "$root/target/e2e"
    printf 'lucidos-engine-input-manifest\t1\n' >"$root/target/e2e/lucidos-input-manifest"
    echo artefact >"$root/target/e2e/libx.rlib"
    mkdir -p "$SANDBOX/state-in"
    tar -C "$root" -cf - target | zstd -q -o "$SANDBOX/state-in/e2e-target.tar.zst"
    out="$(bash -c 'source "$1"; PROJECT_DIR="$2"; E2E_GITHUB_CARGO_PROFILE=e2e
        engine_manifest_stamp_mtimes() { echo "stamp $1 $2"; }
        mkdir -p "$2"; restore_target_state "$3"; restore_target_state "$4"' \
        _ "$shard" "$SANDBOX/state-checkout" "$SANDBOX/state-in" "$SANDBOX/no-state")"
    if [ -f "$SANDBOX/state-checkout/target/e2e/libx.rlib" ] \
        && [[ "$out" == *"stamp $SANDBOX/state-checkout $SANDBOX/state-checkout/target/e2e/lucidos-input-manifest"* ]] \
        && [[ "$out" == *"none arrived, compiling from scratch"* ]]; then
        pass "the state is unpacked and aged, and a missing one compiles from scratch"
    else
        fail "restore printed: $out"
    fi
}

test_a_failed_lookup_lets_the_build_run() {
    echo "test: an engine lookup that cannot reach GitHub names nothing and does not fail"
    local shard="$SCRIPT_DIR/../e2e-github-shard.sh" ghout="$SANDBOX/lookup-fail-out" rc=0
    : >"$ghout"
    rm -rf "$SANDBOX/lookup-fail-plan"
    GITHUB_OUTPUT="$ghout" GITHUB_REPOSITORY=example-org/example-repo E2E_GITHUB_OUT="$SANDBOX/lookup-fail-plan" \
        bash -c 'source "$1"; github_api() { return 1; }; cmd_find_engine macos' _ "$shard" >/dev/null 2>&1 || rc=$?
    if [ "$rc" -eq 0 ] && grep -qx 'macos_engine_run=' "$ghout" \
        && [ "$(cat "$SANDBOX/lookup-fail-plan/plan/engines" 2>/dev/null)" = "macos -" ]; then
        pass "no engine named, exit 0, so the build job runs"
    else
        fail "rc=$rc, lookup wrote: $(tr '\n' ';' <"$ghout")"
    fi
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

test_each_job_warms_only_what_it_needs() {
    echo "test: the warm-up starts the Postgres image and apt only on Linux, and a browser only for a browser shard"
    local shard="$SCRIPT_DIR/../e2e-github-shard.sh" got
    warm_tasks() { # <os> <suite> [<project>]
        RUNNER_TEMP="$SANDBOX/warm-runner" GITHUB_ENV="$SANDBOX/warm-env" bash -c '
            source "$1"; os="$2"; uname() { echo "$os"; }
            e2e_warm_start() { printf "%s " "$1"; }
            cmd_warm "$3" "$4"' _ "$shard" "$@" 2>/dev/null
    }
    got="$(warm_tasks Linux browser chromium)"
    if [ "$got" = "pg-image apt npm gcloud browsers " ]; then pass "a Linux browser shard: $got"; else fail "Linux browser started '$got'"; fi
    got="$(warm_tasks Linux api)"
    if [ "$got" = "pg-image apt npm gcloud " ]; then pass "a Linux API job starts no browser"; else fail "Linux api started '$got'"; fi
    got="$(warm_tasks Darwin browser mobile-webkit)"
    if [ "$got" = "npm gcloud browsers " ]; then pass "a macOS shard starts no Docker and no apt"; else fail "macOS started '$got'"; fi
    if grep -qx "LUCIDOS_E2E_WARM_DIR=$SANDBOX/warm-runner/e2e-warm" "$SANDBOX/warm-env"; then
        pass "later steps learn the warm-up directory"
    else
        fail "GITHUB_ENV holds: $(cat "$SANDBOX/warm-env" 2>/dev/null)"
    fi
}

test_the_apt_warm_up_drops_the_microsoft_source() {
    echo "test: the apt warm-up removes the Microsoft source before any apt-get update"
    local shard="$SCRIPT_DIR/../e2e-github-shard.sh" dir="$SANDBOX/apt-sources" log="$SANDBOX/apt-order"
    mkdir -p "$dir"
    : >"$dir/microsoft-prod.list"
    : >"$dir/other.list"
    rm -f "$log"
    APT_SOURCES_DIR="$dir" APT_ORDER_LOG="$log" bash -c '
        source "$1"; sudo() { if [ "$1" = apt-get ]; then ls "$APT_SOURCES_DIR" | tr "\n" " " >>"$APT_ORDER_LOG"; else "$@"; fi; }
        warm_apt api' _ "$shard" >/dev/null 2>&1
    if [ "$(cat "$log" 2>/dev/null)" = "other.list other.list " ] && [ ! -e "$dir/microsoft-prod.list" ]; then
        pass "apt-get ran with only other.list left"
    else
        fail "apt-get saw '$(cat "$log" 2>/dev/null)'"
    fi
}

test_a_shard_installs_only_its_own_browser() {
    echo "test: Chromium shards install the headless shell alone, and the WebKit shard WebKit alone"
    local shard="$SCRIPT_DIR/../e2e-github-shard.sh" got
    browsers_for() {
        bash -c 'source "$1"; e2e_warm_wait() { :; }; npx() { echo "$*"; }; warm_browsers "$2"' _ "$shard" "$1"
    }
    got="$(browsers_for chromium)"
    if [ "$got" = "playwright install --only-shell chromium" ]; then pass "chromium: $got"; else fail "chromium installed '$got'"; fi
    got="$(browsers_for mobile)"
    if [ "$got" = "playwright install --only-shell chromium" ]; then pass "mobile: $got"; else fail "mobile installed '$got'"; fi
    got="$(browsers_for mobile-webkit)"
    if [ "$got" = "playwright install webkit" ]; then pass "mobile-webkit: $got"; else fail "mobile-webkit installed '$got'"; fi
}

test_the_macos_cache_keys_name_the_pinned_builds() {
    echo "test: the macOS cache keys carry the pinned Postgres, pgvector and Playwright versions"
    local shard="$SCRIPT_DIR/../e2e-github-shard.sh" out pg pgv pw
    out="$(GITHUB_OUTPUT=/dev/stdout bash -c 'source "$1"; cmd_cache_keys' _ "$shard")"
    pg="$(bash -c 'source "$1"; pinned_version PG_VERSION' _ "$shard")"
    pgv="$(bash -c 'source "$1"; pinned_version PGVECTOR_VERSION' _ "$shard")"
    pw="$(bash -c 'source "$1"; locked_playwright_version' _ "$shard")"
    if [[ "$out" == *"postgres_dir=postgresql-$pg-aarch64-apple-darwin"* ]] \
        && [[ "$out" == *"postgres=postgresql-$pg-aarch64-apple-darwin-pgvector-$pgv"* ]] \
        && [[ "$out" == *"playwright=$pw"* ]] && [ -n "$pw" ]; then
        pass "keys: $(printf '%s' "$out" | tr '\n' ' ')"
    else
        fail "keys were: $out"
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
test_a_filtered_run_stays_here_and_runs_lean
test_an_unfiltered_run_still_goes_to_github
test_the_unavailable_reason_names_each_precondition
test_the_main_check_selection_has_no_fallback
test_attach_takes_a_run_and_its_branch_for_the_main_check_only
test_the_report_matches_the_local_tally_and_exit_codes
test_the_report_counts_the_engine_suite
test_a_refused_build_is_no_verdict_and_a_failure_outranks_it
test_the_run_state_reads_both_answers_and_the_rate_limit
test_the_poll_speeds_up_once_a_test_job_starts
test_the_wait_gives_up_with_no_verdict
test_the_wait_polls_its_own_run_adaptively
test_the_timings_record_reuse_and_job_times
test_a_detach_leaves_a_found_run_going_and_cancels_one_not_found
test_a_second_download_replaces_a_partial_one
test_only_a_coverage_request_turns_coverage_on
test_every_suite_sees_the_coverage_request
test_the_coverage_map_is_replaced_only_when_every_chromium_shard_reported
test_the_plan_gives_the_engine_suite_its_own_job
test_the_plan_puts_webkit_on_macos_and_respects_test_ignore
test_only_tagged_specs_stay_on_the_local_leg
test_a_named_commit_routes_the_specs_it_holds
test_a_spec_file_among_playwright_args_is_a_selection
test_the_plan_refuses_a_real_claude_code_spec
test_the_fake_installs_only_on_a_runner_and_never_over_a_claude
test_the_runner_checkout_gets_a_main_and_an_existing_one_never_moves
test_the_pinned_postgres_versions_resolve
test_the_engine_lookup_names_the_newest_matching_build
test_a_failed_lookup_lets_the_build_run
test_restored_target_state_ages_its_unchanged_inputs
test_each_job_warms_only_what_it_needs
test_the_apt_warm_up_drops_the_microsoft_source
test_a_shard_installs_only_its_own_browser
test_the_macos_cache_keys_name_the_pinned_builds

echo ""
echo "Passed: $PASS  Failed: $FAIL"
[ "$FAIL" -eq 0 ]
