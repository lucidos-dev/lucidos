#!/bin/bash
# Tests for scripts/lib/workspace.sh.
# Run: ./scripts/lib/workspace_test.sh
set -u

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_DIR="$(cd "$SCRIPT_DIR/../.." && pwd)"

# Sandbox: redirect $HOME so our fake workspaces don't see real ones.
SANDBOX="$(mktemp -d)"
trap 'rm -rf "$SANDBOX"' EXIT
export HOME="$SANDBOX"
export LUCIDOS_GATEWAY_DATA="$SANDBOX/gateway"

PASS=0
FAIL=0
fail() { echo "  FAIL: $*"; FAIL=$((FAIL+1)); }
pass() { echo "  ok:   $*"; PASS=$((PASS+1)); }

# Source the lib in a way that doesn't require global setup.
# shellcheck source=workspace.sh
source "$SCRIPT_DIR/workspace.sh"
# shellcheck source=scripts/lib/flock_test_support.sh
source "$SCRIPT_DIR/flock_test_support.sh"

# No `lsof` stub here, unlike `ports_test.sh`, and deliberately.
#
# That rule (ADR 0025) exists because `ports.sh`'s reclaim path resolves a
# port's occupier and SIGUSR1s it. Nothing in this suite reaches that path: it
# calls neither `allocate_ports` nor `start_gateway`, the only two callers.
#
# The suite's own probes DO shell out to `lsof`, to ask whether a frontend of
# this project is running, and a stub answering "nothing is listening" makes
# eight tests measure the wrong thing. Add one only alongside a test that can
# reach a signal.

# ── fixture helpers ────────────────────────────────────────────────────
make_pkg_dir() {
    # Creates a package dir with a package.json and (optionally) a node_modules.
    # Install-detection is content-based (see _deps_fingerprint), so what
    # matters is whether a finished install exists, not relative mtimes:
    # fresh=1 → node_modules with npm's install marker (no install needed,
    # self-heals a stamp), fresh=0 → node_modules absent (install needed).
    local dir="$1"
    local fresh="${2:-1}"   # 1 = a finished install is present
    mkdir -p "$dir"
    echo '{"name":"x"}' > "$dir/package.json"
    if [ "$fresh" = "1" ]; then
        mkdir -p "$dir/node_modules"
        echo '{}' > "$dir/node_modules/.package-lock.json"
    fi
}

# Hold <project>'s build-watch lock and publish the holder's pid, as a running
# watcher does. Sets HOLDER_PID and HOLDER_RELEASE.
hold_build_watch() { # <project>
    local dir="$1/crates/lucidos-app/.build-watch"
    mkdir -p "$dir"
    start_lock_holder "$SANDBOX" "$dir/watch.lock" ex "$dir/pid"
}

# ── Test 1: refuse npm install while THIS checkout's build-watch runs ──
test_refuses_install_when_this_checkouts_build_watch_runs() {
    echo "test: refuses install while this checkout's build-watch runs"

    # `local PROJECT_DIR` shadows the global that ensure_npm_deps reads —
    # bash dynamic scoping makes the local visible to called functions.
    local PROJECT_DIR="$SANDBOX/proj-conflict"
    make_pkg_dir "$PROJECT_DIR" 0       # no node_modules → install IS needed
    hold_build_watch "$PROJECT_DIR"

    npm() { echo "NPM_INSTALL_RAN" >&2; return 0; }
    export -f npm
    local out rc=0
    out="$(ensure_npm_deps "$PROJECT_DIR" "test deps" 2>&1)" || rc=$?
    unset -f npm
    release_lock_holder "$HOLDER_PID" "$HOLDER_RELEASE"

    if [ $rc -eq 0 ]; then fail "expected non-zero exit, got 0"; else pass "exited non-zero ($rc)"; fi
    if echo "$out" | grep -q "NPM_INSTALL_RAN"; then
        fail "npm install ran under a running build-watch"
    else
        pass "npm install did NOT run"
    fi
    if echo "$out" | grep -q "build-watch (PID $HOLDER_PID)"; then
        pass "the error names the build-watch and its pid"
    else
        fail "error message did not name the watch: $out"
    fi
}

# ── Test 1a: ENGINE_ONLY restart skips (does not abort) under a running watch ──
# The engine-only restart (CC Apply) deliberately keeps this checkout's frontend
# alive, and generate_build_outputs runs on that path. Aborting would leave the workspace with
# no engine at all, so ENGINE_ONLY skips the install, warns, and returns 0.
test_engine_only_skips_install_when_build_watch_runs() {
    echo "test: ENGINE_ONLY restart skips install (no abort) under a running build-watch"

    local PROJECT_DIR="$SANDBOX/proj-engine-only"
    make_pkg_dir "$PROJECT_DIR" 0
    # Dynamic scoping makes this visible to ensure_npm_deps, like PROJECT_DIR.
    local ENGINE_ONLY=1
    hold_build_watch "$PROJECT_DIR"

    npm() { echo "NPM_INSTALL_RAN" >&2; return 0; }
    export -f npm
    local out rc=0
    out="$(ensure_npm_deps "$PROJECT_DIR" "workspace dependencies" 2>&1)" || rc=$?
    unset -f npm
    release_lock_holder "$HOLDER_PID" "$HOLDER_RELEASE"

    if [ $rc -eq 0 ]; then pass "exited 0 (engine restart not aborted)"; else fail "expected exit 0, got $rc; output: $out"; fi
    if echo "$out" | grep -q "NPM_INSTALL_RAN"; then
        fail "npm install ran under a running build-watch"
    else
        pass "npm install did NOT run (frontend preserved)"
    fi
    if echo "$out" | grep -q "WARNING"; then pass "warned that deps changed"; else fail "expected a WARNING: $out"; fi
}

# ── Test 1b: a build-watch in a DIFFERENT checkout does not block ──
# Mirrors the CC-worktree case: the watch lock lives in its own checkout, so a
# watch in the main repo does not share node_modules with a worktree.
test_allows_install_when_another_checkouts_build_watch_runs() {
    echo "test: allows install while another checkout's build-watch runs"

    local PROJECT_DIR="$SANDBOX/proj-worktree"
    make_pkg_dir "$PROJECT_DIR" 0
    hold_build_watch "$SANDBOX/proj-main"

    npm() { echo "NPM_INSTALL_RAN" >&2; return 0; }
    export -f npm
    local out rc=0
    out="$(ensure_npm_deps "$PROJECT_DIR" "test deps" 2>&1)" || rc=$?
    unset -f npm
    release_lock_holder "$HOLDER_PID" "$HOLDER_RELEASE"

    if [ $rc -ne 0 ]; then fail "expected exit 0, got $rc; output: $out"; else pass "exited 0"; fi
    if echo "$out" | grep -q "NPM_INSTALL_RAN"; then pass "npm install ran"; else fail "npm install did not run; output: $out"; fi
}

# ── Test 2: install proceeds when no build-watch runs ──
test_installs_when_no_build_watch_runs() {
    echo "test: installs when no build-watch runs"

    local PROJECT_DIR="$SANDBOX/proj-idle"
    make_pkg_dir "$PROJECT_DIR" 0

    npm() { echo "NPM_INSTALL_RAN" >&2; return 0; }
    export -f npm
    local out rc=0
    out="$(ensure_npm_deps "$PROJECT_DIR" "test deps" 2>&1)" || rc=$?
    unset -f npm

    if [ $rc -ne 0 ]; then fail "expected exit 0, got $rc; output: $out"; else pass "exited 0"; fi
    if echo "$out" | grep -q "NPM_INSTALL_RAN"; then pass "npm install ran"; else fail "npm install did not run; output: $out"; fi
}

# ── Test 3: a pidfile whose watch lock nobody holds does not block ──
test_an_unlocked_build_watch_pidfile_does_not_block() {
    echo "test: a build-watch pidfile with no lock held does not block install"

    local PROJECT_DIR="$SANDBOX/proj-ghost-watch"
    make_pkg_dir "$PROJECT_DIR" 0
    mkdir -p "$PROJECT_DIR/crates/lucidos-app/.build-watch"
    echo "$$" > "$PROJECT_DIR/crates/lucidos-app/.build-watch/pid"

    npm() { echo "NPM_INSTALL_RAN" >&2; return 0; }
    export -f npm
    local out rc=0
    out="$(ensure_npm_deps "$PROJECT_DIR" "test deps" 2>&1)" || rc=$?
    unset -f npm

    if [ $rc -ne 0 ]; then fail "an unlocked pidfile blocked install; output: $out"; else pass "it did not block install"; fi
}

# ── Test 4: no install needed → no check, no failure ──
test_no_install_needed_skips_check() {
    echo "test: no install needed skips the build-watch check"

    local PROJECT_DIR="$SANDBOX/proj-fresh"
    make_pkg_dir "$PROJECT_DIR" 1       # node_modules present → no install needed
    hold_build_watch "$PROJECT_DIR"

    npm() { echo "NPM_INSTALL_RAN" >&2; return 0; }
    export -f npm
    local out rc=0
    out="$(ensure_npm_deps "$PROJECT_DIR" "test deps" 2>&1)" || rc=$?
    unset -f npm
    release_lock_holder "$HOLDER_PID" "$HOLDER_RELEASE"

    if [ $rc -ne 0 ]; then fail "no-install case errored; output: $out"; else pass "no-install case exited 0"; fi
    if echo "$out" | grep -q "NPM_INSTALL_RAN"; then fail "npm install ran when not needed"; else pass "npm install did not run"; fi
}

# ── Test 5: resolve_workspace_path expands bare names like web-dev.sh ──
test_resolves_bare_name_to_home_workspaces() {
    echo "test: resolve_workspace_path expands bare name"

    rm -rf "${HOME:?}/workspaces"
    mkdir -p "$HOME/workspaces/dev/.lucidos"
    # Get canonical path for comparison (handles macOS /private/var symlinks)
    local expected
    expected="$(cd "$HOME/workspaces/dev" && pwd)"

    WORKSPACE="dev"
    if resolve_workspace_path; then
        if [ "$WORKSPACE" = "$expected" ]; then
            pass "bare name 'dev' resolved to $expected"
        else
            fail "expected $expected, got $WORKSPACE"
        fi
    else
        fail "resolve_workspace_path returned non-zero for existing workspace"
    fi
}

test_resolves_absolute_path_unchanged() {
    echo "test: resolve_workspace_path accepts absolute paths"

    rm -rf "${HOME:?}/workspaces"
    mkdir -p "$HOME/workspaces/dev/.lucidos"
    local expected
    expected="$(cd "$HOME/workspaces/dev" && pwd)"

    WORKSPACE="$HOME/workspaces/dev"
    if resolve_workspace_path && [ "$WORKSPACE" = "$expected" ]; then
        pass "absolute path resolved to $expected"
    else
        fail "expected $expected, got $WORKSPACE"
    fi
}

test_errors_on_missing_workspace() {
    echo "test: resolve_workspace_path errors when workspace missing"

    rm -rf "${HOME:?}/workspaces"

    WORKSPACE="ghost-ws"
    local err
    err="$(resolve_workspace_path 2>&1)"
    local rc=$?

    if [ $rc -eq 0 ]; then
        fail "expected non-zero exit for missing workspace, got 0"
    else
        pass "errored on missing workspace (rc=$rc)"
    fi
    if echo "$err" | grep -q "Workspace not found"; then
        pass "error message names the missing workspace"
    else
        fail "error message did not mention 'Workspace not found': $err"
    fi
}

test_does_not_create_directories() {
    echo "test: resolve_workspace_path is side-effect free (no mkdir)"

    rm -rf "${HOME:?}/workspaces"

    WORKSPACE="never-existed"
    resolve_workspace_path 2>/dev/null || true

    # Critical: stop / status must NOT create the workspace dir as a
    # side effect of resolving its name. resolve_workspace (the mutating
    # variant) does that for start scripts.
    if [ -d "$HOME/workspaces/never-existed" ]; then
        fail "resolve_workspace_path created directory: $HOME/workspaces/never-existed"
    else
        pass "no directory created"
    fi
}

# ── Test: workspace member with hoisted root deps does not need install ──
# Reproduces the "fresh git worktree" failure: in npm-workspace setups, the
# per-package node_modules dir only holds Vite cache. A fresh worktree has no
# per-package node_modules but the root one is fully populated. The check
# must not falsely report "missing" in that case.
test_workspace_member_with_root_deps_skips_install() {
    echo "test: workspace member skips install when root node_modules present"

    rm -rf "${HOME:?}/workspaces"

    # Build the layout that npm workspaces produces:
    #   $root/package.json   { "workspaces": ["pkg-member"] }
    #   $root/node_modules/  (populated by `npm install` at root)
    #   $root/pkg-member/package.json   ( workspace member )
    #   $root/pkg-member/   ( NO node_modules — this is the fresh-worktree case )
    local root="$SANDBOX/wsroot-$$"
    mkdir -p "$root/node_modules" "$root/pkg-member"
    echo '{}' > "$root/node_modules/.package-lock.json"
    cat > "$root/package.json" <<'EOF'
{"private":true,"workspaces":["pkg-member"]}
EOF
    echo '{"name":"pkg-member"}' > "$root/pkg-member/package.json"

    # A running build-watch in this checkout: if the function decides install
    # IS needed, this would make it exit 1.
    local PROJECT_DIR="$root"
    hold_build_watch "$root"

    npm() { echo "NPM_INSTALL_RAN" >&2; return 0; }
    export -f npm

    local out rc
    out="$(ensure_npm_deps "$root/pkg-member" "frontend deps" 2>&1)"
    rc=$?

    release_lock_holder "$HOLDER_PID" "$HOLDER_RELEASE"
    unset -f npm

    if [ $rc -ne 0 ]; then
        fail "fresh-worktree case errored (rc=$rc); output: $out"
    else
        pass "fresh-worktree case exited 0"
    fi
    if echo "$out" | grep -q "NPM_INSTALL_RAN"; then
        fail "npm install ran when root deps already present"
    else
        pass "npm install did not run"
    fi
}

