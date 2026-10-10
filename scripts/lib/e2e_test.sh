#!/bin/bash
# Tests for scripts/lib/e2e.sh helpers.
# Run: ./scripts/lib/e2e_test.sh
set -u

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"

SANDBOX="$(mktemp -d)"
trap 'rm -rf "$SANDBOX"' EXIT

PASS=0
FAIL=0
fail() { echo "  FAIL: $*"; FAIL=$((FAIL+1)); }
pass() { echo "  ok:   $*"; PASS=$((PASS+1)); }

# Source via a fake E2E_WORKSPACE so the lib doesn't try to touch the real one.
export E2E_WORKSPACE="$SANDBOX/e2e-test"
mkdir -p "$E2E_WORKSPACE/.lucidos/worktrees"

# shellcheck source=e2e.sh
source "$SCRIPT_DIR/e2e.sh"

# ── prune_orphan_worktree_dirs ────────────────────────────────────────
test_prune_removes_empty_dir() {
    echo "test: prune_orphan_worktree_dirs removes empty dirs"
    local wt_root="$E2E_WORKSPACE/.lucidos/worktrees"
    rm -rf "${wt_root:?}"/*
    mkdir -p "$wt_root/empty-orphan"

    prune_orphan_worktree_dirs >/dev/null 2>&1

    if [ -d "$wt_root/empty-orphan" ]; then
        fail "empty orphan dir not removed"
    else
        pass "empty orphan dir removed"
    fi
}

test_prune_removes_dir_with_dangling_gitdir() {
    echo "test: prune_orphan_worktree_dirs removes dirs with dangling .git pointer"
    local wt_root="$E2E_WORKSPACE/.lucidos/worktrees"
    rm -rf "${wt_root:?}"/*
    mkdir -p "$wt_root/dangling-orphan"
    echo "Cargo.lock" > "$wt_root/dangling-orphan/Cargo.lock"
    echo "gitdir: $SANDBOX/does-not-exist/.git/worktrees/x" > "$wt_root/dangling-orphan/.git"

    prune_orphan_worktree_dirs >/dev/null 2>&1

    if [ -d "$wt_root/dangling-orphan" ]; then
        fail "dangling-pointer orphan dir not removed"
    else
        pass "dangling-pointer orphan dir removed"
    fi
}

test_prune_keeps_live_worktree() {
    echo "test: prune_orphan_worktree_dirs keeps live worktrees"
    local wt_root="$E2E_WORKSPACE/.lucidos/worktrees"
    local fake_repo_gitdir="$SANDBOX/repo/.git/worktrees/live"
    rm -rf "${wt_root:?}"/* "$SANDBOX/repo"
    mkdir -p "$wt_root/live-worktree" "$fake_repo_gitdir"
    echo "src" > "$wt_root/live-worktree/src.txt"
    echo "gitdir: $fake_repo_gitdir" > "$wt_root/live-worktree/.git"

    prune_orphan_worktree_dirs >/dev/null 2>&1

    if [ -d "$wt_root/live-worktree" ]; then
        pass "live worktree preserved"
    else
        fail "live worktree was removed"
    fi
}

test_prune_keeps_dir_without_git_pointer() {
    echo "test: prune_orphan_worktree_dirs keeps non-empty dirs without .git pointer"
    # A non-empty dir without a .git pointer is not necessarily an orphan
    # worktree — could be unrelated state. Don't touch it.
    local wt_root="$E2E_WORKSPACE/.lucidos/worktrees"
    rm -rf "${wt_root:?}"/*
    mkdir -p "$wt_root/random-stuff"
    echo "data" > "$wt_root/random-stuff/file.txt"

    prune_orphan_worktree_dirs >/dev/null 2>&1

    if [ -d "$wt_root/random-stuff" ]; then
        pass "non-worktree dir preserved"
    else
        fail "non-worktree dir was removed"
    fi
}

test_prune_handles_missing_root() {
    echo "test: prune_orphan_worktree_dirs is a no-op when worktree root missing"
    rm -rf "$E2E_WORKSPACE/.lucidos/worktrees"

    if prune_orphan_worktree_dirs >/dev/null 2>&1; then
        pass "exited cleanly with no worktree root"
    else
        fail "errored on missing worktree root"
    fi

    # Recreate for any further tests.
    mkdir -p "$E2E_WORKSPACE/.lucidos/worktrees"
}

# ── cleanup_e2e_worktrees (shared-repo branch) ────────────────────────
# The dangerous half of cleanup runs against $_E2E_PROJECT_DIR — the canonical
# lucidos checkout, shared with every real CC session. Point it at a sandbox
# repo so the test never touches the real one, then prove cleanup removes only
# the e2e-created worktree (path under $E2E_WORKSPACE) and its branch, while
# sparing real CC sessions — including an ancestor-of-main branch with no
# commits yet, the exact shape the old ancestry sweep force-deleted (2026-06-13).
test_cleanup_spares_real_cc_sessions() {
    echo "test: cleanup_e2e_worktrees removes e2e worktrees but spares real CC sessions"
    local canon="$SANDBOX/canonical"
    local dev="$SANDBOX/dev-ws"
    rm -rf "$canon" "$dev" "$E2E_WORKSPACE/.lucidos/worktrees"
    mkdir -p "$canon" "$E2E_WORKSPACE/.lucidos/worktrees"

    git init -q -b main "$canon"
    git -C "$canon" config user.email e2e@test
    git -C "$canon" config user.name e2e
    git -C "$canon" commit -q --allow-empty -m init

    # e2e CC test worktree: lives under $E2E_WORKSPACE, registered in canonical.
    git -C "$canon" worktree add -q -b lucidos-claude-code-repo-lucidos-e2e-fake \
        "$E2E_WORKSPACE/.lucidos/worktrees/e2e-cc" main >/dev/null 2>&1
    # Real CC session worktree: lives in a different workspace, on an
    # ancestor-of-main branch (just started, nothing committed yet). Named with
    # the current `lucidos-*` prefix, which the disposable-workspace sweep DOES
    # match by name, so this pins that the shared-repo half still discriminates
    # by path.
    git -C "$canon" worktree add -q -b lucidos-codex-repo-lucidos-real-live \
        "$dev/.lucidos/worktrees/real-cc" main >/dev/null 2>&1
    # Real CC session branch with NO worktree, also ancestor-of-main — exactly
    # what the old `for-each-ref … merge-base --is-ancestor … branch -D` deleted.
    # Legacy prefix, so the sweep is pinned against both branch shapes.
    git -C "$canon" branch claude-code/real-untracked main

    local saved_proj="$_E2E_PROJECT_DIR"
    _E2E_PROJECT_DIR="$canon"
    cleanup_e2e_worktrees >/dev/null 2>&1
    _E2E_PROJECT_DIR="$saved_proj"

    local wts
    wts="$(git -C "$canon" worktree list --porcelain 2>/dev/null)"

    case "$wts" in
        *"$E2E_WORKSPACE/.lucidos/worktrees/e2e-cc"*) fail "e2e worktree not removed" ;;
        *) pass "e2e worktree removed" ;;
    esac
    if git -C "$canon" show-ref --verify --quiet refs/heads/lucidos-claude-code-repo-lucidos-e2e-fake; then
        fail "e2e branch not deleted"
    else
        pass "e2e branch deleted"
    fi

    case "$wts" in
        *"$dev/.lucidos/worktrees/real-cc"*) pass "real session worktree preserved" ;;
        *) fail "real session worktree was removed" ;;
    esac
    if git -C "$canon" show-ref --verify --quiet refs/heads/lucidos-codex-repo-lucidos-real-live; then
        pass "real session branch (live worktree) preserved"
    else
        fail "real session branch (live worktree) was deleted"
    fi
    if git -C "$canon" show-ref --verify --quiet refs/heads/claude-code/real-untracked; then
        pass "real ancestor-of-main branch preserved (regression)"
    else
        fail "real ancestor-of-main branch was deleted (regression!)"
    fi
}

# A run killed between the fresh-workspace reset and the engine boot leaves the
# workspace with no repo. The next run's cleanup must get through that under the
# scripts' `set -e`, and git must not resolve some ENCLOSING repo instead.
test_cleanup_survives_a_workspace_with_no_repo() {
    echo "test: cleanup_e2e_worktrees passes over a workspace that has no repo yet"
    local outer="$SANDBOX/outer" canon="$SANDBOX/canonical-norepo" rc
    rm -rf "$outer" "$canon" "$SANDBOX/bare-ws"
    git init -q -b main "$outer"
    git -C "$outer" config user.email e2e@test
    git -C "$outer" config user.name e2e
    git -C "$outer" commit -q --allow-empty -m init
    git -C "$outer" worktree add -q -b lucidos-outer-live "$SANDBOX/outer-wt" main >/dev/null 2>&1
    git init -q -b main "$canon"
    local saved_proj="$_E2E_PROJECT_DIR"
    _E2E_PROJECT_DIR="$canon"

    # Never `( … ) || rc=$?`: a tested subshell runs with set -e switched off.
    local E2E_WORKSPACE="$SANDBOX/bare-ws/e2e-test"
    mkdir -p "$E2E_WORKSPACE/.lucidos/worktrees"
    ( set -e; cleanup_e2e_worktrees ) >/dev/null 2>&1
    rc=$?
    if [ "$rc" -eq 0 ]; then pass "it returns 0 under set -e"; else fail "it exited $rc"; fi

    E2E_WORKSPACE="$outer/e2e-test"
    mkdir -p "$E2E_WORKSPACE/.lucidos/worktrees"
    ( set -e; cleanup_e2e_worktrees ) >/dev/null 2>&1
    _E2E_PROJECT_DIR="$saved_proj"
    if git -C "$outer" show-ref --verify --quiet refs/heads/lucidos-outer-live \
        && git -C "$outer" worktree list | grep -q "outer-wt"; then
        pass "the enclosing repo keeps its worktree and branch"
    else
        fail "the cleanup swept the enclosing repo"
    fi
}

# The other side of that guard: once the workspace IS a repo, the sweep runs.
# The sandbox sits under a symlinked temp dir, so this also pins that git's
# top-level path and `pwd -P` resolve alike.
test_cleanup_sweeps_the_workspace_repo() {
    echo "test: cleanup_e2e_worktrees sweeps the workspace's own repo"
    local E2E_WORKSPACE="$SANDBOX/own-repo/e2e-test" canon="$SANDBOX/canonical-own"
    rm -rf "$SANDBOX/own-repo" "$canon" "$SANDBOX/own-wt"
    mkdir -p "$E2E_WORKSPACE/.lucidos/worktrees"
    git init -q -b main "$E2E_WORKSPACE"
    git -C "$E2E_WORKSPACE" config user.email e2e@test
    git -C "$E2E_WORKSPACE" config user.name e2e
    git -C "$E2E_WORKSPACE" commit -q --allow-empty -m init
    git -C "$E2E_WORKSPACE" worktree add -q -b lucidos-e2e-leftover "$SANDBOX/own-wt" main >/dev/null 2>&1
    git init -q -b main "$canon"
    local saved_proj="$_E2E_PROJECT_DIR"
    _E2E_PROJECT_DIR="$canon"
    ( set -e; cleanup_e2e_worktrees ) >/dev/null 2>&1
    _E2E_PROJECT_DIR="$saved_proj"
    if git -C "$E2E_WORKSPACE" worktree list | grep -q "own-wt"; then
        fail "the leftover worktree survived"
    else
        pass "the leftover worktree is removed"
    fi
    if git -C "$E2E_WORKSPACE" show-ref --verify --quiet refs/heads/lucidos-e2e-leftover; then
        fail "the leftover branch survived"
    else
        pass "the leftover branch is deleted"
    fi
}

# ── ensure_frontend_built (stale dist/ guard) ─────────────────────────
# The browser suite runs against whatever dist/ is on disk, so the old
# existence-only guard let a checkout whose dist/ predated its own frontend
# commits report GREEN against a stale frontend. These cover all three branches:
# missing dist → rebuild, stale dist → rebuild, fresh dist → reuse.
#
# The real `npx vite build` is swapped for a stub that counts calls and refreshes
# dist/index.html (the repo's seam convention — see host_load_guard.sh), so each
# case costs milliseconds while still exercising the real decision.
FE_ROOT=""
BUILD_CALLS=0
FE_OUT=""

_run_vite_build() {
    BUILD_CALLS=$((BUILD_CALLS + 1))
    mkdir -p "$FRONTEND_DIR/dist"
    : > "$FRONTEND_DIR/dist/index.html"
}

# Build a throwaway checkout holding every path _frontend_build_inputs names, and
# point the lib's two path globals at it.
setup_frontend_sandbox() {
    FE_ROOT="$SANDBOX/fe/$1"
    rm -rf "$FE_ROOT"
    FRONTEND_DIR="$FE_ROOT/crates/lucidos-app"
    _E2E_PROJECT_DIR="$FE_ROOT"
    mkdir -p "$FRONTEND_DIR/src" "$FRONTEND_DIR/public" "$FRONTEND_DIR/dist" \
        "$FE_ROOT/packages/lucidos-sdk/src" "$FE_ROOT/crates/lucidos-engine"
    : > "$FRONTEND_DIR/index.html"
    : > "$FRONTEND_DIR/vite.config.ts"
    : > "$FRONTEND_DIR/tsconfig.json"
    : > "$FRONTEND_DIR/package.json"
    : > "$FRONTEND_DIR/src/main.tsx"
    : > "$FRONTEND_DIR/public/sw.js"
    : > "$FE_ROOT/packages/lucidos-sdk/src/index.ts"
    : > "$FE_ROOT/crates/lucidos-engine/VERSION"
    : > "$FE_ROOT/package.json"
    : > "$FE_ROOT/package-lock.json"
    : > "$FRONTEND_DIR/dist/index.html"
    BUILD_CALLS=0
    FE_OUT="$SANDBOX/fe-$1.out"
}

# Stamp every build input (directories included — a directory's own mtime moves
# when its entries change) to an absolute timestamp, so the ordering under test
# is explicit instead of racing the filesystem clock.
touch_build_inputs() {
    local ts="$1" p
    while IFS= read -r p; do
        [ -e "$p" ] || continue
        find "$p" -exec touch -t "$ts" {} +
    done < <(_frontend_build_inputs)
}

test_frontend_build_missing_dist_rebuilds() {
    echo "test: ensure_frontend_built rebuilds when dist/index.html is missing"
    local saved_fe="$FRONTEND_DIR" saved_proj="$_E2E_PROJECT_DIR"
    setup_frontend_sandbox missing
    rm -rf "$FRONTEND_DIR/dist"

    ensure_frontend_built >"$FE_OUT" 2>&1
    local rc=$?
    FRONTEND_DIR="$saved_fe"; _E2E_PROJECT_DIR="$saved_proj"

    if [ "$rc" -eq 0 ]; then pass "returned 0"; else fail "returned $rc"; fi
    if [ "$BUILD_CALLS" -eq 1 ]; then
        pass "missing dist triggered exactly one build"
    else
        fail "missing dist ran $BUILD_CALLS builds (expected 1)"
    fi
    if grep -q "REBUILDING dist/ (stale — no dist/index.html)" "$FE_OUT"; then
        pass "logged the REBUILDING branch and why"
    else
        fail "did not log the REBUILDING branch"; cat "$FE_OUT"
    fi
}

test_frontend_build_stale_dist_rebuilds() {
    echo "test: ensure_frontend_built rebuilds when dist/ is older than a source input"
    local saved_fe="$FRONTEND_DIR" saved_proj="$_E2E_PROJECT_DIR"
    setup_frontend_sandbox stale
    # dist/ built first, source moved forward afterwards — exactly the shape a
    # checkout has when its committed dist/ predates its frontend commits.
    touch -t 202601010000 "$FRONTEND_DIR/dist/index.html"
    touch_build_inputs 202601020000

    ensure_frontend_built >"$FE_OUT" 2>&1
    FRONTEND_DIR="$saved_fe"; _E2E_PROJECT_DIR="$saved_proj"

    if [ "$BUILD_CALLS" -eq 1 ]; then
        pass "stale dist triggered a rebuild"
    else
        fail "stale dist ran $BUILD_CALLS builds (expected 1) — a stale frontend would have been tested"
    fi
    if grep -q "REBUILDING dist/ (stale — build input newer than dist/index.html: " "$FE_OUT"; then
        pass "logged which input made it stale"
    else
        fail "did not name the newer input"; cat "$FE_OUT"
    fi
}

test_frontend_build_stale_via_workspace_local_sdk() {
    echo "test: ensure_frontend_built treats the aliased @lucidos/sdk source as a build input"
    local saved_fe="$FRONTEND_DIR" saved_proj="$_E2E_PROJECT_DIR"
    setup_frontend_sandbox sdk
    touch_build_inputs 202601010000
    touch -t 202601020000 "$FRONTEND_DIR/dist/index.html"
    # Only the workspace-local package moves — the app tree stays untouched.
    touch -t 202601030000 "$FE_ROOT/packages/lucidos-sdk/src/index.ts"

    ensure_frontend_built >"$FE_OUT" 2>&1
    FRONTEND_DIR="$saved_fe"; _E2E_PROJECT_DIR="$saved_proj"

    if [ "$BUILD_CALLS" -eq 1 ]; then
        pass "a newer SDK source triggered a rebuild"
    else
        fail "SDK source change ran $BUILD_CALLS builds (expected 1)"
    fi
}

test_frontend_build_stale_via_root_lockfile() {
    echo "test: ensure_frontend_built treats the root lockfile as a build input"
    local saved_fe="$FRONTEND_DIR" saved_proj="$_E2E_PROJECT_DIR"
    setup_frontend_sandbox lockfile
    touch_build_inputs 202601010000
    touch -t 202601020000 "$FRONTEND_DIR/dist/index.html"
    # npm workspaces hoist to the root and `npm ci` restores node_modules from
    # the root lockfile, so a dep bump changes the bundle without touching a
    # single app file — the bundle must not be reused across it.
    touch -t 202601030000 "$FE_ROOT/package-lock.json"

    ensure_frontend_built >"$FE_OUT" 2>&1
    FRONTEND_DIR="$saved_fe"; _E2E_PROJECT_DIR="$saved_proj"

    if [ "$BUILD_CALLS" -eq 1 ]; then
        pass "a newer root lockfile triggered a rebuild"
    else
        fail "lockfile change ran $BUILD_CALLS builds (expected 1) — the bundle would keep the old dependency graph"
    fi
}

test_frontend_build_staleness_check_fails_open() {
    echo "test: a failing filesystem walk degrades to 'not stale', it does not abort the run"
    local saved_fe="$FRONTEND_DIR" saved_proj="$_E2E_PROJECT_DIR" rc
    setup_frontend_sandbox failopen
    touch_build_inputs 202601010000
    touch -t 202601020000 "$FRONTEND_DIR/dist/index.html"
    # The caller captures the probe through `newer="$(…)"`, and a bare assignment
    # from a command substitution takes the substitution's exit status — so under
    # the e2e scripts' `set -e` a walk that failed for any transient reason would
    # kill the entire run instead of just rebuilding. Stub `find` to fail and
    # prove both the direct call and the captured call survive.
    (
        set -e
        find() { return 1; }
        _first_build_input_newer_than "$FRONTEND_DIR/dist/index.html" >/dev/null
        newer="$(_first_build_input_newer_than "$FRONTEND_DIR/dist/index.html")"
        [ -z "$newer" ]
    )
    rc=$?
    FRONTEND_DIR="$saved_fe"; _E2E_PROJECT_DIR="$saved_proj"
    if [ "$rc" -eq 0 ]; then
        pass "the probe returns 0 and empty, so a \`set -e\` caller survives it"
    else
        fail "the probe propagated a walk failure (rc=$rc) — set -e would abort the e2e run"
    fi
}

test_frontend_build_fresh_dist_reused() {
    echo "test: ensure_frontend_built reuses a dist/ newer than every build input"
    local saved_fe="$FRONTEND_DIR" saved_proj="$_E2E_PROJECT_DIR"
    setup_frontend_sandbox fresh
    touch_build_inputs 202601010000
    touch -t 202601020000 "$FRONTEND_DIR/dist/index.html"

    ensure_frontend_built >"$FE_OUT" 2>&1
    local rc=$?
    FRONTEND_DIR="$saved_fe"; _E2E_PROJECT_DIR="$saved_proj"

    if [ "$rc" -eq 0 ]; then pass "returned 0"; else fail "returned $rc"; fi
    if [ "$BUILD_CALLS" -eq 0 ]; then
        pass "fresh dist was reused (no build)"
    else
        fail "fresh dist ran $BUILD_CALLS builds (expected 0)"
    fi
    if grep -q "REUSED existing dist/" "$FE_OUT"; then
        pass "logged the REUSED branch"
    else
        fail "did not log the REUSED branch"; cat "$FE_OUT"
    fi
}

# ── report_webkit_excluded ────────────────────────────────────────────
# A --no-webkit run is missing the most expensive project in the suite. It must
# not read like a complete one, and the per-project table cannot say so, because
# an excluded project is dropped from it rather than given a fake rc.
test_webkit_exclusion_is_announced() {
    echo "test: report_webkit_excluded names the gap and the recovery command"
    local out="$SANDBOX/webkit-excluded.out"
    report_webkit_excluded 1 >"$out" 2>&1

    if grep -q "did NOT run" "$out"; then
        pass "says mobile-webkit did not run"
    else
        fail "an excluded project was not announced"; cat "$out"
    fi
    if grep -q "Coverage is incomplete" "$out"; then
        pass "says coverage is incomplete"
    else
        fail "did not say the run has a hole in it"; cat "$out"
    fi
    if grep -q -- "e2e-browser.sh --webkit" "$out"; then
        pass "names the command that closes the gap"
    else
        fail "left the reader with no recovery command"; cat "$out"
    fi
}

test_webkit_exclusion_is_silent_when_it_ran() {
    echo "test: report_webkit_excluded says nothing on an ordinary full run"
    local out="$SANDBOX/webkit-included.out"
    report_webkit_excluded "" >"$out" 2>&1

    if [ -s "$out" ]; then
        fail "warned about an exclusion on a run that included mobile-webkit"; cat "$out"
    else
        pass "silent when the project was not excluded"
    fi
}

# ── report_project_exit_codes ─────────────────────────────────────────
# The nightly's per-project table printed a blank rc for the last project on a
# run where that project had two real failures. Two halves are covered here: the
# reporter must never print a blank cell as if it were a result, and the lib
# functions the project loop calls must not leak an iteration variable that
# corrupts the caller's array in the first place.
RPT_OUT=""
run_reporter() {
    RPT_OUT="$SANDBOX/report.out"
    report_project_exit_codes "$@" >"$RPT_OUT" 2>&1
}

test_report_prints_every_exit_code() {
    echo "test: report_project_exit_codes prints a real integer for every project"
    local rc
    run_reporter 1 mobile-webkit:0 chromium:0 mobile:2
    rc=$?

    if grep -q "^  mobile-webkit: 0$" "$RPT_OUT" \
        && grep -q "^  chromium: 0$" "$RPT_OUT" \
        && grep -q "^  mobile: 2$" "$RPT_OUT"; then
        pass "every project printed its integer rc"
    else
        fail "per-project table incomplete"; cat "$RPT_OUT"
    fi
    if [ "$rc" -eq 1 ]; then
        pass "passes the umbrella exit code through unchanged"
    else
        fail "umbrella exit code became $rc (expected 1)"
    fi
    if grep -q "harness bug" "$RPT_OUT"; then
        fail "flagged a harness bug on a complete table"
    else
        pass "no harness-bug banner on a complete table"
    fi
}

test_report_blank_rc_is_unknown_and_fails() {
    echo "test: a blank per-project rc prints UNKNOWN and forces a non-zero exit"
    local rc
    # The exact 2026-07-26 shape: everything passed as far as the harness knows,
    # but the last project's rc never landed. Reporting green here is the bug.
    run_reporter 0 mobile-webkit:0 chromium:0 mobile:
    rc=$?

    if grep -q "^  mobile: UNKNOWN (harness bug)$" "$RPT_OUT"; then
        pass "blank rc printed as UNKNOWN (harness bug)"
    else
        fail "blank rc not flagged"; cat "$RPT_OUT"
    fi
    if [ "$rc" -ne 0 ]; then
        pass "forced a non-zero exit ($rc) instead of reporting green"
    else
        fail "reported green with an unknown project status"
    fi
}

test_report_non_numeric_rc_is_unknown() {
    echo "test: a non-numeric per-project rc is UNKNOWN too"
    local rc
    run_reporter 0 mobile-webkit:0 chromium:oops mobile:1
    rc=$?

    if grep -q "^  chromium: UNKNOWN (harness bug)$" "$RPT_OUT"; then
        pass "non-numeric rc printed as UNKNOWN (harness bug)"
    else
        fail "non-numeric rc not flagged"; cat "$RPT_OUT"
    fi
    if [ "$rc" -ne 0 ]; then pass "forced a non-zero exit ($rc)"; else fail "reported green"; fi
}

test_report_non_numeric_overall_forced_nonzero() {
    echo "test: a non-integer umbrella exit code is itself treated as a harness bug"
    local rc
    run_reporter "" mobile-webkit:0
    rc=$?
    if [ "$rc" -eq 1 ]; then pass "empty umbrella code forced to 1"; else fail "got $rc (expected 1)"; fi
    if grep -q "is not an integer" "$RPT_OUT"; then
        pass "explained the forced exit code"
    else
        fail "did not explain the forced exit code"; cat "$RPT_OUT"
    fi
}

test_no_sourced_lib_leaks_a_loop_variable() {
    echo "test: no function in a sourced e2e lib leaks a loop variable to its caller"
    # The dynamic test below pins the one function that actually caused the
    # nightly's blank exit code. This one holds the whole CLASS: e2e-browser.sh
    # drives an indexed loop and calls deep into these libs, so ANY of them
    # leaking a loop variable can corrupt the caller's iteration. Static, because
    # the dangerous functions (start_engine, _start_postgres_container) spawn
    # engines and Docker containers — not something to boot for an assertion —
    # and because the failure we want to prevent is a NEW leak being added, which
    # a scan catches and a fixture never would.
    #
    # `_` is exempt: bash reassigns it after every simple command, so no caller
    # can rely on it and localising it would be noise.
    local scan="$SANDBOX/loopscan.awk"
    cat > "$scan" <<'AWK'
/^[A-Za-z_][A-Za-z0-9_]*\(\)[[:space:]]*[({]?[[:space:]]*$/ { fn=$0; sub(/\(\).*/,"",fn); locals=" "; next }
/^[)}][[:space:]]*$/ { fn=""; locals=" "; next }
/^[[:space:]]*local[[:space:]]/ {
  line=$0; sub(/^[[:space:]]*local[[:space:]]+/,"",line)
  n=split(line, parts, /[[:space:]]+/)
  for (k=1;k<=n;k++) { v=parts[k]; sub(/=.*/,"",v); if (v ~ /^-/) continue
    if (v ~ /^[A-Za-z_][A-Za-z0-9_]*$/) locals = locals v " " }
}
# Require `do` on the same line so an embedded Python heredoc (`for w in wss:`)
# is not mistaken for a shell loop. Every shell for-in in these libs is written
# that way.
/(^|[[:space:];])do([[:space:]]|$)/ && match($0, /(^|[[:space:];])for[[:space:]]+[A-Za-z_][A-Za-z0-9_]*[[:space:]]+in[[:space:]]/) {
  s=substr($0, RSTART, RLENGTH); gsub(/^[^f]*for[[:space:]]+/,"",s); sub(/[[:space:]]+in[[:space:]]*$/,"",s)
  if (s == "_") next
  if (fn != "" && index(locals, " " s " ") == 0) printf "%s:%d %s() loops on non-local `%s`\n", FILENAME, NR, fn, s
}
AWK

    # Everything scripts/lib/e2e.sh pulls in, directly or transitively — the set
    # reachable from e2e-browser.sh's project loop.
    local libs=(e2e.sh workspace.sh ports.sh e2e_lock.sh webkit_reaper.sh
        host_load_guard.sh sleep.sh preflight.sh)
    local lib leaks=""
    for lib in "${libs[@]}"; do
        [ -f "$SCRIPT_DIR/$lib" ] || continue
        leaks="$leaks$(awk -f "$scan" "$SCRIPT_DIR/$lib")"
    done

    if [ -z "$leaks" ]; then
        pass "every loop variable in the sourced libs is declared local"
    else
        fail "loop variables leak to callers:"
        printf '%s\n' "$leaks" | sed 's/^/    /'
    fi

    # Prove the scan can actually see a leak — otherwise a broken matcher would
    # report "clean" forever, which is the very failure mode this file exists for.
    local probe="$SANDBOX/leaky.sh"
    cat > "$probe" <<'SH'
