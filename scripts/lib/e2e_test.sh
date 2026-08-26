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
        start_engine() { echo "engine"; }
        curl() { if [[ "$*" == *health* ]]; then return 1; fi; echo "<!DOCTYPE html>"; }
        ensure_workspace_running 2>/dev/null | grep -E '^(frontend|engine)$' | tr '\n' ' '
    )"

    if [ "$order" = "frontend engine " ]; then
        pass "the build lands before the boot that pins it"
    else
        fail "order was '$order': the engine pins a stale dist/ and serves it all run"
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

test_source_honors_pinned_workspace
test_prune_removes_empty_dir
test_prune_removes_dir_with_dangling_gitdir
test_prune_keeps_live_worktree
test_prune_keeps_dir_without_git_pointer
test_prune_handles_missing_root
test_cleanup_spares_real_cc_sessions
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
test_no_sourced_lib_leaks_a_loop_variable
test_ensure_workspace_running_does_not_leak_loop_index
test_ensure_workspace_running_builds_the_frontend_before_the_engine

echo ""
echo "Passed: $PASS  Failed: $FAIL"
[ $FAIL -eq 0 ]