# ── Test: workspace member install triggers when package.json content changes ──
# Once a stamp exists, a genuine content change (someone added a dep) must
# still fire an install. Detection is by content fingerprint, not mtime.
test_workspace_member_install_when_package_json_bumped() {
    echo "test: workspace member triggers install when its package.json content changes"

    rm -rf "${HOME:?}/workspaces"

    local root="$SANDBOX/wsroot2-$$"
    mkdir -p "$root/node_modules" "$root/pkg-member"
    echo '{}' > "$root/node_modules/.package-lock.json"
    cat > "$root/package.json" <<'EOF'
{"private":true,"workspaces":["pkg-member"]}
EOF
    echo '{"name":"pkg-member"}' > "$root/pkg-member/package.json"

    npm() { echo "NPM_INSTALL_RAN" >&2; return 0; }
    export -f npm

    # First call self-heals the stamp from the original content (no install).
    ensure_npm_deps "$root/pkg-member" "frontend deps" >/dev/null 2>&1

    # Now genuinely change the dependency set.
    echo '{"name":"pkg-member","dependencies":{"new":"^1"}}' > "$root/pkg-member/package.json"

    local out rc
    out="$(ensure_npm_deps "$root/pkg-member" "frontend deps" 2>&1)"
    rc=$?
    unset -f npm

    if [ $rc -ne 0 ]; then
        fail "expected exit 0, got $rc; output: $out"
    else
        pass "exited 0"
    fi
    if echo "$out" | grep -q "NPM_INSTALL_RAN"; then
        pass "npm install ran on dependency change"
    else
        fail "npm install did not run; output: $out"
    fi
}

# ── Test: no-op rewrite (mtime bump, identical content) does NOT reinstall ──
# THE regression: a git checkout / worktree add / CC change apply rewrites
# package.json, bumping its mtime without changing a byte. The old mtime check
# read that as "package.json changed" and — with a frontend running — aborted
# the engine-only restart, leaving the engine down. Content-based detection
# must see an identical fingerprint and do nothing.
test_noop_rewrite_does_not_trigger_install() {
    echo "test: no-op rewrite (mtime bump, same content) does not reinstall"

    rm -rf "${HOME:?}/workspaces"

    local PROJECT_DIR="$SANDBOX/proj-noop"
    local pkg="$PROJECT_DIR"
    mkdir -p "$pkg/node_modules"
    echo '{}' > "$pkg/node_modules/.package-lock.json"
    echo '{"name":"x"}' > "$pkg/package.json"

    npm() { echo "NPM_INSTALL_RAN" >&2; return 0; }
    export -f npm

    # Establish the stamp from the original content.
    ensure_npm_deps "$pkg" "test deps" >/dev/null 2>&1

    # A running build-watch in THIS checkout would force exit 1 if the
    # function wrongly decided an install was needed.
    hold_build_watch "$PROJECT_DIR"

    # No-op rewrite: identical bytes, fresh mtime (what `git checkout` does).
    echo '{"name":"x"}' > "$pkg/package.json"
    touch "$pkg/package.json"

    local out rc
    out="$(ensure_npm_deps "$pkg" "test deps" 2>&1)"
    rc=$?

    release_lock_holder "$HOLDER_PID" "$HOLDER_RELEASE"
    unset -f npm

    if [ $rc -ne 0 ]; then
        fail "no-op rewrite aborted (rc=$rc); output: $out"
    else
        pass "no-op rewrite exited 0"
    fi
    if echo "$out" | grep -q "NPM_INSTALL_RAN"; then
        fail "npm install ran on a no-op rewrite"
    else
        pass "npm install did not run"
    fi
}

# ── Test: missing root node_modules in a workspace setup still triggers install ──
test_workspace_member_install_when_root_node_modules_missing() {
    echo "test: workspace member triggers install when root node_modules missing"

    rm -rf "${HOME:?}/workspaces"

    local root="$SANDBOX/wsroot3-$$"
    mkdir -p "$root/pkg-member"
    cat > "$root/package.json" <<'EOF'
{"private":true,"workspaces":["pkg-member"]}
EOF
    echo '{"name":"pkg-member"}' > "$root/pkg-member/package.json"
    # No root/node_modules and no per-pkg node_modules → install needed.

    npm() { echo "NPM_INSTALL_RAN" >&2; return 0; }
    export -f npm

    local out rc
    out="$(ensure_npm_deps "$root/pkg-member" "frontend deps" 2>&1)"
    rc=$?
    unset -f npm

    if [ $rc -ne 0 ]; then
        fail "expected exit 0, got $rc; output: $out"
    else
        pass "exited 0"
    fi
    if echo "$out" | grep -q "NPM_INSTALL_RAN"; then
        pass "npm install ran when root node_modules missing"
    else
        fail "npm install did not run; output: $out"
    fi
}

# ── Test: an interrupted install is reinstalled, never stamped as current ──
# `npm ci` wipes node_modules before it installs. Killed mid-way, it leaves a
# tree with no stamp and packages missing, which every later build fails on.
test_interrupted_install_is_reinstalled() {
    echo "test: node_modules without npm's install marker is reinstalled"

    rm -rf "${HOME:?}/workspaces"

    local pkg="$SANDBOX/pkg-interrupted"
    mkdir -p "$pkg/node_modules/.vite-temp"
    echo '{"name":"x"}' > "$pkg/package.json"

    npm() { echo "NPM_INSTALL_RAN" >&2; return 0; }
    export -f npm

    local out rc
    out="$(ensure_npm_deps "$pkg" "test deps" 2>&1)"
    rc=$?
    unset -f npm

    if [ $rc -ne 0 ]; then
        fail "expected exit 0, got $rc; output: $out"
    else
        pass "exited 0"
    fi
    if echo "$out" | grep -q "NPM_INSTALL_RAN"; then
        pass "npm install ran on an incomplete tree"
    else
        fail "an incomplete tree was trusted; output: $out"
    fi
}

test_gateway_scope_ignores_the_optin() {
    echo "test: the opt-out CANNOT buy a worktree-rooted machine-global gateway"

    # The asymmetry that matters. `web-dev.sh -w e2e-test -b` from a worktree stops
    # the user's gateway and relaunches it pinned to a throwaway checkout, where it
    # outlives the session and serves every workspace a frozen dist/ — the
    # 2026-07-26 incident. No test workflow justifies that, so `gateway` scope
    # ignores LUCIDOS_ALLOW_WORKTREE_STACK entirely.
    local wt="$SANDBOX/ws3/.lucidos/worktrees/thread-gw"
    mkdir -p "$wt"

    local out rc
    out="$(LUCIDOS_ALLOW_WORKTREE_STACK=1 assert_stack_not_worktree_pinned "$wt" gateway 2>&1)" && rc=0 || rc=$?
    if [ "${rc:-0}" -ne 0 ]; then
        pass "gateway scope refuses even with the opt-in set"
    else
        fail "opt-in bought a worktree-rooted gateway — the exact incident mechanism"
    fi
    case "$out" in
        *"does NOT apply"*) pass "message says the opt-in is powerless here" ;;
        *) fail "message should state the opt-in does not apply: $out" ;;
    esac
    case "$out" in
        *"./scripts/e2e.sh"*) pass "message points at the e2e scripts (no start step needed)" ;;
        *) fail "message should point at ./scripts/e2e.sh: $out" ;;
    esac
    # The advice must not send the reader in a circle: LUCIDOS_NO_GATEWAY alone
    # drops to `stack` scope, which still refuses without LUCIDOS_ALLOW_WORKTREE_STACK
    # (web-dev.sh does not set it). If the message names NO_GATEWAY it must name both.
    case "$out" in
        *LUCIDOS_NO_GATEWAY*)
            case "$out" in
                *LUCIDOS_NO_GATEWAY*LUCIDOS_ALLOW_WORKTREE_STACK*)
                    pass "the gateway-less hint names BOTH opt-ins" ;;
                *) fail "message suggests LUCIDOS_NO_GATEWAY alone, which is still refused: $out" ;;
            esac ;;
        *) pass "no circular NO_GATEWAY-only hint" ;;
    esac
}

test_stack_scope_still_honours_the_optin() {
    echo "test: stack scope (direct engine, what e2e uses) still honours the opt-in"

    # scripts/lib/e2e.sh calls start_engine directly, so it only ever reaches
    # this scope. Regressing it breaks e2e's frontend.
    local wt="$SANDBOX/ws4/.lucidos/worktrees/thread-e2e"
    mkdir -p "$wt"
    if LUCIDOS_ALLOW_WORKTREE_STACK=1 assert_stack_not_worktree_pinned "$wt" >/dev/null 2>&1; then
        pass "default scope permits a worktree-rooted direct-engine stack"
    else
        fail "opt-in no longer works for the direct-engine path (this breaks e2e)"
    fi
    # And the scope defaults to `stack` when omitted.
    if LUCIDOS_ALLOW_WORKTREE_STACK=1 assert_stack_not_worktree_pinned "$wt" stack >/dev/null 2>&1; then
        pass "explicit stack scope behaves the same as the default"
    else
        fail "explicit stack scope diverged from the default"
    fi
}

test_gateway_scope_allows_a_real_checkout() {
    echo "test: gateway scope does not fire for a normal checkout"
    local proj="$SANDBOX/projects/lucidos-gw"
    mkdir -p "$proj"
    if assert_stack_not_worktree_pinned "$proj" gateway >/dev/null 2>&1; then
        pass "real checkout allowed in gateway scope"
    else
        fail "gateway scope wrongly refused a real checkout"
    fi
}

# ── the shared build-watch singleton, by locks (ADR 0438) ──────────────
# Hermetic. Each checkout is a sandbox whose `node` is a stand-in watcher: it
# records its pid, publishes dist/index.html, then holds on, so it keeps the
# watch lock the real wrapper hands it. Every pause is a FIFO, never a sleep.

# Make <project> a checkout with a stand-in watcher.
arrange_build_watch_checkout() { # <project>
    local project="$1"
    mkdir -p "$project/crates/lucidos-app/dist" "$project/bin"
    cat >"$project/bin/node" <<'EOF'
#!/bin/bash
echo "$$" >>"$BW_SPAWN_LOG"
mkdir -p dist && echo built >dist/index.html
exec sleep 600
EOF
    chmod +x "$project/bin/node"
    echo stale >"$project/crates/lucidos-app/dist/stale"
    : >"$project/spawns"
}

# Run start_frontend_built for <project>, as a launch does. Call it inside a
# subshell, which may first override a hook.
launch_frontend() { # <project>
    PROJECT_DIR="$1"
    FRONTEND_DIR="$1/crates/lucidos-app"
    ENGINE_BIN=""
    PATH="$1/bin:$PATH"
    export BW_SPAWN_LOG="$1/spawns"
    start_frontend_built
}

teardown_for() { # <project>
    (PROJECT_DIR="$1"; teardown_shared_build_watch_if_idle)
}

spawn_count() { wc -l <"$1/spawns" | tr -d ' '; }

test_two_launches_start_one_build_watch() {
    echo "test: two concurrent launches start one build-watch, and only it clears dist/"
    local p="$SANDBOX/proj-bw-race" line
    arrange_build_watch_checkout "$p"
    channel_open 6 "$SANDBOX/bw-race.events"
    channel_open 5 "$SANDBOX/bw-race.go"

    # Launch A probes, finds no watch, and pauses there, inside start.lock.
    (
        # shellcheck disable=SC2317 # called by start_frontend_built
        build_watch_running() {
            local rc=0
            lock_is_held "$(build_watch_lock)" || rc=$?
            echo "A probed $rc" >&6
            read -r _ <&5
            return "$rc"
        }
        launch_frontend "$p"
    ) >"$SANDBOX/bw-race.a" 2>&1 &
    local a=$!
    read -r line <&6
    if [ "$line" = "A probed 1" ]; then pass "A found no watch"; else fail "A saw: $line"; fi

    # Launch B must wait for A's decision rather than make its own.
    (
        # shellcheck disable=SC2317 # called by flock_open
        _flock_waiting() { echo "B waiting" >&6; }
        launch_frontend "$p"
    ) >"$SANDBOX/bw-race.b" 2>&1 &
    local b=$!
    read -r line <&6
    if [ "$line" = "B waiting" ]; then pass "B waited on the start lock"; else fail "B saw: $line"; fi

    echo go >&5
    wait "$a" || fail "launch A failed: $(cat "$SANDBOX/bw-race.a")"
    wait "$b" || fail "launch B failed: $(cat "$SANDBOX/bw-race.b")"
    channel_close 5
    channel_close 6

    if [ "$(spawn_count "$p")" = 1 ]; then pass "exactly one watcher started"; else fail "$(spawn_count "$p") watchers started"; fi
    if grep -q "Reusing shared frontend build-watch (PID $(cat "$p/spawns"))" "$SANDBOX/bw-race.b"; then pass "B reused A's watcher"; else fail "B: $(cat "$SANDBOX/bw-race.b")"; fi
    if [ ! -e "$p/crates/lucidos-app/dist/stale" ]; then pass "the owner cleared the old dist/"; else fail "the old dist/ survived the new watcher"; fi

    teardown_for "$p" >/dev/null
}

test_a_reuse_leaves_dist_alone() {
    echo "test: a launch that reuses a running watch never touches dist/"
    local p="$SANDBOX/proj-bw-reuse"
    arrange_build_watch_checkout "$p"
    (launch_frontend "$p") >/dev/null 2>&1 || fail "the first launch failed"
    echo keep >"$p/crates/lucidos-app/dist/keep"
    local out
    out="$( (launch_frontend "$p") 2>&1)" || fail "the second launch failed: $out"
    case "$out" in *"Reusing shared frontend build-watch"*) pass "reused" ;; *) fail "$out" ;; esac
    if [ -e "$p/crates/lucidos-app/dist/keep" ]; then pass "dist/ untouched"; else fail "a reuse cleared dist/"; fi
    if [ "$(spawn_count "$p")" = 1 ]; then pass "no second watcher"; else fail "$(spawn_count "$p") watchers"; fi
    teardown_for "$p" >/dev/null
}