leaky_fn() {
    local other
    for i in 1 2 3; do
        other="$i"
    done
}
SH
    if [ -n "$(awk -f "$scan" "$probe")" ]; then
        pass "the scan detects a planted leak (not vacuous)"
    else
        fail "the scan missed a planted leak — it would never catch a real one"
    fi
}

test_ensure_workspace_running_does_not_leak_loop_index() {
    echo "test: ensure_workspace_running does not leak its readiness counter into the caller"
    # The mechanism behind the blank rc, reproduced end to end: e2e-browser.sh
    # drives `for i in "${!PROJECTS[@]}"` and calls reset_e2e_database — hence
    # ensure_workspace_running — from inside the body, then records the result at
    # PROJECT_RCS[$i]. While that function's `for i in {1..30}` readiness poll was
    # not local, `i` came back as the poll count, so every iteration wrote the
    # same low slot and the LAST project's entry was never created.
    #
    # Run in a subshell so the stubs (a curl that always answers, no-ops for the
    # workspace/port/build helpers) can't leak into later tests. Only this
    # function's own control flow is exercised — nothing is booted.
    local result entries last
    result="$(
        e2e_workspace_env() { VITE_PORT=65000; PROTO=http; }
        swap_ports() { :; }
        ensure_frontend_built() { :; }
        curl() { echo "<!DOCTYPE html>"; }
        projects=(mobile-webkit chromium mobile)
        rcs=()
        for i in "${!projects[@]}"; do
            ensure_workspace_running >/dev/null 2>&1
            rcs[i]=7
        done
        printf '%s %s' "${#rcs[@]}" "${rcs[2]:-MISSING}"
    )"
    entries="${result%% *}"
    last="${result##* }"

    if [ "$entries" = "3" ]; then
        pass "one recorded entry per project (got $entries)"
    else
        fail "recorded $entries entries for 3 projects — the loop index was clobbered"
    fi
    if [ "$last" = "7" ]; then
        pass "the last project's slot was written"
    else
        fail "the last project's slot is $last — exactly the blank nightly cell"
    fi
}