test_a_consumer_keeps_the_build_watch() {
    echo "test: teardown keeps the watch while a consumer holds the consumer lock"
    local p="$SANDBOX/proj-bw-consumer" watcher out
    arrange_build_watch_checkout "$p"
    (launch_frontend "$p") >/dev/null 2>&1 || fail "the launch failed"
    watcher="$(cat "$p/spawns")"

    # An engine serving this dist/ holds the consumer lock for its life.
    start_lock_holder "$SANDBOX" "$p/crates/lucidos-app/.build-watch/consumers.lock" sh
    out="$(teardown_for "$p")"
    if kill -0 "$watcher" 2>/dev/null; then pass "the watch survives a live consumer"; else fail "killed the watch under a consumer"; fi
    case "$out" in *Keeping*) pass "it says why it kept the watch" ;; *) fail "silent refusal: $out" ;; esac
    release_lock_holder "$HOLDER_PID" "$HOLDER_RELEASE"

    teardown_for "$p" >/dev/null
    if kill -0 "$watcher" 2>/dev/null; then
        fail "the watch outlived the teardown that stopped it"
    else
        pass "with no consumer left, the watch is gone when teardown returns"
    fi
}

test_a_watch_from_before_the_lock_is_stopped_once() {
    echo "test: a watch from before the watch lock is stopped, and only that one"
    local PROJECT_DIR="$SANDBOX/proj-bw-pre-lock" out
    mkdir -p "$PROJECT_DIR/crates/lucidos-app/.build-watch"
    echo "$$" >"$PROJECT_DIR/crates/lucidos-app/.build-watch/pid"
    # `ps` and `kill` are stubbed, so nothing real is listed or signalled.
    out="$( ps() { echo "node dev-build-watch.mjs"; }; kill() { echo "killed $*"; }
        stop_pre_lock_build_watch )"
    case "$out" in *"killed $$"*) pass "the old watcher is stopped" ;; *) fail "not stopped: $out" ;; esac
    out="$( ps() { echo "node some-other-script.mjs"; }; kill() { echo "killed $*"; }
        stop_pre_lock_build_watch )"
    case "$out" in *killed*) fail "stopped a node process that is not the watch: $out" ;; *) pass "another node process is left alone" ;; esac
    : >"$PROJECT_DIR/crates/lucidos-app/.build-watch/watch.lock"
    out="$( ps() { echo "node dev-build-watch.mjs"; }; kill() { echo "killed $*"; }
        stop_pre_lock_build_watch )"
    case "$out" in *killed*) fail "acted although the watch lock exists: $out" ;; *) pass "once the lock exists, it does nothing" ;; esac
}

test_a_launch_in_flight_keeps_the_build_watch() {
    echo "test: teardown never races a launch that is starting the watch"
    local p="$SANDBOX/proj-bw-midstart" line out
    arrange_build_watch_checkout "$p"
    (launch_frontend "$p") >/dev/null 2>&1 || fail "the first launch failed"
    channel_open 6 "$SANDBOX/bw-mid.events"
    channel_open 5 "$SANDBOX/bw-mid.go"
    (
        # shellcheck disable=SC2317 # called by start_frontend_built
        build_watch_running() {
            local rc=0
            lock_is_held "$(build_watch_lock)" || rc=$?
            echo "probed $rc" >&6
            read -r _ <&5
            return "$rc"
        }
        launch_frontend "$p"
    ) >/dev/null 2>&1 &
    local launch=$!
    read -r line <&6
    if [ "$line" = "probed 0" ]; then pass "the launch is deciding to reuse the watch"; else fail "saw: $line"; fi

    out="$(teardown_for "$p")"
    case "$out" in *Keeping*) pass "the teardown refused" ;; *) fail "teardown ran mid-launch: $out" ;; esac
    if kill -0 "$(cat "$p/spawns")" 2>/dev/null; then pass "the watch the launch is reusing survives"; else fail "killed the watch the launch was reusing"; fi

    echo go >&5
    wait "$launch" || fail "the paused launch failed"
    channel_close 5
    channel_close 6
    teardown_for "$p" >/dev/null
}

test_shared_pg_sql_quoting() {
    echo "test: shared Postgres SQL quoting"

    if [ "$(_shared_pg_ident lucidos_dev-project)" = '"lucidos_dev-project"' ]; then
        pass "shared database identifier quoted"
    else
        fail "shared database identifier quote mismatch"
    fi
    if [ "$(_shared_pg_literal "a'b")" = "'a''b'" ]; then
        pass "shared database literal escapes apostrophes"
    else
        fail "shared database literal quote mismatch: $(_shared_pg_literal "a'b")"
    fi
}

test_legacy_pg_volume_layout_detects_parent_pgdata() {
    echo "test: legacy PG volume layout detects PG18 parent PGDATA"

    docker() {
        case "$*" in
            *"test -f /v/18/docker/PG_VERSION"*) return 0 ;;
            *) return 1 ;;
        esac
    }

    local layout
    layout="$(_legacy_pg_volume_layout legacy-volume)"
    unset -f docker

    if [ "$layout" = "parent:18" ]; then
        pass "PG18 parent-layout volume detected"
    else
        fail "unexpected parent-layout detection: ${layout:-<empty>}"
    fi
}

test_legacy_pg_volume_layout_detects_root_pgdata() {
    echo "test: legacy PG volume layout detects root PGDATA"

    docker() {
        case "$*" in
            *"test -f /v/18/docker/PG_VERSION"*) return 1 ;;
            *"cat /v/PG_VERSION"*) echo "17"; return 0 ;;
            *) return 1 ;;
        esac
    }

    local layout
    layout="$(_legacy_pg_volume_layout legacy-volume)"
    unset -f docker

    if [ "$layout" = "root:17" ]; then
        pass "root-layout volume detected"
    else
        fail "unexpected root-layout detection: ${layout:-<empty>}"
    fi
}

test_swap_ports_writes_shared_database_url() {
    echo "test: swap_ports exports the shared database URL and writes no secret"

    local PROJECT_DIR="$SANDBOX/proj-shared-db-url"
    local FRONTEND_DIR="$PROJECT_DIR/crates/lucidos-app"
    mkdir -p "$FRONTEND_DIR" "$HOME/workspaces/dev-project/.lucidos"

    WORKSPACE="$HOME/workspaces/dev-project"
    VITE_PORT=5188
    PG_PORT=5544
    LUCIDOS_TLS_CERT=""
    LUCIDOS_TLS_KEY=""

    swap_ports >/dev/null

    local ports_file="$WORKSPACE/.lucidos/ports"
    if grep -qx "PG_DATABASE=lucidos_dev-project" "$ports_file"; then
        pass "ports file records shared workspace database"
    else
        fail "ports file missing PG_DATABASE=lucidos_dev-project: $(cat "$ports_file")"
    fi
    # The ports file is 0644 inside a workspace other local users can read,
    # so the cluster's secret must never land in it.
    local secret
    secret="$(shared_pg_password)"
    if grep -q "DATABASE_URL" "$ports_file" || grep -qF "$secret" "$ports_file"; then
        fail "ports file carries the shared cluster's secret: $(cat "$ports_file")"
    else
        pass "ports file carries no secret"
    fi
    if [ "$DATABASE_URL" = "postgres://lucidos:$secret@localhost:5544/lucidos_dev-project" ]; then
        pass "DATABASE_URL exported for direct engine mode, with the minted secret"
    else
        fail "unexpected DATABASE_URL export: ${DATABASE_URL:-<unset>}"
    fi
}

# detect_tls used to APPEND its line, so a workspace relaunched N times carried
# N PROTO lines and a reader took whichever it parsed last.
test_relaunching_leaves_exactly_one_proto_line() {
    echo "test: relaunching leaves exactly one PROTO line"

    local PROJECT_DIR="$SANDBOX/proj-proto-once"
    mkdir -p "$PROJECT_DIR" "$HOME/workspaces/proto-once/.lucidos"
    WORKSPACE="$HOME/workspaces/proto-once"
    LUCIDOS_TLS_CERT=""
    LUCIDOS_TLS_KEY=""

    detect_tls
    detect_tls
    detect_tls

    local count
    count=$(grep -c '^PROTO=' "$WORKSPACE/.lucidos/ports")
    if [ "$count" = "1" ]; then
        pass "three launches wrote one PROTO line"
    else
        fail "expected one PROTO line, got $count: $(cat "$WORKSPACE/.lucidos/ports")"
    fi
}

# It also skipped the write entirely when no ports file existed yet, so a launch
# that reached here first recorded no scheme at all.
test_detect_tls_records_the_scheme_with_no_ports_file_yet() {
    echo "test: detect_tls records the scheme even with no ports file yet"

    local PROJECT_DIR="$SANDBOX/proj-proto-fresh"
    mkdir -p "$PROJECT_DIR" "$HOME/workspaces/proto-fresh/.lucidos"
    WORKSPACE="$HOME/workspaces/proto-fresh"
    LUCIDOS_TLS_CERT=""
    LUCIDOS_TLS_KEY=""
    rm -f "$WORKSPACE/.lucidos/ports"

    detect_tls

    if grep -qx "PROTO=http" "$WORKSPACE/.lucidos/ports" 2>/dev/null; then
        pass "the scheme was recorded into a file that did not exist"
    else
        fail "no PROTO line: $(cat "$WORKSPACE/.lucidos/ports" 2>/dev/null)"
    fi
}

# Every launcher runs detect_tls before swap_ports, and swap_ports rewrites the
# same file. A reader that finds no PROTO falls back to https, so a dropped
# line makes every caller fail the TLS handshake against a plain http engine.
test_swap_ports_keeps_the_proto_detect_tls_wrote() {
    echo "test: swap_ports keeps the PROTO line detect_tls wrote"

    local PROJECT_DIR="$SANDBOX/proj-proto"
    local FRONTEND_DIR="$PROJECT_DIR/crates/lucidos-app"
    mkdir -p "$FRONTEND_DIR" "$HOME/workspaces/proto-project/.lucidos"

    WORKSPACE="$HOME/workspaces/proto-project"
    VITE_PORT=5190
    PG_PORT=5545
    LUCIDOS_TLS_CERT=""
    LUCIDOS_TLS_KEY=""

    local ports_file="$WORKSPACE/.lucidos/ports"
    echo "API_PORT=$VITE_PORT" > "$ports_file"

    detect_tls
    swap_ports >/dev/null

    if [ "$PROTO" != "http" ]; then
        fail "fixture expected a cert-less detect_tls, got PROTO=${PROTO:-<unset>}"
        return
    fi
    if grep -qx "PROTO=http" "$ports_file"; then
        pass "ports file still records the detected scheme"
    else
        fail "swap_ports dropped PROTO: $(cat "$ports_file")"
    fi
}

# ── adopt_into_gateway: register through the gateway, never its file ──
# `curl` is shadowed in a subshell, so no request leaves this process. It records
# what it was asked and answers with the canned status and body.
adopt_with() { # <http-code> <body>
    printf '%s' "$2" >"$SANDBOX/adopt-body"
    printf '%s' "$1" >"$SANDBOX/adopt-code"
    : >"$SANDBOX/adopt-args"
    mkdir -p "$SANDBOX/ws-adopt/.lucidos"
    (
        set -e
        WORKSPACE="$SANDBOX/ws-adopt"; ENGINE_PORT=5273; PROTO=https; GATEWAY_PORT=5251
        # shellcheck disable=SC2317 # called by adopt_into_gateway
        curl() {
            printf '%s\n' "$@" >>"$SANDBOX/adopt-args"
            printf '%s\n%s' "$(cat "$SANDBOX/adopt-body")" "$(cat "$SANDBOX/adopt-code")"
        }
        adopt_into_gateway >/dev/null 2>"$SANDBOX/adopt-err"
        echo "id=$GATEWAY_WS_ID"
    )
}

test_adopt_registers_the_dir_and_port_through_the_gateway() {
    echo "test: adopt_into_gateway posts the dir and port, and takes the id from the answer"
    local out
    out="$(adopt_with 200 '{"workspace":{"id":"ws-adopt","port":5273}}')"
    if [ "$out" = "id=ws-adopt" ]; then
        pass "GATEWAY_WS_ID comes from the gateway's answer"
    else
        fail "got '$out'"
    fi
    if grep -qx "https://localhost:5251/~/api/v1/control/workspaces/adopt" "$SANDBOX/adopt-args"; then
        pass "posted to the adopt endpoint"
    else
        fail "unexpected request: $(cat "$SANDBOX/adopt-args")"
    fi
    if grep -q "\"dir\": \"$SANDBOX/ws-adopt\"" "$SANDBOX/adopt-args" \
        && grep -q '"port": 5273' "$SANDBOX/adopt-args"; then
        pass "the body names the directory and the engine's port"
    else
        fail "unexpected body: $(cat "$SANDBOX/adopt-args")"
    fi
    if grep -q "autostart\|\"name\"" "$SANDBOX/adopt-args"; then
        fail "the body overrides what the picker owns"
    else
        pass "name and autostart are left to the gateway"
    fi
    if [ -e "$(gateway_data_dir)/config/workspaces.json" ]; then
        fail "the launcher wrote the gateway's registry file"
    else
        pass "nothing wrote workspaces.json"
    fi
}

test_adopt_refusal_fails_the_launch() {
    echo "test: a refused adopt fails the launch and quotes the gateway"
    local out
    out="$(adopt_with 409 '{"error":"port 5273 belongs to workspace '"'"'myws'"'"'"}')"
    if [ -z "$out" ]; then
        pass "the launch stopped"
    else
        fail "carried on after a refusal: $out"
    fi
    if grep -q "belongs to workspace 'myws'" "$SANDBOX/adopt-err"; then
        pass "the gateway's reason is quoted"
    else
        fail "stderr was: $(cat "$SANDBOX/adopt-err")"
    fi
    out="$(adopt_with 000 '')"
    if [ -z "$out" ]; then
        pass "no answer fails too"
    else
        fail "carried on with no gateway: $out"
    fi
}