test_ensure_workspace_running_builds_the_frontend_before_the_engine() {
    echo "test: ensure_workspace_running builds dist/ before the engine pins it"
    # At boot the engine snapshots the dist/ it finds and serves that copy
    # (api/frontend_snapshot.rs). A build landing after start_engine is never
    # served, so every spec grades the PREVIOUS build and says nothing about it.
    # Stubs only, in a subshell: the two steps announce themselves and nothing
    # is booted. The health curl answers no, so the start branch is the one run.
    local order
    order="$(
        e2e_workspace_env() { VITE_PORT=65000; PROTO=http; }
        swap_ports() { :; }
        setup_postgres() { :; }
        build_e2e_engine_once() { :; }
        ensure_frontend_built() { echo "frontend"; }
        attach_e2e_gateway() { echo "gateway"; }
        start_engine() { echo "engine"; }
        curl() { if [[ "$*" == *health* ]]; then return 1; fi; echo "<!DOCTYPE html>"; }
        ensure_workspace_running 2>/dev/null | grep -E '^(frontend|gateway|engine)$' | tr '\n' ' '
    )"

    if [ "$order" = "frontend gateway engine " ]; then
        pass "the build and the gateway identity land before the boot"
    else
        fail "order was '$order': the engine boots on a stale dist/ or an inherited gateway"
    fi
}

# ── the sandbox contract itself ───────────────────────────────────────
# Every test above aims at $E2E_WORKSPACE, which this file pins to a sandbox
# BEFORE sourcing e2e.sh. That pin is load-bearing well beyond worktree pruning:
# `e2e_workspace_env` exports E2E_WORKSPACE as $WORKSPACE, which resolves
# $ENGINE_PIDFILE, which `stop_e2e_engine` sends SIGUSR1 to — the one signal the
# engine does NOT ignore. A hard `E2E_WORKSPACE=...` in e2e.sh silently clobbered
# the pin, pointing this whole file (and any future test that reaches a stop or
# cleanup path) at the real ~/workspaces/e2e-test. Assert the pin survives the
# source, so the escape can't come back unnoticed.
test_source_honors_pinned_workspace() {
    echo "test: sourcing e2e.sh does not clobber a pinned E2E_WORKSPACE"
    if [ "$E2E_WORKSPACE" = "$SANDBOX/e2e-test" ]; then
        pass "E2E_WORKSPACE still points at the sandbox after sourcing e2e.sh"
    else
        fail "e2e.sh clobbered the pin: expected $SANDBOX/e2e-test, got $E2E_WORKSPACE"
    fi
}

# ── engine version pin ────────────────────────────────────────────────
# Without the pin, a commit landing in the checkout mid-run made the e2e engine
# rebuild itself and raise a version toast over unrelated specs.
test_source_pins_the_engine_version_for_the_engine_it_starts() {
    echo "test: sourcing e2e.sh exports the engine version pin to child processes"
    local got
    # Scrubbed first, so an inherited value cannot pass this. `env` lists only
    # what a child process would inherit, so a bare assignment fails too.
    got=$(
        unset LUCIDOS_PIN_ENGINE_VERSION
        # shellcheck source=e2e.sh
        source "$SCRIPT_DIR/e2e.sh" >/dev/null 2>&1
        env | sed -n 's/^LUCIDOS_PIN_ENGINE_VERSION=//p'
    )
    if [ "$got" = "1" ]; then
        pass "LUCIDOS_PIN_ENGINE_VERSION=1 reaches the engine"
    else
        fail "expected LUCIDOS_PIN_ENGINE_VERSION=1 in the engine's environment, got '$got'"
    fi
}