test_refuses_install_when_this_checkouts_build_watch_runs
test_engine_only_skips_install_when_build_watch_runs
test_allows_install_when_another_checkouts_build_watch_runs
test_installs_when_no_build_watch_runs
test_an_unlocked_build_watch_pidfile_does_not_block
test_no_install_needed_skips_check
test_workspace_member_with_root_deps_skips_install
test_workspace_member_install_when_package_json_bumped
test_noop_rewrite_does_not_trigger_install
test_workspace_member_install_when_root_node_modules_missing
test_interrupted_install_is_reinstalled
test_resolves_bare_name_to_home_workspaces
test_resolves_absolute_path_unchanged
test_errors_on_missing_workspace
test_does_not_create_directories
test_shared_pg_sql_quoting
test_legacy_pg_volume_layout_detects_parent_pgdata
test_legacy_pg_volume_layout_detects_root_pgdata
test_swap_ports_writes_shared_database_url
test_swap_ports_keeps_the_proto_detect_tls_wrote
test_relaunching_leaves_exactly_one_proto_line
test_detect_tls_records_the_scheme_with_no_ports_file_yet
test_adopt_registers_the_dir_and_port_through_the_gateway
test_adopt_refusal_fails_the_launch

# ── worktree-pinned stack guard ────────────────────────────────────────
# Regression cover for the 2026-07-26 incident: the whole stack (gateway binary,
# engine binary, LUCIDOS_STATIC_DIR) was running out of an orphaned CC worktree,
# so every frontend-only Apply silently served a frozen dist/.
# See docs/plans/2026-07-26-worktree-pinned-stack-guard.md.

test_worktree_predicate_classifies_paths() {
    echo "test: path_is_in_cc_worktree classifies worktree vs real checkout paths"

    local p ok=1
    # Inside a CC worktree → true. Note the predicate must NOT stat: an orphaned
    # worktree may no longer exist, which is exactly when the guard must fire.
    for p in \
        "/Users/me/workspaces/dev/.lucidos/worktrees/thread-abc123" \
        "/Users/me/workspaces/dev/.lucidos/worktrees/thread-abc123/crates/lucidos-app" \
        "/w/.lucidos/worktrees"
    do
        path_is_in_cc_worktree "$p" || { fail "should be worktree: $p"; ok=0; }
    done
    # A real checkout → false, including paths that merely mention the words.
    for p in \
        "/Users/me/projects/lucidos" \
        "/Users/me/projects/lucidos/crates/lucidos-app" \
        "/Users/me/worktrees/lucidos" \
        "/Users/me/projects/.lucidos-worktrees/x"
    do
        path_is_in_cc_worktree "$p" && { fail "should NOT be worktree: $p"; ok=0; }
    done
    [ "$ok" = "1" ] && pass "predicate classifies both directions"
}

test_refuses_worktree_pinned_stack() {
    echo "test: a worktree-rooted checkout is refused with an actionable message"

    local wt="$SANDBOX/ws/.lucidos/worktrees/thread-dead/crates"
    mkdir -p "$wt"
    local out rc
    out="$(LUCIDOS_ALLOW_WORKTREE_STACK='' assert_stack_not_worktree_pinned \
             "$SANDBOX/ws/.lucidos/worktrees/thread-dead" 2>&1)" && rc=0 || rc=$?

    if [ "${rc:-0}" -ne 0 ]; then
        pass "refused with non-zero exit"
    else
        fail "worktree-rooted checkout was allowed"
    fi
    # The message has to be actionable, not just a refusal.
    case "$out" in
        *"web-dev.sh -w"*) pass "message names the command to run" ;;
        *) fail "message lacks the corrective command: $out" ;;
    esac
    case "$out" in
        *LUCIDOS_ALLOW_WORKTREE_STACK*) pass "message names the opt-in" ;;
        *) fail "message lacks the opt-in escape hatch: $out" ;;
    esac
}

test_worktree_stack_allowed_with_explicit_optin() {
    echo "test: LUCIDOS_ALLOW_WORKTREE_STACK=1 keeps the e2e path working"

    # This is the contract e2e depends on — CC sessions run
    # `web-dev.sh -w e2e-test -b` from inside their own worktree.
    if LUCIDOS_ALLOW_WORKTREE_STACK=1 assert_stack_not_worktree_pinned \
         "$SANDBOX/ws/.lucidos/worktrees/thread-live" >/dev/null 2>&1; then
        pass "opt-in permits a worktree-rooted stack"
    else
        fail "opt-in did not permit a worktree-rooted stack (this breaks e2e)"
    fi
}

test_worktree_error_names_the_real_checkout() {
    echo "test: the refusal resolves the real checkout from the worktree .git file"

    local wt="$SANDBOX/ws2/.lucidos/worktrees/thread-x"
    mkdir -p "$wt"
    # A linked worktree's .git is a file pointing back at the main checkout.
    echo "gitdir: $SANDBOX/realcheckout/.git/worktrees/thread-x" > "$wt/.git"

    local out
    out="$(LUCIDOS_ALLOW_WORKTREE_STACK='' assert_stack_not_worktree_pinned "$wt" 2>&1)" || true
    case "$out" in
        *"cd $SANDBOX/realcheckout"*) pass "names the real checkout to cd into" ;;
        *) fail "did not resolve the real checkout: $out" ;;
    esac
}

test_real_checkout_is_not_refused() {
    echo "test: a normal checkout passes the guard untouched"

    local proj="$SANDBOX/projects/lucidos"
    mkdir -p "$proj"
    if LUCIDOS_ALLOW_WORKTREE_STACK='' assert_stack_not_worktree_pinned "$proj" >/dev/null 2>&1; then
        pass "real checkout allowed"
    else
        fail "real checkout was wrongly refused"
    fi
}

# ── Published launch binaries (ADR 0022) ───────────────────────────────
# Regression cover for the 2026-07-26 root cause: every cargo variant in the
# checkout uplifts to ONE `target/<profile>/lucidos-engine`, so launching from
# it ran (and compared against) whatever landed there last: another commit,
# another feature configuration. Builds now publish into
# `.launch/<profile>/<variant>/` and launch from there.
# See docs/plans/2026-07-27-launch-binary-published-per-variant.md.
#
# The dir sits OUTSIDE `target/`, which is the 2026-08-13 regression cover:
# `cargo clean` wipes `target/` wholesale, and the launch dir holds the
# `lucidos` CLI that the engine puts on PATH for every spawned trigger and
# coding-agent session. One inline `cargo clean` in the nightly orchestrator
# left the workspace with no CLI for eight hours.
# See docs/plans/2026-08-13-launch-binaries-survive-cargo-clean.md.

# A stand-in for a built binary: prints `$id` for `--build-id`, like the real
# `lucidos-engine --build-id` the verification step reads.
make_build_id_stub() {
    local path="$1" id="$2"
    mkdir -p "$(dirname "$path")"
    # shellcheck disable=SC2016 # ${1:-} belongs to the GENERATED script, so it must not expand here
    printf '#!/bin/bash\n[ "${1:-}" = "--build-id" ] && printf "%%s\\n" "%s"\nexit 0\n' "$id" > "$path"
    chmod +x "$path"
}

# A build-id stub that also answers `--build-id --source-state` with $3.
make_source_state_stub() {
    local path="$1" id="$2" state="$3"
    mkdir -p "$(dirname "$path")"
    # shellcheck disable=SC2016 # ${1:-} and ${2:-} belong to the GENERATED script
    printf '#!/bin/bash\n[ "${1:-}" = "--build-id" ] || exit 0\nif [ "${2:-}" = "--source-state" ]; then printf "%%s\\n" "%s"; else printf "%%s\\n" "%s"; fi\n' \
        "$state" "$id" > "$path"
    chmod +x "$path"
}

test_launch_bin_dir_is_per_profile_and_variant() {
    echo "test: the launch dir is keyed by BOTH profile and feature variant"

    local PROJECT_DIR="$SANDBOX/proj-launchdir"

    local got
    got="$(RELEASE="" ENGINE_BUILD_FEATURES="" launch_bin_dir)"
    if [ "$got" = "$PROJECT_DIR/.launch/debug/plain" ]; then
        pass "plain debug build publishes to .launch/debug/plain"
    else
        fail "unexpected plain debug launch dir: $got"
    fi

    # The pairing that matters: e2e (release + e2e-test-hooks) and a dev
    # workspace (debug + plain) must resolve to DISJOINT directories, so a
    # hooks-enabled engine, whose push transport is an in-process stub, can
    # never become what a dev workspace launches.
    local e2e_dir dev_dir
    e2e_dir="$(RELEASE=1 ENGINE_BUILD_FEATURES="e2e-test-hooks" launch_bin_dir)"
    dev_dir="$(RELEASE="" ENGINE_BUILD_FEATURES="" launch_bin_dir)"
    if [ "$e2e_dir" = "$PROJECT_DIR/.launch/release/e2e-test-hooks" ]; then
        pass "e2e publishes to .launch/release/e2e-test-hooks"
    else
        fail "unexpected e2e launch dir: $e2e_dir"
    fi
    if [ "$e2e_dir" != "$dev_dir" ]; then
        pass "e2e and dev launch dirs are disjoint"
    else
        fail "e2e and dev share a launch dir, the whole collision is back"
    fi

    # LUCIDOS_E2E_DEBUG=1 drops e2e to the debug profile; it must still not
    # land on the plain dev binary.
    got="$(RELEASE="" ENGINE_BUILD_FEATURES="e2e-test-hooks" launch_bin_dir)"
    if [ "$got" = "$PROJECT_DIR/.launch/debug/e2e-test-hooks" ]; then
        pass "debug e2e stays out of the plain debug launch dir"
    else
        fail "unexpected debug e2e launch dir: $got"
    fi

    # Multiple features collapse into one component, and the slug can never
    # escape the launch dir (it is a path component).
    got="$(ENGINE_BUILD_FEATURES="a b" engine_build_variant_slug)"
    if [ "$got" = "a_b" ]; then
        pass "a multi-feature list becomes one path component"
    else
        fail "unexpected multi-feature slug: $got"
    fi
    got="$(ENGINE_BUILD_FEATURES="../escape" engine_build_variant_slug)"
    if [ "$got" = "escape" ]; then
        pass "slug strips path separators and dots"
    else
        fail "slug did not sanitize traversal: $got"
    fi
}