test_the_engine_reads_the_variable_the_script_sets() {
    echo "test: the engine's pin variable is the one e2e.sh exports"
    local src="$SCRIPT_DIR/../../crates/lucidos-engine/src/engine/engine_version.rs"
    if grep -q 'PIN_ENGINE_VERSION_ENV: &str = "LUCIDOS_PIN_ENGINE_VERSION";' "$src"; then
        pass "engine_version.rs reads LUCIDOS_PIN_ENGINE_VERSION"
    else
        fail "engine_version.rs no longer reads LUCIDOS_PIN_ENGINE_VERSION, so the e2e pin is inert"
    fi
}

# ── playwright_file_filter ────────────────────────────────────────────
test_playwright_filter_anchors_the_basename() {
    echo "test: playwright_file_filter anchors and escapes a spec name"
    local got
    got=$(playwright_file_filter "chat.spec.ts")
    if [ "$got" = '/chat\.spec\.ts$' ]; then
        pass "chat.spec.ts becomes /chat\\.spec\\.ts\$"
    else
        fail "expected /chat\\.spec\\.ts\$, got $got"
    fi
}

test_playwright_filter_escapes_regex_metacharacters() {
    echo "test: playwright_file_filter escapes metacharacters in a spec name"
    local got
    got=$(playwright_file_filter "a+b(c).spec.ts")
    if [ "$got" = '/a\+b\(c\)\.spec\.ts$' ]; then
        pass "metacharacters are escaped"
    else
        fail "expected /a\\+b\\(c\\)\\.spec\\.ts\$, got $got"
    fi
}

# The concrete regression. A bare basename `chat.spec.ts` also selects
# app-coding-agent-spawn-from-chat.spec.ts, so a run asked for one spec silently
# got a coding-agent one too.
test_playwright_filter_does_not_match_a_longer_sibling() {
    echo "test: playwright_file_filter does not match a longer sibling path"
    local re
    re=$(playwright_file_filter "chat.spec.ts")
    if [[ "/repo/e2e/chat.spec.ts" =~ $re ]]; then
        pass "the filter still matches its own file"
    else
        fail "the filter no longer matches its own file"
    fi
    if [[ "/repo/e2e/app-coding-agent-spawn-from-chat.spec.ts" =~ $re ]]; then
        fail "the filter still matches the longer sibling"
    else
        pass "the filter does not match the longer sibling"
    fi
}

# The durable guard: run the REAL mobile-webkit inventory through the filter and
# assert every pattern selects exactly its own file. This fails the day someone
# adds a spec whose name is a path suffix of another, which is how nine tests
# came to run twice with nothing reporting it.
test_every_mobile_webkit_spec_filter_selects_exactly_one_file() {
    echo "test: every mobile-webkit spec filter selects exactly one spec"
    local repo_root e2e_dir f base re other collisions=0 n=0
    repo_root="$(cd "$SCRIPT_DIR/../.." && pwd)"
    e2e_dir="$repo_root/crates/lucidos-app/e2e"
    if [ ! -d "$e2e_dir" ]; then
        fail "spec dir not found: $e2e_dir"
        return
    fi
    local specs=()
    for f in "$e2e_dir"/*.spec.ts; do
        [ -e "$f" ] || continue
        base="$(basename "$f")"
        case "$base" in *-desktop.spec.ts) continue ;; esac
        specs+=("$base")
    done
    if [ "${#specs[@]}" -eq 0 ]; then
        fail "no mobile-webkit specs discovered (a disarmed check must not read as clean)"
        return
    fi
    for base in "${specs[@]}"; do
        n=$((n + 1))
        re=$(playwright_file_filter "$base")
        if ! [[ "$e2e_dir/$base" =~ $re ]]; then
            fail "filter for $base does not match its own path"
            collisions=$((collisions + 1))
        fi
        for other in "${specs[@]}"; do
            [ "$other" = "$base" ] && continue
            if [[ "$e2e_dir/$other" =~ $re ]]; then
                fail "filter for $base also selects $other"
                collisions=$((collisions + 1))
            fi
        done
    done
    if [ "$collisions" -eq 0 ]; then
        pass "$n spec filters each select exactly one file"
    fi
}

# ── project_runs_spec ─────────────────────────────────────────────────
test_project_runs_spec_mirrors_test_ignore() {
    echo "test: project_runs_spec skips exactly the projects that testIgnore a spec"
    local row project spec want got
    for row in \
        "chromium chat.spec.ts yes" \
        "mobile chat.spec.ts yes" \
        "mobile-webkit chat.spec.ts yes" \
        "chromium header-desktop.spec.ts yes" \
        "mobile header-desktop.spec.ts no" \
        "mobile-webkit e2e/header-desktop.spec.ts no" \
        "chromium swipe-mobile.spec.ts no" \
        "mobile swipe-mobile.spec.ts yes" \
        "mobile-webkit swipe-mobile.spec.ts yes"; do
        read -r project spec want <<<"$row"
        if project_runs_spec "$project" "$spec"; then got=yes; else got=no; fi
        if [ "$got" = "$want" ]; then
            pass "$project runs $spec: $want"
        else
            fail "$project runs $spec: expected $want, got $got"
        fi
    done
}

# The helper restates playwright.config.ts, so pin the config to what it says.
test_project_runs_spec_matches_the_playwright_config() {
    echo "test: playwright.config.ts still carries the testIgnore patterns project_runs_spec mirrors"
    local config n
    config="$(cd "$SCRIPT_DIR/../.." && pwd)/crates/lucidos-app/playwright.config.ts"
    n=$(grep -c "testIgnore: /-mobile\\\\.spec\\\\.ts\$/," "$config")
    if [ "$n" = 1 ]; then
        pass "one project ignores -mobile specs"
    else
        fail "expected one -mobile testIgnore in $config, found $n"
    fi
    n=$(grep -c "testIgnore: /-desktop\\\\.spec\\\\.ts\$/," "$config")
    if [ "$n" = 2 ]; then
        pass "two projects ignore -desktop specs"
    else
        fail "expected two -desktop testIgnores in $config, found $n"
    fi
}

# ── e2e_browser_projects / e2e_browser_lock_projects ──────────────────
test_browser_projects_match_the_playwright_config() {
    echo "test: e2e_browser_projects names every project playwright.config.ts defines"
    local config ours theirs
    config="$(cd "$SCRIPT_DIR/../.." && pwd)/crates/lucidos-app/playwright.config.ts"
    theirs="$(sed -n "s/^      name: '\\([^']*\\)',\$/\\1/p" "$config" | sort | paste -sd, -)"
    ours="$(e2e_browser_projects | sort | paste -sd, -)"
    if [ -n "$theirs" ] && [ "$ours" = "$theirs" ]; then
        pass "the shell list and the config agree ($ours)"
    else
        fail "shell lists '$ours', playwright.config.ts defines '$theirs'"
    fi
    if [ "$(e2e_browser_projects --no-webkit | paste -sd, -)" = "chromium,mobile" ]; then
        pass "--no-webkit drops mobile-webkit"
    else
        fail "--no-webkit kept the wrong set: $(e2e_browser_projects --no-webkit | paste -sd, -)"
    fi
}

test_lock_projects_follow_the_arguments() {
    echo "test: e2e_browser_lock_projects declares the projects the run will exercise"
    local got
    _check_lock_projects() {
        got="$(e2e_browser_lock_projects "$@")"
        if [ "$got" = "$EXPECT" ]; then pass "$WHY → $got"; else fail "$WHY: expected '$EXPECT', got '$got'"; fi
    }
    EXPECT="chromium,mobile,mobile-webkit" WHY="no flags" _check_lock_projects "" "" ""
    EXPECT="chromium,mobile" WHY="--no-webkit" _check_lock_projects "" 1 ""
    EXPECT="mobile-webkit" WHY="--webkit" _check_lock_projects 1 "" ""
    EXPECT="mobile" WHY="--project=mobile" _check_lock_projects "" "" "" --project=mobile --grep x
    EXPECT="chromium,mobile" WHY="--project twice, split form" \
        _check_lock_projects "" "" "" --project chromium --project mobile
    EXPECT="chromium,mobile-webkit" WHY="one split --project takes every value up to the next flag" \
        _check_lock_projects "" "" "" --project chromium mobile-webkit --grep x
    EXPECT="mobile,mobile-webkit" WHY="-f on a mobile-only spec" \
        _check_lock_projects "" "" "composer-row-fit-mobile.spec.ts"
    EXPECT="chromium" WHY="-f on a desktop-only spec" \
        _check_lock_projects "" "" "app-frame-can-copy-desktop.spec.ts"
    unset -f _check_lock_projects
}

# ── report_e2e_mem_top_deltas ─────────────────────────────────────────
# Pins the reporter's wire format: formatMemLine in
# crates/lucidos-app/e2e/memSampleReporter.ts writes these keys, and the
# test below checks it still does.
_mem_line() {
    printf '2026-10-07T04:30:00.000Z\tproject=%s\tspec=%s\ttitle=t\tretry=0\tstatus=passed\tduration_ms=1\tpages=1\tlimit=1000\tdelta=%s\twebkit_gpu=1\twebkit_gpu_rss_kb=2\n' "$1" "$2" "$3"
}

test_mem_sample_env_follows_the_run_id() {
    echo "test: export_e2e_mem_sample_env puts the mirror under \$HOME, named for the run"
    # Read back through printenv: the Playwright child sees only what is exported.
    local with without
    HOME="$SANDBOX/home" LUCIDOS_E2E_RUN_ID="123-45-abc" export_e2e_mem_sample_env /x/full
    with="$(printenv LUCIDOS_E2E_MEM_LOG)|$(printenv LUCIDOS_E2E_MEM_MIRROR)"
    local token
    token="$(printenv LUCIDOS_E2E_WEBKIT_PATH_TOKEN)"
    LUCIDOS_E2E_RUN_ID="" export_e2e_mem_sample_env /x
    without="$(printenv LUCIDOS_E2E_MEM_LOG)|$(printenv LUCIDOS_E2E_MEM_MIRROR)"
    unset LUCIDOS_E2E_MEM_LOG LUCIDOS_E2E_MEM_MIRROR LUCIDOS_E2E_WEBKIT_PATH_TOKEN
    if [ "$with" = "/x/full/mem-samples.log|$SANDBOX/home/.lucidos/e2e-mem/123-45-abc.log" ]; then
        pass "log under the output root, mirror under \$HOME named for the run"
    else
        fail "wrong paths with a run id: $with"
    fi
    if [ "$without" = "/x/mem-samples.log|" ]; then
        pass "no run id, no mirror"
    else
        fail "wrong paths without a run id: $without"
    fi
    if [ -n "$token" ] && [ "$token" = "$(_reaper_match)" ]; then
        pass "the GPU count uses the WebKit reaper's own path token ($token)"
    else
        fail "exported token '$token' is not the reaper's '$(_reaper_match)'"
    fi
}

test_mem_top_deltas_groups_and_ranks() {
    echo "test: report_e2e_mem_top_deltas sums by project and spec, largest first"
    local log="$SANDBOX/mem.log" out="$SANDBOX/mem.out" i
    {
        _mem_line mobile-webkit chat.spec.ts 300
        _mem_line mobile-webkit chat.spec.ts 200
        _mem_line chromium chat.spec.ts 400
        _mem_line mobile-webkit phone-probe.spec.ts 900
        _mem_line mobile-webkit drain.spec.ts -50
        _mem_line mobile-webkit unknown.spec.ts '?'
        for i in 1 2 3 4 5 6 7 8 9 10; do _mem_line mobile "small-$i.spec.ts" "$i"; done
    } > "$log"
    report_e2e_mem_top_deltas "$log" > "$out"

    local rows
    rows="$(grep -E '^ +-?[0-9]+ ' "$out")"
    if [ "$(printf '%s\n' "$rows" | wc -l | tr -d ' ')" = 10 ]; then
        pass "prints ten rows"
    else
        fail "expected ten rows:"; cat "$out"
    fi
    if printf '%s\n' "$rows" | head -1 | grep -qE '^ +900 +90\.0% +1 +mobile-webkit +phone-probe\.spec\.ts$'; then
        pass "the largest delta leads, with its share of the limit"
    else
        fail "wrong first row:"; cat "$out"
    fi
    if printf '%s\n' "$rows" | sed -n 2p | grep -qE '^ +500 +50\.0% +2 +mobile-webkit +chat\.spec\.ts$'; then
        pass "one spec's tests on one project are summed, apart from other projects"
    else
        fail "wrong second row:"; cat "$out"
    fi
    if grep -q 'unknown.spec.ts' "$out"; then
        fail "a line with an unknown delta was ranked"
    else
        pass "an unknown delta is left out"
    fi
    if grep -q "per-test log: $log" "$out"; then
        pass "names the log"
    else
        fail "did not say where the log is"; cat "$out"
    fi
}

test_mem_line_keys_match_the_reporter() {
    echo "test: every key report_e2e_mem_top_deltas reads is one the reporter writes"
    local reporter key
    reporter="$(cd "$SCRIPT_DIR/../.." && pwd)/crates/lucidos-app/e2e/memSampleReporter.ts"
    for key in project spec delta limit; do
        if grep -q "\`$key=\\\${" "$reporter"; then
            pass "the reporter writes $key="
        else
            fail "formatMemLine in $reporter no longer writes '$key='"
        fi
    done
}

test_mem_top_deltas_is_silent_without_a_log() {
    echo "test: report_e2e_mem_top_deltas prints nothing when no samples were written"
    local out
    out="$(report_e2e_mem_top_deltas "$SANDBOX/no-such.log")"
    if [ -z "$out" ]; then pass "silent"; else fail "printed: $out"; fi
}

# ── summarise_playwright_log / report_playwright_totals ───────────────
# A project runs in chunks and prints a summary per invocation, so its own
# verdict exists only if the harness adds them up. These pin the adding up and
# the guard on it.

# Two chunks of a chunked project, the shape Playwright's list reporter emits.
_write_two_chunk_log() {
    cat > "$1" <<'LOG'
Running 9 tests using 1 worker
  1 flaky
  8 passed (1.2m)
Running 7 tests using 1 worker
  2 skipped
  5 passed (48.0s)
LOG
}

test_tally_sums_every_chunk() {
    echo "test: summarise_playwright_log adds up every chunk's summary"
    local log tally
    log="$SANDBOX/tally-two.log"
    _write_two_chunk_log "$log"
    tally=$(summarise_playwright_log "$log")
    # planned passed failed flaky skipped interrupted didnotrun invocations
    if [ "$tally" = "16 13 0 1 2 0 0 2" ]; then
        pass "two chunks summed to 16 planned, 13 passed, 1 flaky, 2 skipped"
    else
        fail "expected '16 13 0 1 2 0 0 2', got '$tally'"
    fi
}

test_tally_strips_colour() {
    echo "test: summarise_playwright_log counts through ANSI colour"
    local log tally esc
    log="$SANDBOX/tally-colour.log"
    esc=$(printf '\033')
    {
        echo "Running 3 tests using 1 worker"
        printf '  %s[32m3 passed%s[39m (4.0s)\n' "$esc" "$esc"
    } > "$log"
    tally=$(summarise_playwright_log "$log")
    if [ "$tally" = "3 3 0 0 0 0 0 1" ]; then
        pass "colour codes did not hide the counts"
    else
        fail "expected '3 3 0 0 0 0 0 1', got '$tally'"
    fi
}

test_tally_ignores_per_test_lines() {
    echo "test: summarise_playwright_log ignores the per-test progress lines"
    local log tally
    log="$SANDBOX/tally-progress.log"
    cat > "$log" <<'LOG'
Running 2 tests using 1 worker
  ✓   1 [mobile-webkit] › e2e/a.spec.ts:1:1 › passed once (1.0s)
  ✘   2 [mobile-webkit] › e2e/b.spec.ts:2:2 › failed here (2.0s)
  1 failed
  1 passed (3.0s)
LOG
    tally=$(summarise_playwright_log "$log")
    if [ "$tally" = "2 1 1 0 0 0 0 1" ]; then
        pass "only the summary lines were counted"
    else
        fail "expected '2 1 1 0 0 0 0 1', got '$tally'"
    fi
}

test_totals_report_one_verdict_for_the_project() {
    echo "test: report_playwright_totals prints one verdict and returns 0"
    local log out rc=0
    log="$SANDBOX/tally-ok.log"
    out="$SANDBOX/tally-ok.out"
    _write_two_chunk_log "$log"
    report_playwright_totals mobile-webkit "$log" > "$out" 2>&1 || rc=$?
    if [ "$rc" -eq 0 ]; then
        pass "a tally that adds up returns 0"
    else
        fail "returned $rc on a consistent tally"; cat "$out"
    fi
    if grep -q "mobile-webkit total: 16 tests over 2 invocation" "$out"; then
        pass "printed the project's own total"
    else
        fail "no project total line"; cat "$out"
    fi
}

# The regression this whole pair exists for: an invocation that died before
# printing its summary. Chunked green was green precisely because nothing
# noticed the missing numbers.
test_totals_catch_an_invocation_that_never_reported() {
    echo "test: report_playwright_totals fails when a chunk never reported"
    local log out rc=0
    log="$SANDBOX/tally-lost.log"
    out="$SANDBOX/tally-lost.out"
    cat > "$log" <<'LOG'
Running 9 tests using 1 worker
  9 passed (1.2m)
Running 7 tests using 1 worker
LOG
    report_playwright_totals mobile-webkit "$log" > "$out" 2>&1 || rc=$?
    if [ "$rc" -ne 0 ]; then
        pass "a chunk with no summary forces a non-zero return"
    else
        fail "returned 0 while 7 tests went unaccounted"; cat "$out"
    fi
    if grep -q "planned 16 tests but accounted for 9" "$out"; then
        pass "named the shortfall"
    else
        fail "did not name the shortfall"; cat "$out"
    fi
}

test_totals_refuse_an_empty_log() {
    echo "test: report_playwright_totals refuses an empty capture"
    local out rc=0
    out="$SANDBOX/tally-empty.out"
    : > "$SANDBOX/tally-empty.log"
    report_playwright_totals mobile-webkit "$SANDBOX/tally-empty.log" > "$out" 2>&1 || rc=$?
    if [ "$rc" -ne 0 ]; then
        pass "no captured output is a harness bug, not a green run"
    else
        fail "returned 0 on an empty log"; cat "$out"
    fi
}

# ── assert_e2e_workspace_tree_clean ───────────────────────────────────
# Each test points E2E_WORKSPACE at a throwaway repo of its own, so the real
# e2e workspace is never read.
new_tree_repo() {
    local repo="$SANDBOX/tree-$1"
    rm -rf "$repo"
    git init -q "$repo"
    git -C "$repo" -c user.name=t -c user.email=t@t commit -q --allow-empty -m init
    echo "$repo"
}

test_tree_assert_passes_a_run_that_leaves_nothing() {
    echo "test: assert_e2e_workspace_tree_clean passes a run that leaves nothing"
    local E2E_WORKSPACE before
    E2E_WORKSPACE="$(new_tree_repo clean)"
    before="$(e2e_workspace_tree_status)"
    if assert_e2e_workspace_tree_clean "$before" 0 >/dev/null 2>&1; then
        pass "clean before and after passes"
    else
        fail "a clean tree failed the assertion"
    fi
}

test_tree_assert_fails_on_a_left_behind_file() {
    echo "test: assert_e2e_workspace_tree_clean fails on what a run leaves behind"
    local E2E_WORKSPACE before out="$SANDBOX/tree-dirty.out" rc=0
    E2E_WORKSPACE="$(new_tree_repo dirty)"
    before="$(e2e_workspace_tree_status)"
    mkdir -p "$E2E_WORKSPACE/data/triggers/probe"
    echo x > "$E2E_WORKSPACE/data/triggers/probe/run.py"
    assert_e2e_workspace_tree_clean "$before" 0 > "$out" 2>&1 || rc=$?
    if [ "$rc" -ne 0 ] && grep -q "data/triggers/probe/run.py" "$out"; then
        pass "a left-behind file fails and is named"
    else
        fail "rc=$rc, expected a failure naming the file"; cat "$out"
    fi
}

test_tree_assert_fails_on_an_uncommitted_delete() {
    echo "test: assert_e2e_workspace_tree_clean fails on a deletion left uncommitted"
    local E2E_WORKSPACE before rc=0
    E2E_WORKSPACE="$(new_tree_repo deleted)"
    echo x > "$E2E_WORKSPACE/fixture.py"
    git -C "$E2E_WORKSPACE" add fixture.py
    git -C "$E2E_WORKSPACE" -c user.name=t -c user.email=t@t commit -q -m fixture
    before="$(e2e_workspace_tree_status)"
    rm "$E2E_WORKSPACE/fixture.py"
    assert_e2e_workspace_tree_clean "$before" 0 >/dev/null 2>&1 || rc=$?
    if [ "$rc" -ne 0 ]; then
        pass "an unstaged deletion fails"
    else
        fail "an unstaged deletion passed"
    fi
}

test_tree_assert_does_not_blame_a_run_for_its_start() {
    echo "test: assert_e2e_workspace_tree_clean ignores dirt that predates the run"
    local E2E_WORKSPACE before
    E2E_WORKSPACE="$(new_tree_repo predates)"
    echo x > "$E2E_WORKSPACE/left-by-an-earlier-run.txt"
    before="$(e2e_workspace_tree_status)"
    if assert_e2e_workspace_tree_clean "$before" 0 >/dev/null 2>&1; then
        pass "pre-existing dirt is not this run's"
    else
        fail "dirt from before the run failed it"
    fi
}

test_tree_assert_waits_out_a_late_commit() {
    echo "test: assert_e2e_workspace_tree_clean waits for a commit still landing"
    local E2E_WORKSPACE before rc=0 committer
    E2E_WORKSPACE="$(new_tree_repo late)"
    before="$(e2e_workspace_tree_status)"
    echo x > "$E2E_WORKSPACE/late.txt"
    (
        sleep 1
        git -C "$E2E_WORKSPACE" add late.txt
        git -C "$E2E_WORKSPACE" -c user.name=t -c user.email=t@t commit -q -m late
    ) &
    committer=$!
    assert_e2e_workspace_tree_clean "$before" 5 >/dev/null 2>&1 || rc=$?
    wait "$committer"
    if [ "$rc" -eq 0 ]; then
        pass "a commit landing inside the settle window passes"
    else
        fail "failed before the late commit could land"
    fi
}

test_tree_assert_refuses_an_unreadable_tree() {
    echo "test: assert_e2e_workspace_tree_clean treats an unreadable tree as a failure"
    local E2E_WORKSPACE="$SANDBOX/tree-not-a-repo" rc=0
    mkdir -p "$E2E_WORKSPACE"
    assert_e2e_workspace_tree_clean "" 0 >/dev/null 2>&1 || rc=$?
    if [ "$rc" -ne 0 ]; then
        pass "a status that would not run is not clean"
    else
        fail "an unreadable tree passed as clean"
    fi
}

# ── reset_e2e_workspace_tree ──────────────────────────────────────────
# A sandbox workspace with a sibling beside it, so a reset that strayed past
# the workspace would show.
new_fresh_ws() {
    local ws="$SANDBOX/fresh/e2e-test"
    rm -rf "$SANDBOX/fresh"
    mkdir -p "$ws/.lucidos/worktrees" "$ws/data/apps/old-app" "$ws/artifacts" "$ws/.git" "$SANDBOX/fresh/e2e-test-old"
    echo "API_PORT=5341" > "$ws/.lucidos/ports"
    echo lock > "$ws/.lucidos/e2e.lock"
    echo x > "$ws/data/apps/old-app/index.html"
    echo x > "$ws/e2e-test1-01d859d3.txt"
    echo x > "$ws/.gitignore"
    echo x > "$ws/..odd"
    echo keep > "$SANDBOX/fresh/e2e-test-old/sibling.txt"
    echo "$ws"
}

test_tree_reset_keeps_only_lucidos() {
    echo "test: reset_e2e_workspace_tree removes every top-level entry but .lucidos/"
    local E2E_WORKSPACE left
    E2E_WORKSPACE="$(new_fresh_ws)"
    reset_e2e_workspace_tree > "$SANDBOX/fresh.out" 2>&1 || fail "the reset returned non-zero"
    left="$(find "$E2E_WORKSPACE" -mindepth 1 -maxdepth 1 | sed 's|.*/||' | tr '\n' ' ')"
    if [ "$left" = ".lucidos " ]; then pass "only .lucidos/ is left"; else fail "left behind: $left"; fi
    if [ -f "$E2E_WORKSPACE/.lucidos/ports" ] && [ -f "$E2E_WORKSPACE/.lucidos/e2e.lock" ]; then
        pass ".lucidos/ keeps the pinned ports and the lock"
    else
        fail ".lucidos/ lost its contents"
    fi
    if [ -f "$SANDBOX/fresh/e2e-test-old/sibling.txt" ]; then pass "a sibling of the workspace is untouched"; else fail "the reset reached a sibling"; fi
    if grep -q "removed 6 top-level entries" "$SANDBOX/fresh.out"; then pass "it says what it removed"; else fail "no count: $(cat "$SANDBOX/fresh.out")"; fi
}