test_launch_dir_is_outside_cargo_target() {
    echo "test: no launch dir is under target/, so cargo clean cannot remove it"

    local PROJECT_DIR="$SANDBOX/proj-launch-outside"

    # `cargo clean` deletes `target/` wholesale. The launch dir holds the
    # `lucidos` CLI the engine puts on PATH for every spawned trigger and
    # coding-agent session (`find_lucidos_cli_dir`), so a launch dir under
    # `target/` means one `cargo clean` disables the workspace: triggers fail
    # with "No such file or directory: 'lucidos'" and `run_coding_agent` cannot
    # even spawn the child that would rebuild it. That is the 2026-08-13 outage.
    local profile variant dir
    for profile in debug release; do
        for variant in plain e2e-test-hooks; do
            dir="$(launch_bin_dir "$profile" "$variant")"
            case "$dir" in
                */target/*)
                    fail "launch dir is under target/, cargo clean would wipe it: $dir"
                    continue
                    ;;
            esac
            case "$dir" in
                "$PROJECT_DIR"/*) ;;
                *)
                    # Inside the CHECKOUT is the real requirement (ADR 0022):
                    # `paths::repo_root` walks ancestors for `scripts/web-dev.sh`,
                    # and ADR 0021's worktree refusal is a substring path test.
                    # A dir outside the checkout breaks the first and launders a
                    # worktree binary past the second.
                    fail "launch dir escaped the checkout: $dir"
                    continue
                    ;;
            esac
            pass "$profile/$variant publishes outside target/ but inside the checkout"
        done
    done
}

test_worktree_refusal_still_sees_a_worktree_launch_dir() {
    echo "test: a worktree's own launch dir is still refused (ADR 0021)"

    # Moving the launch dir out of `target/` must not move it out of the
    # worktree, or ADR 0021's pure path test would stop matching and a
    # worktree-built binary would be launderable into a long-lived stack.
    local wt="$SANDBOX/ws-refusal/.lucidos/worktrees/thread-abc"
    local PROJECT_DIR="$wt"
    local dir
    dir="$(launch_bin_dir debug plain)"
    if path_is_in_cc_worktree "$dir"; then
        pass "a worktree-rooted launch dir is still classified as a worktree"
    else
        fail "worktree launch dir escaped the ADR 0021 path test: $dir"
    fi
    if path_is_in_cc_worktree "$dir/lucidos-engine"; then
        pass "the published engine path inside it is classified too"
    else
        fail "worktree engine path escaped the ADR 0021 path test"
    fi
}

test_publish_launch_binary_is_atomic_and_executable() {
    echo "test: publishing replaces the launch binary completely and leaves no temp"

    local src="$SANDBOX/publish-src/lucidos-engine"
    local dst="$SANDBOX/publish-dst/launch/plain/lucidos-engine"
    mkdir -p "$(dirname "$src")"
    printf 'NEW-BINARY' > "$src"
    chmod +x "$src"
    mkdir -p "$(dirname "$dst")"
    printf 'OLD-BINARY' > "$dst"

    if publish_launch_binary "$src" "$dst"; then
        pass "publish reported success"
    else
        fail "publish of an existing source failed"
    fi
    if [ "$(cat "$dst")" = "NEW-BINARY" ]; then
        pass "launch binary replaced with the freshly built one"
    else
        fail "launch binary not replaced: $(cat "$dst")"
    fi
    if [ -x "$dst" ]; then
        pass "published binary is executable"
    else
        fail "published binary lost its exec bit"
    fi
    # A leftover temp would mean a non-atomic path: a spawn could catch a
    # half-written binary, which is exactly what the rename prevents.
    if [ -z "$(find "$(dirname "$dst")" -name '*.tmp.*' 2>/dev/null)" ]; then
        pass "no temp file left behind"
    else
        fail "publish left a temp file in the launch dir"
    fi
}

test_publish_failure_preserves_the_previous_binary() {
    echo "test: a failed publish never leaves the launch path missing (never strands)"

    local src_dir="$SANDBOX/publish-fail-src"
    local dst_dir="$SANDBOX/publish-fail-dst/launch/plain"
    mkdir -p "$src_dir" "$dst_dir"
    # The engine did not build (aborted / killed mid-compile); gateway + CLI did.
    printf 'GW' > "$src_dir/lucidos-gateway"
    printf 'CLI' > "$src_dir/lucidos"
    printf 'PREVIOUS-ENGINE' > "$dst_dir/lucidos-engine"

    # Keep the suite hermetic: the real signer is macOS + keychain dependent and
    # would either print a setup hint or try to codesign these 2-byte fixtures.
    sign_engine_binary() { :; }
    local rc=0
    publish_launch_binaries "$src_dir" "$dst_dir" || rc=$?
    unset -f sign_engine_binary
    if [ "$rc" -ne 0 ]; then
        pass "publish reports failure when the engine is missing"
    else
        fail "publish claimed success with no engine to publish"
    fi
    if [ "$(cat "$dst_dir/lucidos-engine")" = "PREVIOUS-ENGINE" ]; then
        pass "the previously published engine is left intact"
    else
        fail "a failed publish clobbered the working engine binary"
    fi
    if [ -z "$(find "$dst_dir" -name '*.tmp.*' 2>/dev/null)" ]; then
        pass "no temp file left behind on the failure path"
    else
        fail "failed publish left a temp file"
    fi
    # The `lucidos` CLI must land next to the engine — find_lucidos_cli_dir
    # walks up from the engine's exe dir, and without it the lucidos-cli skill
    # is not installed into coding-agent sessions.
    if [ -x "$dst_dir/lucidos" ]; then
        pass "the lucidos CLI is published next to the engine"
    else
        fail "the lucidos CLI was not published alongside the engine"
    fi
}

test_publish_prunes_only_dead_publish_temps() {
    echo "test: a SIGKILLed publish's temp is collected, a live one's is not"

    # An explicit Rebuild SIGKILLs the whole build process group
    # (engine_version::BuildProcessGroupGuard), which no trap catches, so a kill
    # inside the copy/sign window strands a ~250 MB temp that nothing collects.
    # The next publish sweeps it, but ONLY when its pid is dead: a human
    # `web-dev.sh -b` is not coordinated by the engine's build lock, and eating
    # its in-flight temp would break that build's rename.
    local src_dir="$SANDBOX/prune-src"
    local dst_dir="$SANDBOX/prune-dst/launch/plain"
    mkdir -p "$src_dir" "$dst_dir"
    printf 'ENGINE' > "$src_dir/lucidos-engine"
    printf 'GW' > "$src_dir/lucidos-gateway"
    printf 'CLI' > "$src_dir/lucidos"

    # A pid that is certainly gone: spawn one and reap it.
    sh -c 'exit 0' &
    local dead_pid=$!
    wait "$dead_pid" 2>/dev/null || true
    # A pid that is certainly alive for the duration. Deliberately NOT `$$`:
    # `publish_launch_binary` names its own temp `$dst.tmp.$$`, so this shell's
    # pid would collide with the publish under test and be consumed by its
    # rename, proving nothing about the sweep.
    ( exec sleep 30 ) & local live_pid=$!

    local dead_temp="$dst_dir/lucidos-engine.tmp.$dead_pid"
    local live_temp="$dst_dir/lucidos-engine.tmp.$live_pid"
    local junk_temp="$dst_dir/lucidos-engine.tmp.notapid"
    printf 'ORPHAN' > "$dead_temp"
    printf 'IN-FLIGHT' > "$live_temp"
    printf 'JUNK' > "$junk_temp"

    sign_engine_binary() { :; }
    publish_launch_binaries "$src_dir" "$dst_dir"
    unset -f sign_engine_binary

    if [ ! -e "$dead_temp" ]; then
        pass "a temp whose publisher is gone is collected"
    else
        fail "the orphaned publish temp was left to accumulate"
    fi
    if [ -e "$live_temp" ]; then
        pass "a temp belonging to a live publisher is left alone"
    else
        fail "swept an in-flight publish's temp, whose rename would now fail"
    fi
    if [ -e "$junk_temp" ]; then
        pass "an unparseable suffix is left alone rather than guessed at"
    else
        fail "deleted a temp whose suffix is not a pid"
    fi
    if [ "$(cat "$dst_dir/lucidos-engine")" = "ENGINE" ]; then
        pass "the publish itself still lands"
    else
        fail "pruning interfered with the publish"
    fi
    kill "$live_pid" 2>/dev/null || true
}

test_publish_signs_the_temp_before_the_rename() {
    echo "test: signing happens on the temp copy, never on the published path"

    # `codesign --force` rewrites its target IN PLACE. Signing the already-
    # renamed binary would leave a peer engine spawning a half-rewritten file —
    # defeating the atomicity the rename exists for. Assert the ordering by
    # recording what sign_engine_binary was handed and whether the destination
    # existed at that moment.
    local src="$SANDBOX/sign-order-src/lucidos-engine"
    local dst="$SANDBOX/sign-order-dst/launch/plain/lucidos-engine"
    mkdir -p "$(dirname "$src")"
    printf 'BINARY' > "$src"

    local signed_path="" dst_existed_at_sign_time=""
    sign_engine_binary() {
        signed_path="$1"
        [ -e "$dst" ] && dst_existed_at_sign_time=yes || dst_existed_at_sign_time=no
    }

    publish_launch_binary "$src" "$dst" sign
    unset -f sign_engine_binary

    case "$signed_path" in
        "$dst".tmp.*) pass "signed the temp copy, not the launch path" ;;
        "") fail "sign_engine_binary was never called for a 'sign' publish" ;;
        *) fail "signed the wrong path: $signed_path" ;;
    esac
    if [ "$dst_existed_at_sign_time" = "no" ]; then
        pass "the launch path did not exist yet when signing ran"
    else
        fail "signing ran after the rename — a peer could spawn a mid-codesign binary"
    fi

    # And a publish without the flag must not sign at all (the CLI).
    signed_path=""
    sign_engine_binary() { signed_path="$1"; }
    publish_launch_binary "$src" "$SANDBOX/sign-order-dst/launch/plain/lucidos"
    unset -f sign_engine_binary
    if [ -z "$signed_path" ]; then
        pass "an unsigned publish does not invoke the signer"
    else
        fail "unexpectedly signed $signed_path"
    fi
}

test_published_build_state_classifies_against_head() {
    echo "test: published_build_state tells 'stale' from 'unknown'"

    local PROJECT_DIR="$SANDBOX/proj-buildstate"
    mkdir -p "$PROJECT_DIR"
    git -C "$PROJECT_DIR" init -q 2>/dev/null
    git -C "$PROJECT_DIR" config user.email "test@example.com"
    git -C "$PROJECT_DIR" config user.name "Test"
    printf 'x' > "$PROJECT_DIR/a.txt"
    git -C "$PROJECT_DIR" add . >/dev/null 2>&1
    git -C "$PROJECT_DIR" commit -qm first >/dev/null 2>&1
    local short full
    short="$(git -C "$PROJECT_DIR" rev-parse --short HEAD)"
    full="$(git -C "$PROJECT_DIR" rev-parse HEAD)"

    local stub="$SANDBOX/buildstate/lucidos-engine"

    make_build_id_stub "$stub" "$short"
    if [ "$(published_build_state "$stub")" = "current" ]; then
        pass "HEAD's short sha reads as current"
    else
        fail "HEAD's short sha misread: $(published_build_state "$stub")"
    fi

    # Dirty engine source stamps `<sha>-<diffhash>` — still the same commit.
    make_build_id_stub "$stub" "$short-0badc0ffee123456"
    if [ "$(published_build_state "$stub")" = "current" ]; then
        pass "a dirty-tree suffix is still current"
    else
        fail "dirty-tree id misread as not current"
    fi

    # The abbreviation trap: the two sides can be abbreviated to different
    # lengths, so the comparison must be a prefix test in BOTH directions —
    # otherwise every build would "fail" verification and rebuild forever.
    make_build_id_stub "$stub" "$full"
    if [ "$(published_build_state "$stub")" = "current" ]; then
        pass "a longer abbreviation of the same commit is current"
    else
        fail "prefix comparison is not symmetric"
    fi

    # A binary from a different commit: the case the retry exists for. This
    # stub cannot answer --source-state, like a binary that predates the flag.
    make_build_id_stub "$stub" "0123456789abc"
    if [ "$(published_build_state "$stub")" = "stale" ]; then
        pass "a different commit reads as stale when the binary cannot say more"
    else
        fail "a different commit was not detected as stale"
    fi

    # HEAD moved only by files that need no restart: no second build.
    make_source_state_stub "$stub" "0123456789abc" "current"
    if [ "$(published_build_state "$stub")" = "current" ]; then
        pass "a move that needs no restart keeps the binary current"
    else
        fail "a no-restart move still reads as $(published_build_state "$stub")"
    fi

    # HEAD moved by engine source: the retry still runs.
    make_source_state_stub "$stub" "0123456789abc" "stale"
    if [ "$(published_build_state "$stub")" = "stale" ]; then
        pass "a move that needs a restart reads as stale"
    else
        fail "a restart move misread as $(published_build_state "$stub")"
    fi

    # The binary could not tell (git failed): keep the old answer, stale.
    make_source_state_stub "$stub" "0123456789abc" "unknown"
    if [ "$(published_build_state "$stub")" = "stale" ]; then
        pass "an unanswerable source check stays stale"
    else
        fail "an unanswerable source check misread as $(published_build_state "$stub")"
    fi

    # Indeterminate must never read as stale: a rebuild cannot fix any of these,
    # so treating them as a mismatch would double every build forever.
    make_build_id_stub "$stub" "src-0123456789abcdef"
    if [ "$(published_build_state "$stub")" = "unknown" ]; then
        pass "a no-git src-… id is unknown, not stale"
    else
        fail "src-… id misclassified"
    fi
    make_build_id_stub "$stub" ""
    if [ "$(published_build_state "$stub")" = "unknown" ]; then
        pass "an empty build id is unknown"
    else
        fail "empty build id misclassified"
    fi
    printf '#!/bin/bash\nexit 1\n' > "$stub"; chmod +x "$stub"
    if [ "$(published_build_state "$stub")" = "unknown" ]; then
        pass "a binary that cannot report its id is unknown"
    else
        fail "unreadable build id misclassified"
    fi
    if [ "$(published_build_state "$SANDBOX/buildstate/does-not-exist")" = "unknown" ]; then
        pass "a missing binary is unknown"
    else
        fail "missing binary misclassified"
    fi

    # No git in the checkout (shipped tarball / CI container).
    local PROJECT_DIR_NOGIT="$SANDBOX/proj-nogit"
    mkdir -p "$PROJECT_DIR_NOGIT"
    make_build_id_stub "$stub" "0123456789abc"
    if [ "$(PROJECT_DIR="$PROJECT_DIR_NOGIT" published_build_state "$stub")" = "unknown" ]; then
        pass "a non-git checkout is unknown, never stale"
    else
        fail "non-git checkout misclassified"
    fi
}

test_locate_prefers_published_and_falls_back() {
    echo "test: the no-build path prefers the published binary, then warns"

    local PROJECT_DIR="$SANDBOX/proj-locate"
    local ENGINE_BIN="" GATEWAY_BIN="" out rc=0
    # Both dirs explicitly: the launch dir no longer lives under `target/`, so
    # creating it no longer creates cargo's uplift dir as a side effect.
    mkdir -p "$PROJECT_DIR/.launch/debug/plain" "$PROJECT_DIR/target/debug"
    : > "$PROJECT_DIR/target/debug/lucidos-engine"
    : > "$PROJECT_DIR/target/debug/lucidos-gateway"
    : > "$PROJECT_DIR/.launch/debug/plain/lucidos-engine"
    : > "$PROJECT_DIR/.launch/debug/plain/lucidos-gateway"

    out="$(RELEASE="" locate_launch_binaries 2>&1)"
    RELEASE="" locate_launch_binaries >/dev/null 2>&1
    if [ "$ENGINE_BIN" = "$PROJECT_DIR/.launch/debug/plain/lucidos-engine" ]; then
        pass "published binary preferred over cargo's uplift path"
    else
        fail "did not prefer the published binary: $ENGINE_BIN"
    fi
    if [ "$GATEWAY_BIN" = "$PROJECT_DIR/.launch/debug/plain/lucidos-gateway" ]; then
        pass "engine and gateway come from the same directory"
    else
        fail "gateway did not pair with the engine: $GATEWAY_BIN"
    fi
    case "$out" in
        *WARNING*) fail "warned while a published binary was available: $out" ;;
        *) pass "no warning when launching a published binary" ;;
    esac

    # No published binary yet (first launch after this change, or a hand-run
    # `cargo build`): fall back rather than strand the workspace, but say so.
    rm -f "$PROJECT_DIR/.launch/debug/plain/lucidos-engine"
    out="$(RELEASE="" locate_launch_binaries 2>&1)"
    RELEASE="" locate_launch_binaries >/dev/null 2>&1
    if [ "$ENGINE_BIN" = "$PROJECT_DIR/target/debug/lucidos-engine" ]; then
        pass "falls back to cargo's uplift path instead of stranding"
    else
        fail "no fallback to the uplift path: $ENGINE_BIN"
    fi
    case "$out" in
        *WARNING*"-b"*) pass "the fallback warns and names -b" ;;
        *) fail "fallback did not warn actionably: $out" ;;
    esac

    # A launch dir holding ONLY the engine is a half-finished build: selecting it
    # would pair a fresh engine with a missing gateway and fail much later, with
    # a far less obvious error than "run with -b".
    mkdir -p "$PROJECT_DIR/.launch/debug/plain"
    : > "$PROJECT_DIR/.launch/debug/plain/lucidos-engine"
    rm -f "$PROJECT_DIR/.launch/debug/plain/lucidos-gateway"
    RELEASE="" locate_launch_binaries >/dev/null 2>&1
    if [ "$ENGINE_BIN" = "$PROJECT_DIR/target/debug/lucidos-engine" ]; then
        pass "a gateway-less launch dir is skipped, not half-selected"
    else
        fail "selected an incomplete launch dir: $ENGINE_BIN"
    fi
    rm -f "$PROJECT_DIR/.launch/debug/plain/lucidos-engine"

    # A release request still falls back to a debug build, as it always has.
    if RELEASE=1 locate_launch_binaries >/dev/null 2>&1 &&
       [ "$ENGINE_BIN" = "$PROJECT_DIR/target/debug/lucidos-engine" ]; then
        pass "a release request still falls back to debug"
    else
        fail "release→debug fallback regressed: $ENGINE_BIN"
    fi

    # A featured build looks in its OWN launch dir, not the plain one.
    mkdir -p "$PROJECT_DIR/.launch/debug/e2e-test-hooks"
    : > "$PROJECT_DIR/.launch/debug/e2e-test-hooks/lucidos-engine"
    : > "$PROJECT_DIR/.launch/debug/e2e-test-hooks/lucidos-gateway"
    if RELEASE="" ENGINE_BUILD_FEATURES="e2e-test-hooks" locate_launch_binaries >/dev/null 2>&1 &&
       [ "$ENGINE_BIN" = "$PROJECT_DIR/.launch/debug/e2e-test-hooks/lucidos-engine" ]; then
        pass "a featured build locates its own variant dir"
    else
        fail "featured build did not use its variant dir: $ENGINE_BIN"
    fi

    # Nothing on disk at all keeps the historical, actionable error.
    local PROJECT_DIR_EMPTY="$SANDBOX/proj-locate-empty"
    mkdir -p "$PROJECT_DIR_EMPTY"
    out="$(PROJECT_DIR="$PROJECT_DIR_EMPTY" RELEASE="" locate_launch_binaries 2>&1)" && rc=0 || rc=$?
    if [ "${rc:-0}" -ne 0 ]; then
        pass "an empty checkout still fails"
    else
        fail "an empty checkout reported success"
    fi
    case "$out" in
        *"No engine binary found. Run with -b to build."*) pass "keeps the historical error text" ;;
        *) fail "error text changed: $out" ;;
    esac
}

test_launch_bin_dir_is_per_profile_and_variant
test_launch_dir_is_outside_cargo_target
test_worktree_refusal_still_sees_a_worktree_launch_dir
test_publish_launch_binary_is_atomic_and_executable
test_publish_failure_preserves_the_previous_binary
test_publish_prunes_only_dead_publish_temps
test_publish_signs_the_temp_before_the_rename
test_published_build_state_classifies_against_head
test_locate_prefers_published_and_falls_back
test_worktree_predicate_classifies_paths
test_refuses_worktree_pinned_stack
test_worktree_stack_allowed_with_explicit_optin
test_worktree_error_names_the_real_checkout
test_real_checkout_is_not_refused
test_gateway_scope_ignores_the_optin
test_stack_scope_still_honours_the_optin
test_gateway_scope_allows_a_real_checkout
test_two_launches_start_one_build_watch
test_a_reuse_leaves_dist_alone
test_a_consumer_keeps_the_build_watch
test_a_launch_in_flight_keeps_the_build_watch
test_a_watch_from_before_the_lock_is_stopped_once

# ── direct-engine network bind ─────────────────────────────────────────
# Nothing authenticates a directly-launched engine's port, so what this helper
# pins IS the security boundary. Before ADR 0096's gap was closed it forced
# all-interfaces, and every e2e run put that port on the network.

reset_bind_env() {
    unset LUCIDOS_BIND_ADDR LUCIDOS_BIND_ALL LUCIDOS_BIND_LOOPBACK SCRIPT_NAME
    rm -f "$HOME/.lucidos/network.toml"
}

test_e2e_pins_the_engine_to_loopback() {
    echo "test: apply_dev_engine_bind pins e2e to loopback"
    reset_bind_env
    SCRIPT_NAME=e2e
    apply_dev_engine_bind

    if [ "${LUCIDOS_BIND_ADDR:-}" = "127.0.0.1" ]; then
        pass "e2e pins LUCIDOS_BIND_ADDR to loopback"
    else
        fail "expected 127.0.0.1, got ${LUCIDOS_BIND_ADDR:-<unset>}"
    fi
    if [ -z "${LUCIDOS_BIND_ALL:-}" ]; then
        pass "e2e sets no all-interfaces flag"
    else
        fail "LUCIDOS_BIND_ALL leaked: $LUCIDOS_BIND_ALL"
    fi
}

test_e2e_pin_survives_a_developers_network_toml() {
    echo "test: a personal network.toml does not move the e2e bind"
    reset_bind_env
    mkdir -p "$HOME/.lucidos"
    printf '[gateway]\nbind = "all"\n' > "$HOME/.lucidos/network.toml"
    SCRIPT_NAME=e2e
    apply_dev_engine_bind

    # The pin outranks the file in net_config.rs. That is what keeps a
    # tailnet-bound developer's e2e run reachable on localhost.
    if [ "${LUCIDOS_BIND_ADDR:-}" = "127.0.0.1" ]; then
        pass "e2e still pins loopback with a network.toml present"
    else
        fail "expected 127.0.0.1, got ${LUCIDOS_BIND_ADDR:-<unset>}"
    fi
    rm -f "$HOME/.lucidos/network.toml"
}

test_the_bind_pin_never_claims_to_be_behind_a_gateway() {
    echo "test: apply_dev_engine_bind never sets the behind_gateway signal"
    reset_bind_env
    SCRIPT_NAME=e2e
    apply_dev_engine_bind

    # LUCIDOS_BIND_LOOPBACK would bind loopback too. It would also tell the
    # engine it is fronted, which moves the API base URL handed to subprocesses
    # and suppresses the lucidos.toml port pin.
    if [ -z "${LUCIDOS_BIND_LOOPBACK:-}" ]; then
        pass "LUCIDOS_BIND_LOOPBACK stays unset"
    else
        fail "behind_gateway signal set: $LUCIDOS_BIND_LOOPBACK"
    fi
}

test_a_direct_launch_widens_nothing_by_default() {
    echo "test: a non-e2e direct launch opts into no network bind"
    reset_bind_env
    apply_dev_engine_bind

    if [ -z "${LUCIDOS_BIND_ALL:-}" ] && [ -z "${LUCIDOS_BIND_ADDR:-}" ]; then
        pass "the engine's own loopback default is left to apply"
    else
        fail "launch widened the bind: ALL=${LUCIDOS_BIND_ALL:-<unset>} ADDR=${LUCIDOS_BIND_ADDR:-<unset>}"
    fi
}

test_a_direct_launch_keeps_an_explicit_bind() {
    echo "test: a developer's own bind export survives a direct launch"
    reset_bind_env
    # The network.toml is what makes this case discriminate. The old helper
    # unset LUCIDOS_BIND_ALL on exactly this branch, so without the file the
    # assertion below would hold against the old code too.
    mkdir -p "$HOME/.lucidos"
    printf '[gateway]\nbind = "loopback"\n' > "$HOME/.lucidos/network.toml"
    export LUCIDOS_BIND_ALL=1
    apply_dev_engine_bind

    # Widening a direct engine is the developer's call, and net_config.rs
    # already ranks an env var above the file.
    if [ "${LUCIDOS_BIND_ALL:-}" = "1" ]; then
        pass "an exported LUCIDOS_BIND_ALL is left alone"
    else
        fail "the launch dropped an explicit bind: ${LUCIDOS_BIND_ALL:-<unset>}"
    fi
    unset LUCIDOS_BIND_ALL
    rm -f "$HOME/.lucidos/network.toml"
}

test_a_direct_launch_defers_to_network_toml() {
    echo "test: a direct launch leaves network.toml to the engine resolver"
    reset_bind_env
    mkdir -p "$HOME/.lucidos"
    printf '[gateway]\nbind = "100.64.0.1"\n' > "$HOME/.lucidos/network.toml"
    apply_dev_engine_bind

    if [ -z "${LUCIDOS_BIND_ALL:-}" ] && [ -z "${LUCIDOS_BIND_ADDR:-}" ]; then
        pass "no env var masks the configured bind"
    else
        fail "env masked the file: ALL=${LUCIDOS_BIND_ALL:-<unset>} ADDR=${LUCIDOS_BIND_ADDR:-<unset>}"
    fi
    rm -f "$HOME/.lucidos/network.toml"
}

test_e2e_pins_the_engine_to_loopback
test_e2e_pin_survives_a_developers_network_toml
test_the_bind_pin_never_claims_to_be_behind_a_gateway
test_a_direct_launch_widens_nothing_by_default
test_a_direct_launch_keeps_an_explicit_bind
test_a_direct_launch_defers_to_network_toml

# ── select_stale_dev_script_pids ───────────────────────────────────────
#
# The Claude-Code-shaped fixture ADR 0025 requires of every process-selection
# guard, the same one `webkit_reaper_test.sh` and `e2e_lock_test.sh` carry. A
# coding agent held the engine's thread history in a roughly 22 KB
# `--append-system-prompt` argument, so a thread merely DISCUSSING a dev script
# and this workspace path used to match. The caller sends `pkill -P` and then a
# `kill`, so a match takes that session's children with it.
#
# SYNTHETIC_PS is fail-closed: empty means "no candidates", never "go ask the
# real ps". That fallback is what killed two sessions on 2026-08-03.
SYNTHETIC_PS=""
_dev_script_list_processes() {
    [ -n "$SYNTHETIC_PS" ] || return 0
    printf '%s\n' "$SYNTHETIC_PS"
}

selected_pids() {
    SYNTHETIC_PS="$1" WORKSPACE="$2" select_stale_dev_script_pids | tr '\n' ' ' | sed 's/ $//'
}

test_a_dev_script_for_this_workspace_is_selected() {
    local got
    got="$(selected_pids "4242 /bin/bash /repo/scripts/web-dev.sh -w /ws/mine -b" "/ws/mine")"
    if [ "$got" = "4242" ]; then
        pass "an interpreter-invoked dev script for this workspace matches"
    else
        fail "expected 4242, got '$got'"
    fi

    got="$(selected_pids "4243 /repo/scripts/tauri-dev.sh -w /ws/mine" "/ws/mine")"
    if [ "$got" = "4243" ]; then
        pass "a shebang-invoked dev script matches"
    else
        fail "expected 4243, got '$got'"
    fi
}

test_a_coding_agent_merely_mentioning_a_dev_script_is_not_selected() {
    # argv[0] is the agent binary and argv[1] is a flag, so neither token is a
    # dev script. The workspace path and the script name both appear, deep in
    # the prompt, exactly as they do in a real session.
    local agent="9001 /usr/local/bin/claude --append-system-prompt THREAD HISTORY: run ./scripts/web-dev.sh -w /ws/mine -b to restart"
    local got
    got="$(selected_pids "$agent" "/ws/mine")"
    if [ -z "$got" ]; then
        pass "a coding agent quoting web-dev.sh and the workspace is NOT a candidate"
    else
        fail "coding agent selected as stale dev script: '$got'"
    fi

    got="$(selected_pids "9002 /bin/bash -c echo /repo/scripts/web-dev.sh -w /ws/mine" "/ws/mine")"
    if [ -z "$got" ]; then
        pass "a bash -c whose argument names the script is NOT a candidate"
    else
        fail "bash -c selected: '$got'"
    fi
}

test_another_workspaces_dev_script_is_not_selected() {
    local got
    got="$(selected_pids "4244 /bin/bash /repo/scripts/web-dev.sh -w /ws/other -b" "/ws/mine")"
    if [ -z "$got" ]; then
        pass "a dev script for ANOTHER workspace is left alone"
    else
        fail "other workspace's script selected: '$got'"
    fi
}

test_the_engines_background_build_is_not_selected() {
    # The engine spawns this for Apply, self-heal and Retry build. A launch does
    # not replace it, and the `pkill -P` SIGTERMed its build-slot broker, so the
    # build reported exit 143 while cargo finished underneath it.
    local got
    got="$(selected_pids "4245 /bin/bash /repo/scripts/web-dev.sh -w /ws/mine --engine-build" "/ws/mine")"
    if [ -z "$got" ]; then
        pass "the engine's own --engine-build for this workspace is left alone"
    else
        fail "engine background build selected as stale dev script: '$got'"
    fi

    got="$(selected_pids "4246 /bin/bash /repo/scripts/web-dev.sh -w /ws/mine--engine-build -b" "/ws/mine--engine-build")"
    if [ "$got" = "4246" ]; then
        pass "a workspace path containing the flag's text is still selected"
    else
        fail "expected 4246, got '$got'"
    fi
}

test_init_and_self_are_never_selected() {
    local got
    got="$(selected_pids "1 /bin/bash /repo/scripts/web-dev.sh -w /ws/mine" "/ws/mine")"
    if [ -z "$got" ]; then pass "pid 1 is never a candidate"; else fail "pid 1 selected: '$got'"; fi

    got="$(selected_pids "$$ /bin/bash /repo/scripts/web-dev.sh -w /ws/mine" "/ws/mine")"
    if [ -z "$got" ]; then pass "our own pid is never a candidate"; else fail "self selected: '$got'"; fi
}

# This one pins the FIXTURE, not workspace.sh: the empty feed is swallowed by
# the override above. It is kept because that override is the guard, and a
# later edit giving it a real-`ps` fallback is the 2026-08-03 hazard returning.
# It also catches a fallback added inside the selector after the loop.
test_an_empty_feed_never_reaches_the_real_ps() {
    local got
    got="$(selected_pids "" "/ws/mine")"
    if [ -z "$got" ]; then
        pass "an empty synthetic feed yields no candidates (the seam fails closed)"
    else
        fail "empty feed selected: '$got'"
    fi
}

test_a_dev_script_for_this_workspace_is_selected
test_a_coding_agent_merely_mentioning_a_dev_script_is_not_selected
test_another_workspaces_dev_script_is_not_selected
test_the_engines_background_build_is_not_selected
test_init_and_self_are_never_selected
test_an_empty_feed_never_reaches_the_real_ps

# ── engine_gateway_port: which gateway owns this workspace ─────────────
#
# Hermetic: `ps` is shadowed as a function inside each subshell, so no real
# process is listed. It is the only host call the function makes. The engine
# lock is held by a holder process this file spawned, so the pid is trusted only
# where the test says an engine owns the workspace. DEAD_PID is above the macOS
# pid ceiling, so no process can ever carry it. Shadowing `ps` per subshell
# rather than for the whole file keeps the eight probes above reading the real
# host, as they need to.
DEAD_PID=4194304

# Run engine_gateway_port for a workspace whose engine.pid holds $1 ("none"
# writes no file), with `ps` answering the line $2. $3 "held" means an engine
# holds the workspace's lock.
gateway_port_for() {
    local pid="$1" psline="$2" lock="${3:-}" ws
    ws="$SANDBOX/gwport-$RANDOM"
    mkdir -p "$ws/.lucidos"
    [ "$pid" = "none" ] || printf '%s\n' "$pid" >"$ws/.lucidos/engine.pid"
    : >"$ws/.lucidos/engine.lock"
    if [ "$lock" = held ]; then
        start_lock_holder "$SANDBOX" "$ws/.lucidos/engine.lock" ex
    fi
    (
        # shellcheck disable=SC2317 # called through engine_gateway_port
        ps() {
            printf 'PS\n' >>"$SANDBOX/gwport-calls"
            printf '%s\n' "$psline"
        }
        engine_gateway_port "$ws"
    )
    if [ "$lock" = held ]; then
        release_lock_holder "$HOLDER_PID" "$HOLDER_RELEASE"
    fi
}

PACKAGED_PS="/Applications/Lucidos.app/Contents/MacOS/lucidos-engine \
LUCIDOS_WORKSPACE=/home/u/Library/Application Support/com.lucidos.app/workspaces/packaged \
LUCIDOS_WORKSPACE_ID=packaged LUCIDOS_GATEWAY_PORT=5252 LUCIDOS_API_PORT=59704 PATH=/bin"

test_engine_gateway_port_reads_the_packaged_gateway() {
    echo "test: engine_gateway_port reads the port off the engine being stopped"
    local got
    got="$(gateway_port_for "$$" "$PACKAGED_PS" held)"
    if [ "$got" = "5252" ]; then
        pass "a packaged workspace's engine names the gateway that owns it"
    else
        fail "want 5252, got '$got'"
    fi
}

test_engine_gateway_port_is_empty_without_a_gateway() {
    echo "test: engine_gateway_port answers nothing when there is no gateway to name"
    local got
    # LUCIDOS_NO_GATEWAY dev and the e2e harness: a live engine, no gateway var.
    got="$(gateway_port_for "$$" "/opt/lucidos/bin/lucidos-engine LUCIDOS_WORKSPACE=/ws/e2e PATH=/bin" held)"
    if [ -z "$got" ]; then
        pass "an engine started with no gateway names none"
    else
        fail "invented a port: '$got'"
    fi
}

test_engine_gateway_port_never_lists_a_dead_or_absent_pid() {
    echo "test: engine_gateway_port lists no process for a dead, absent or junk pid"
    : >"$SANDBOX/gwport-calls"
    local got
    # A live pid with no lock held is the shape of a pid the OS reused.
    for pid in "$DEAD_PID" none not-a-pid "$$"; do
        got="$(gateway_port_for "$pid" "$PACKAGED_PS")"
        [ -z "$got" ] || fail "pid '$pid' yielded '$got'"
    done
    if [ ! -s "$SANDBOX/gwport-calls" ]; then
        pass "nothing was listed, so a recycled pid can never be read as the engine"
    else
        fail "ps was called for a pid we could not confirm alive"
    fi
}

test_engine_gateway_port_reads_the_packaged_gateway
test_engine_gateway_port_is_empty_without_a_gateway
test_engine_gateway_port_never_lists_a_dead_or_absent_pid

# ── gateway_stop_status: https first, then http ────────────────────────
#
# `curl` is shadowed in a subshell and answers from a queue, one status per
# call, so the two-scheme fallback is pinned rather than assumed. No request
# leaves this process.

# Run gateway_stop_status with `curl` answering the queued statuses in $1
# (space separated). Writes the call count to $SANDBOX/curl-calls.
#
# The subshell runs under `set -e`, as stop.sh does, and the stub exits 7 on a
# `000` exactly as curl does when a connection is refused. Together those pin
# the arm that used to take the whole script down before the engine signal.
stop_status_with() {
    local queue="$1"
    printf '%s' "$queue" >"$SANDBOX/curl-queue"
    : >"$SANDBOX/curl-calls"
    (
        set -e
        # shellcheck disable=SC2317 # called through gateway_curl
        curl() {
            local seq head rest
            printf 'CALL\n' >>"$SANDBOX/curl-calls"
            seq="$(cat "$SANDBOX/curl-queue")"
            head="${seq%% *}"
            rest="${seq#* }"
            [ "$rest" = "$seq" ] && rest=""
            printf '%s' "$rest" >"$SANDBOX/curl-queue"
            printf '%s' "$head"
            [ "$head" != "000" ]
        }
        gateway_stop_status 5252 packaged
    )
}

curl_calls() { wc -l <"$SANDBOX/curl-calls" | tr -d ' '; }

test_stop_status_reports_the_first_scheme_that_answers() {
    echo "test: gateway_stop_status reports what the gateway answered"
    local got
    got="$(stop_status_with "202 000")"
    if [ "$got" = "202" ] && [ "$(curl_calls)" = "1" ]; then
        pass "https answered 202, so http is never tried"
    else
        fail "want 202 in one call, got '$got' in $(curl_calls)"
    fi

    got="$(stop_status_with "404 000")"
    if [ "$got" = "404" ]; then
        pass "a 404 is reported, not swallowed as a failure to reach anything"
    else
        fail "want 404, got '$got'"
    fi
}

test_stop_status_falls_back_to_http() {
    echo "test: gateway_stop_status falls back to http when TLS refuses"
    local got
    # curl's own 000: no HTTP answer at all. The dev gateway serves plain http
    # when no certs exist, so this is the ordinary local case, not an error.
    got="$(stop_status_with "000 202")"
    if [ "$got" = "202" ] && [ "$(curl_calls)" = "2" ]; then
        pass "http answered after https could not connect"
    else
        fail "want 202 in two calls, got '$got' in $(curl_calls)"
    fi
}

test_stop_status_says_000_when_nothing_answers() {
    echo "test: gateway_stop_status says 000 when no gateway answers"
    local got
    got="$(stop_status_with "000 000")"
    if [ "$got" = "000" ]; then
        pass "no gateway on that port reads as 000, never as success"
    else
        fail "want 000, got '$got'"
    fi

    got="$(stop_status_with " ")"
    if [ "$got" = "000" ]; then
        pass "an empty answer is 000 too, so the case arms stay exhaustive"
    else
        fail "want 000 for an empty answer, got '$got'"
    fi
}

test_stop_status_reports_the_first_scheme_that_answers
test_stop_status_falls_back_to_http
test_stop_status_says_000_when_nothing_answers

# ── desktop_window_url: the door the Tauri window loads ────────────────
#
# The bug this pins: tauri-dev.sh pointed its window at its own engine on the
# workspace port while the gateway already owned that port, so the launcher
# spawned a second engine and it died with AddrInUse.
#
# Pure string building, no host call, so the suite stays hermetic.

test_gateway_mode_window_goes_through_the_gateway() {
    echo "test: desktop_window_url uses the gateway and the workspace prefix"
    local got
    got="$(desktop_window_url 1 https 5251 dev 5173)"
    if [ "$got" = "https://localhost:5251/dev/" ]; then
        pass "gateway mode reaches the workspace at /<slug>/"
    else
        fail "want https://localhost:5251/dev/, got '$got'"
    fi

    # The trailing slash is load-bearing: the gateway resolves the first path
    # segment as the slug, and the engine stamps <base href> from the prefix.
    case "$got" in
        */dev/) pass "the prefix keeps its trailing slash" ;;
        *) fail "want a trailing slash on the prefix, got '$got'" ;;
    esac
}