test_tree_reset_refuses_a_running_engine() {
    echo "test: reset_e2e_workspace_tree refuses while the engine is running"
    local E2E_WORKSPACE rc=0 live
    E2E_WORKSPACE="$(new_fresh_ws)"
    sleep 30 &
    live=$!
    echo "$live" > "$E2E_WORKSPACE/.lucidos/engine.pid"
    reset_e2e_workspace_tree > "$SANDBOX/fresh-live.out" 2>&1 || rc=$?
    kill "$live" 2>/dev/null
    wait "$live" 2>/dev/null
    if [ "$rc" -ne 0 ]; then pass "it refuses"; else fail "it reset the tree under a live engine"; fi
    if [ -f "$E2E_WORKSPACE/data/apps/old-app/index.html" ]; then pass "nothing was removed"; else fail "it removed files before refusing"; fi
}

test_tree_reset_refuses_a_workspace_that_is_not_disposable() {
    echo "test: reset_e2e_workspace_tree refuses a workspace not named e2e-*"
    local E2E_WORKSPACE="$SANDBOX/fresh-dev/dev" rc=0
    mkdir -p "$E2E_WORKSPACE/data"
    echo x > "$E2E_WORKSPACE/data/keep.md"
    reset_e2e_workspace_tree > /dev/null 2>&1 || rc=$?
    if [ "$rc" -ne 0 ] && [ -f "$E2E_WORKSPACE/data/keep.md" ]; then
        pass "a live-looking workspace is refused and untouched"
    else
        fail "rc=$rc, keep.md present: $([ -f "$E2E_WORKSPACE/data/keep.md" ] && echo yes || echo no)"
    fi
}