test_legacy_mode_window_goes_to_the_engine_root() {
    echo "test: desktop_window_url uses the engine root without a gateway"
    local got
    got="$(desktop_window_url "" https 5251 "" 5173)"
    if [ "$got" = "https://localhost:5173" ]; then
        pass "LUCIDOS_NO_GATEWAY keeps the engine as the front door"
    else
        fail "want https://localhost:5173, got '$got'"
    fi

    got="$(desktop_window_url "" http 5251 "" 5273)"
    if [ "$got" = "http://localhost:5273" ]; then
        pass "the scheme and port both come from the caller"
    else
        fail "want http://localhost:5273, got '$got'"
    fi
}

test_a_half_set_gateway_mode_never_builds_a_prefix() {
    echo "test: desktop_window_url needs BOTH gateway signals"
    # show_banner requires GATEWAY_MODE and GATEWAY_WS_ID together. Either one
    # alone would build `/…//` or a prefix for a gateway that is not running,
    # and the window would load a URL nothing serves.
    local got
    got="$(desktop_window_url 1 https 5251 "" 5173)"
    if [ "$got" = "https://localhost:5173" ]; then
        pass "mode without a workspace id falls back to the engine root"
    else
        fail "want the engine root when the workspace id is empty, got '$got'"
    fi

    got="$(desktop_window_url "" https 5251 dev 5173)"
    if [ "$got" = "https://localhost:5173" ]; then
        pass "a workspace id without gateway mode falls back too"
    else
        fail "want the engine root when gateway mode is off, got '$got'"
    fi
}