test_the_fresh_reset_runs_between_the_engine_stop_and_the_boot() {
    echo "test: reset_e2e_database --fresh-workspace resets the tree after the stop and before the boot"
    local order plain
    order="$(
        e2e_workspace_env() { :; }
        setup_postgres() { :; }
        stop_e2e_engine() { echo stop; }
        reset_e2e_workspace_tree() { echo tree; }
        settle_e2e_workspace_tree() { echo settle; }
        workspace_database_name() { echo db; }
        _drop_shared_database() { :; }
        _create_shared_database() { :; }
        shared_pg_container() { echo pg; }
        docker() { echo f; }
        ensure_workspace_running() { echo boot; }
        reset_e2e_database --fresh-workspace 2>/dev/null | grep -E '^(stop|tree|settle|boot)$' | tr '\n' ' '
    )"
    plain="$(
        e2e_workspace_env() { :; }
        setup_postgres() { :; }
        stop_e2e_engine() { echo stop; }
        reset_e2e_workspace_tree() { echo tree; }
        settle_e2e_workspace_tree() { echo settle; }
        workspace_database_name() { echo db; }
        _drop_shared_database() { :; }
        _create_shared_database() { :; }
        shared_pg_container() { echo pg; }
        docker() { echo f; }
        ensure_workspace_running() { echo boot; }
        reset_e2e_database 2>/dev/null | grep -E '^(stop|tree|settle|boot)$' | tr '\n' ' '
    )"
    if [ "$order" = "stop tree settle boot " ]; then pass "the tree resets between the stop and the boot"; else fail "order was '$order'"; fi
    if [ "$plain" = "stop settle boot " ]; then pass "a plain reset leaves the tree alone"; else fail "plain order was '$plain'"; fi
}

test_only_the_start_of_a_run_asks_for_a_fresh_workspace() {
    echo "test: a run asks for a fresh workspace once, at its start, and never under --no-reset"
    local n
    n="$(grep -c 'reset_e2e_database --fresh-workspace' "$SCRIPT_DIR/../e2e.sh")"
    if [ "$n" = "1" ]; then pass "the umbrella asks once"; else fail "the umbrella asks $n times"; fi
    n="$(sed -n '/^setup_e2e_session() {/,/^}/p' "$SCRIPT_DIR/e2e.sh" | grep -c 'reset_e2e_database --fresh-workspace')"
    if [ "$n" = "1" ]; then pass "a standalone session asks once"; else fail "a standalone session asks $n times"; fi
    if sed -n '/^setup_e2e_session() {/,/^}/p' "$SCRIPT_DIR/e2e.sh" | grep -A1 'NO_RESET:-}" \]; then$' | tail -1 | grep -q ensure_workspace_running; then
        pass "--no-reset takes the plain start"
    else
        fail "--no-reset no longer takes the plain start"
    fi
    n="$(grep -c 'fresh-workspace' "$SCRIPT_DIR/../e2e-browser.sh")"
    if [ "$n" = "0" ]; then pass "no mid-run reset touches the tree"; else fail "e2e-browser.sh resets the tree $n times"; fi
}

test_source_honors_pinned_workspace
test_source_pins_the_engine_version_for_the_engine_it_starts
test_the_engine_reads_the_variable_the_script_sets
test_playwright_filter_anchors_the_basename
test_playwright_filter_escapes_regex_metacharacters
test_playwright_filter_does_not_match_a_longer_sibling
test_every_mobile_webkit_spec_filter_selects_exactly_one_file
test_project_runs_spec_mirrors_test_ignore
test_project_runs_spec_matches_the_playwright_config
test_browser_projects_match_the_playwright_config
test_lock_projects_follow_the_arguments
test_mem_sample_env_follows_the_run_id
test_mem_line_keys_match_the_reporter
test_mem_top_deltas_groups_and_ranks
test_mem_top_deltas_is_silent_without_a_log
test_prune_removes_empty_dir
test_prune_removes_dir_with_dangling_gitdir
test_prune_keeps_live_worktree
test_prune_keeps_dir_without_git_pointer
test_prune_handles_missing_root
test_cleanup_spares_real_cc_sessions
test_cleanup_survives_a_workspace_with_no_repo
test_cleanup_sweeps_the_workspace_repo
test_frontend_build_missing_dist_rebuilds
test_frontend_build_stale_dist_rebuilds
test_frontend_build_stale_via_workspace_local_sdk
test_frontend_build_stale_via_root_lockfile
test_frontend_build_staleness_check_fails_open
test_frontend_build_fresh_dist_reused
test_report_prints_every_exit_code
test_webkit_exclusion_is_announced
test_webkit_exclusion_is_silent_when_it_ran
test_report_blank_rc_is_unknown_and_fails
test_report_non_numeric_rc_is_unknown
test_report_non_numeric_overall_forced_nonzero
test_tally_sums_every_chunk
test_tally_strips_colour
test_tally_ignores_per_test_lines
test_totals_report_one_verdict_for_the_project
test_totals_catch_an_invocation_that_never_reported
test_totals_refuse_an_empty_log
test_no_sourced_lib_leaks_a_loop_variable
test_ensure_workspace_running_does_not_leak_loop_index
test_ensure_workspace_running_builds_the_frontend_before_the_engine
test_tree_assert_passes_a_run_that_leaves_nothing
test_tree_assert_fails_on_a_left_behind_file
test_tree_assert_fails_on_an_uncommitted_delete
test_tree_assert_does_not_blame_a_run_for_its_start
test_tree_assert_waits_out_a_late_commit
test_tree_assert_refuses_an_unreadable_tree

test_tree_reset_keeps_only_lucidos
test_tree_reset_refuses_a_running_engine
test_tree_reset_refuses_a_workspace_that_is_not_disposable
test_the_fresh_reset_runs_between_the_engine_stop_and_the_boot
test_only_the_start_of_a_run_asks_for_a_fresh_workspace
echo ""
echo "Passed: $PASS  Failed: $FAIL"
[ $FAIL -eq 0 ]