test_gateway_mode_window_goes_through_the_gateway
test_legacy_mode_window_goes_to_the_engine_root
test_a_half_set_gateway_mode_never_builds_a_prefix

# ── engine_health_scheme: probe the scheme, never assume it ────────────
#
# `curl` is shadowed in a subshell and answers exit codes from a queue, so the
# https-then-http fallback is pinned rather than assumed. No request leaves this
# process. Same shape as the gateway_stop_status stub above.

# Exit with the next code in $SANDBOX/curl-queue, or 7 (refused) once it is
# empty. A subshell binds `curl` to this, so the real one never runs.
queued_curl() {
    local seq head rest
    seq="$(cat "$SANDBOX/curl-queue")"
    head="${seq%% *}"
    rest="${seq#* }"
    [ "$rest" = "$seq" ] && rest=""
    printf '%s' "$rest" >"$SANDBOX/curl-queue"
    return "${head:-7}"
}

# Run engine_health_scheme with `curl` exiting with the queued codes in $1.
# The subshell runs under `set -e`, as start_engine does.
health_scheme_with() {
    printf '%s' "$1" >"$SANDBOX/curl-queue"
    (
        set -e
        # shellcheck disable=SC2317 # called by engine_health_scheme
        curl() { queued_curl; }
        engine_health_scheme 5173
    )
}

test_health_scheme_reports_https_when_tls_answers() {
    echo "test: engine_health_scheme reports the scheme that answered"
    local got
    got="$(health_scheme_with "0 0")"
    if [ "$got" = "https" ]; then
        pass "https answered, so http is never tried"
    else
        fail "want https, got '$got'"
    fi
}

test_health_scheme_falls_back_to_http() {
    echo "test: engine_health_scheme falls back to http when TLS refuses"
    # The gateway-spawned engine case: dev certs make detect_tls pick https,
    # but the engine behind the gateway serves plain http. Reading it as dead
    # is what spawned a second engine onto the held port.
    local got
    got="$(health_scheme_with "35 0")"
    if [ "$got" = "http" ]; then
        pass "a plain-http engine is found, not read as dead"
    else
        fail "want http, got '$got'"
    fi
}

test_health_scheme_is_empty_and_succeeds_when_nothing_answers() {
    echo "test: engine_health_scheme is empty when no engine answers"
    local got rc
    got="$(health_scheme_with "7 7")"
    rc=$?
    if [ -z "$got" ]; then
        pass "no answer on either scheme reads as empty"
    else
        fail "want an empty scheme, got '$got'"
    fi
    # Pins the exit status the caller depends on: start_engine assigns this
    # into a variable under `set -e`, so a non-zero status aborts the launch.
    # Both the `if` with no `else` and the explicit `return 0` produce it, so
    # this guards a restructure rather than one line.
    if [ "$rc" -eq 0 ]; then
        pass "an absent engine still exits 0, so the caller survives set -e"
    else
        fail "want exit 0 when nothing answers, got $rc"
    fi
}

test_health_scheme_bounds_every_probe() {
    echo "test: engine_health_scheme bounds both probes"
    # Every gateway launch runs this probe, so an engine that accepts and never
    # answers must cost seconds, not the whole launch.
    : >"$SANDBOX/curl-args"
    (
        # shellcheck disable=SC2317 # called by engine_health_scheme
        curl() { printf '%s\n' "$*" >>"$SANDBOX/curl-args"; return 28; }
        engine_health_scheme 5173 >/dev/null
    )
    local probes bounded
    probes="$(grep -c . "$SANDBOX/curl-args")"
    bounded="$(grep -c -- '--max-time' "$SANDBOX/curl-args")"
    if [ "$probes" -eq 2 ] && [ "$bounded" -eq 2 ]; then
        pass "a timed-out https probe falls to a bounded http probe"
    else
        fail "want 2 probes with --max-time, got: $(cat "$SANDBOX/curl-args")"
    fi
}

test_health_scheme_reports_https_when_tls_answers
test_health_scheme_falls_back_to_http
test_health_scheme_is_empty_and_succeeds_when_nothing_answers
test_health_scheme_bounds_every_probe

# ── adopt_engine_scheme: the running engine owns its scheme ────────────

test_adopting_a_matching_scheme_changes_nothing() {
    echo "test: adopt_engine_scheme is silent when the scheme already matches"
    local out
    WORKSPACE="$SANDBOX/ws-adopt-match"
    ENGINE_PORT=5173
    PROTO=https
    mkdir -p "$WORKSPACE/.lucidos"
    out="$(adopt_engine_scheme https)"
    if [ -z "$out" ] && [ "$PROTO" = "https" ]; then
        pass "a matching scheme is a no-op"
    else
        fail "want silence and https, got output '$out' and PROTO '$PROTO'"
    fi
}

test_adopting_a_different_scheme_rewrites_the_ports_file() {
    echo "test: adopt_engine_scheme takes the scheme the engine really serves"
    WORKSPACE="$SANDBOX/ws-adopt-differ"
    ENGINE_PORT=5173
    PROTO=https
    mkdir -p "$WORKSPACE/.lucidos"
    printf 'API_PORT=5173\nPROTO=https\n' >"$WORKSPACE/.lucidos/ports"
    adopt_engine_scheme http >/dev/null
    if [ "$PROTO" = "http" ]; then
        pass "PROTO follows the live engine"
    else
        fail "want PROTO http, got '$PROTO'"
    fi
    # Every CLI caller reads the scheme back out of this file, so an adopted
    # scheme that only lived in this shell would strand them on the old one.
    if grep -qx 'PROTO=http' "$WORKSPACE/.lucidos/ports"; then
        pass "the ports file records the adopted scheme"
    else
        fail "ports file did not record it: $(cat "$WORKSPACE/.lucidos/ports")"
    fi
    if grep -qx 'API_PORT=5173' "$WORKSPACE/.lucidos/ports"; then
        pass "the other keys survive the rewrite"
    else
        fail "API_PORT was lost: $(cat "$WORKSPACE/.lucidos/ports")"
    fi
}

test_adopting_a_matching_scheme_changes_nothing
test_adopting_a_different_scheme_rewrites_the_ports_file

# ── record_engine_scheme: a gateway launch records the engine's scheme ─
#
# The bug this pins: a gateway-mode launch reused the running gateway and
# engine, so neither adopt_engine_scheme nor the gateway's own ports publish
# ran. The ports file kept the https detect_tls derived from .certs/, while
# the engine served plain http (ADR 0096), and every CLI call by workspace
# name then failed its TLS handshake.
#
# `curl` is shadowed in a subshell and answers from the same queue as
# health_scheme_with, so no request leaves this process.

# Seed $WORKSPACE's ports file, run record_engine_scheme under `set -e` with
# `curl` exiting with the queued codes in $1, and print the in-memory PROTO
# after. The caller sets WORKSPACE, because callers run this inside `$(...)`.
record_scheme_with() { # <curl-queue>
    mkdir -p "$WORKSPACE/.lucidos"
    printf 'API_PORT=5173\nPROTO=https\n' >"$WORKSPACE/.lucidos/ports"
    printf '%s' "$1" >"$SANDBOX/curl-queue"
    (
        set -e
        ENGINE_PORT=5173
        PROTO=https
        # shellcheck disable=SC2317 # called by engine_health_scheme
        curl() { queued_curl; }
        record_engine_scheme >/dev/null
        printf '%s' "$PROTO"
    )
}

test_a_gateway_launch_records_a_plain_http_engine() {
    echo "test: record_engine_scheme writes the scheme the engine serves"
    local proto
    WORKSPACE="$SANDBOX/ws-record-http"
    proto="$(record_scheme_with "35 0")"
    if grep -qx 'PROTO=http' "$WORKSPACE/.lucidos/ports"; then
        pass "the ports file names the plain-http engine"
    else
        fail "want PROTO=http in the ports file, got: $(cat "$WORKSPACE/.lucidos/ports")"
    fi
    if grep -qx 'API_PORT=5173' "$WORKSPACE/.lucidos/ports"; then
        pass "the other keys survive the rewrite"
    else
        fail "API_PORT was lost: $(cat "$WORKSPACE/.lucidos/ports")"
    fi
    # $PROTO names the gateway's door in this mode: the banner, the window
    # and gateway_curl all build on it, and the gateway keeps its TLS.
    if [ "$proto" = "https" ]; then
        pass "the in-memory scheme still names the gateway"
    else
        fail "want PROTO to stay https for the gateway URLs, got '$proto'"
    fi
}

test_a_silent_engine_leaves_the_ports_file_alone() {
    echo "test: record_engine_scheme writes nothing when no engine answers"
    local rc
    WORKSPACE="$SANDBOX/ws-record-silent"
    record_scheme_with "7 7" >/dev/null
    rc=$?
    if grep -qx 'PROTO=https' "$WORKSPACE/.lucidos/ports"; then
        pass "no answer is no evidence, so the file is untouched"
    else
        fail "want PROTO=https untouched, got: $(cat "$WORKSPACE/.lucidos/ports")"
    fi
    if [ "$rc" -eq 0 ]; then
        pass "an absent engine still exits 0 under set -e"
    else
        fail "want exit 0 when nothing answers, got $rc"
    fi
}

# Both launchers reach the engine through ensure_workspace_engine_running, on
# its adopt branch and on its restart branch alike. The helper stubs every
# host-touching call it makes, so this pins the wiring without a gateway.
ensure_running_with() { # <engine-is-live: 1 or empty>
    WORKSPACE="$SANDBOX/ws-ensure-scheme"
    mkdir -p "$WORKSPACE/.lucidos"
    printf 'API_PORT=5173\nPROTO=https\n' >"$WORKSPACE/.lucidos/ports"
    printf '35 0' >"$SANDBOX/curl-queue"
    (
        set -e
        ENGINE_PORT=5173; PROTO=https; GATEWAY_PORT=5251; GATEWAY_WS_ID=ws
        ENGINE_ONLY=""; LIVE="$1"
        # shellcheck disable=SC2317 # all called by ensure_workspace_engine_running
        {
            engine_on_port_serves_workspace() { [ -n "$LIVE" ]; }
            gateway_curl() { return 0; }
            wait_for_workspace_health() { return 0; }
            curl() { queued_curl; }
        }
        ensure_workspace_engine_running >/dev/null
    )
}

test_both_gateway_branches_record_the_engine_scheme() {
    echo "test: ensure_workspace_engine_running records the engine scheme"
    ensure_running_with 1
    if grep -qx 'PROTO=http' "$WORKSPACE/.lucidos/ports"; then
        pass "adopting the running engine records its scheme"
    else
        fail "the adopt branch left: $(cat "$WORKSPACE/.lucidos/ports")"
    fi
    ensure_running_with ""
    if grep -qx 'PROTO=http' "$WORKSPACE/.lucidos/ports"; then
        pass "starting the engine records its scheme"
    else
        fail "the restart branch left: $(cat "$WORKSPACE/.lucidos/ports")"
    fi
}

test_a_gateway_launch_records_a_plain_http_engine
test_a_silent_engine_leaves_the_ports_file_alone
test_both_gateway_branches_record_the_engine_scheme

# ── workspace_engine_restart_is_needed: adopt, don't restart ───────────
#
# The bug this pins: a plain launch POSTed the gateway's restart control even
# with the engine already up, so every in-flight thread settled as an
# unattributed abort. A script carries no device, so those do not auto-resume.
# See docs/plans/2026-09-18-a-launch-never-tears-down-a-healthy-engine.md.
#
# Pure, so it runs with no gateway and no engine.

test_a_running_engine_is_adopted_not_restarted() {
    echo "test: a plain launch adopts the engine already serving"
    if workspace_engine_restart_is_needed "" 1; then
        fail "restarted an engine this launch had no reason to replace"
    else
        pass "the restart POST is skipped"
    fi
}

test_a_stopped_engine_is_started() {
    echo "test: a launch still starts an engine that is not running"
    # The POST is why it exists: a new workspace defaults to autostart off, so
    # the gateway's own boot spawns nothing. A `-b` lands here too, with its
    # engine already reaped by kill_stale_processes.
    if workspace_engine_restart_is_needed "" ""; then
        pass "the POST starts it"
    else
        fail "a launch with no engine running would start nothing"
    fi
}

test_the_apply_switch_always_respawns() {
    echo "test: --engine-only respawns even though the engine is healthy"
    # That is the whole job of the switch: move onto the freshly built binary.
    if workspace_engine_restart_is_needed 1 1; then
        pass "the Apply switch still restarts"
    else
        fail "a Switch would leave the old engine serving"
    fi
}

test_a_running_engine_is_adopted_not_restarted
test_a_stopped_engine_is_started
test_the_apply_switch_always_respawns

# ── wait_for_workspace_health: the probe carries the local credential ──
#
# The gateway gates every `/<slug>/api/v1/*` path, health included, so an
# uncredentialed probe gets 401 and `curl -f` fails on every tick. The launch
# then waited the full 90 seconds on a healthy engine. The stub answers like
# the gateway: 200 with the token, curl's `-f` exit 22 without it.
health_wait_with_token() { # <token-or-empty>
    rm -f "$HOME/.lucidos/local-token"
    if [ -n "$1" ]; then
        mkdir -p "$HOME/.lucidos"
        printf '%s' "$1" >"$HOME/.lucidos/local-token"
    fi
    : >"$SANDBOX/curl-calls"
    (
        PROTO=https; GATEWAY_PORT=5251; GATEWAY_WS_ID=dev
        # shellcheck disable=SC2317 # called by wait_for_workspace_health
        sleep() { :; }
        # shellcheck disable=SC2317 # called through gateway_curl
        curl() {
            printf 'CALL\n' >>"$SANDBOX/curl-calls"
            case " $* " in
                *" x-lucidos-local-token: secret "*) return 0 ;;
                *) return 22 ;;
            esac
        }
        wait_for_workspace_health
    )
}

test_the_health_wait_sends_the_local_token() {
    echo "test: wait_for_workspace_health authenticates to the gateway"
    local out
    out="$(health_wait_with_token secret)"
    if [[ "$out" == *"ready!"* ]] && [ "$(curl_calls)" = "1" ]; then
        pass "a healthy engine is ready on the first probe"
    else
        fail "want ready in one call, got $(curl_calls) calls: $out"
    fi
    rm -f "$HOME/.lucidos/local-token"
}

test_the_health_wait_sends_the_local_token

# ── LUCIDOS_EXTERNAL_PG_PORT: a native Postgres replaces the Docker cluster ──
#
# E2E GitHub mode sets it on macOS runners, which have no Docker. Both binaries
# are stubbed, so a call reaching the wrong one shows up in the call log.
# shellcheck disable=SC2317 # the stubs are called by the functions under test
test_shared_pg_psql_follows_the_external_port() {
    echo "test: shared_pg_psql talks to the external port when set, Docker otherwise"
    local external docker_out
    external="$(
        docker() { echo "docker $*"; }
        psql() { echo "psql $* PGPASSWORD=${PGPASSWORD:-}"; }
        LUCIDOS_EXTERNAL_PG_PORT=5999 shared_pg_psql mydb -tAc "SELECT 1"
    )"
    docker_out="$(
        docker() { echo "docker $*"; }
        psql() { echo "psql $*"; }
        unset LUCIDOS_EXTERNAL_PG_PORT
        shared_pg_psql mydb -tAc "SELECT 1"
    )"
    if [ "$external" = "psql -h 127.0.0.1 -p 5999 -U lucidos -d mydb -tAc SELECT 1 PGPASSWORD=$(shared_pg_password)" ]; then
        pass "the external port reaches psql over loopback TCP, with the minted secret in its env"
    else
        fail "external call was '$external'"
    fi
    if [ "$docker_out" = "docker exec lucidos-pg-shared psql -U lucidos -d mydb -tAc SELECT 1" ]; then
        pass "no external port keeps the Docker cluster"
    else
        fail "default call was '$docker_out'"
    fi
}

# shellcheck disable=SC2317 # the stubs are called by setup_postgres
test_setup_postgres_on_an_external_port_never_calls_docker() {
    echo "test: setup_postgres with LUCIDOS_EXTERNAL_PG_PORT skips every Docker step"
    : >"$SANDBOX/pg-calls"
    local port
    port="$(
        WORKSPACE="$SANDBOX/ws-external"; PG_PORT=5432
        docker() { echo "docker $*" >>"$SANDBOX/pg-calls"; return 1; }
        report_docker_daemon_if_down() { echo "daemon-probe" >>"$SANDBOX/pg-calls"; return 1; }
        psql() {
            echo "psql $*" >>"$SANDBOX/pg-calls"
            case "$*" in (*"FROM pg_database"*|*"SELECT 1"*) echo 1 ;; esac
        }
        sleep() { :; }
        LUCIDOS_EXTERNAL_PG_PORT=5999 setup_postgres >/dev/null 2>&1 || echo failed
        echo "$PG_PORT"
    )"
    if [ "$port" = "5999" ]; then
        pass "PG_PORT follows the external port"
    else
        fail "setup_postgres printed '$port'"
    fi
    if grep -q "^docker\|^daemon-probe" "$SANDBOX/pg-calls"; then
        fail "Docker was touched: $(grep "^docker\|^daemon-probe" "$SANDBOX/pg-calls" | head -1)"
    else
        pass "no Docker call and no daemon probe"
    fi
}

test_shared_pg_psql_follows_the_external_port
test_setup_postgres_on_an_external_port_never_calls_docker

echo ""
echo "Passed: $PASS  Failed: $FAIL"
[ $FAIL -eq 0 ]
