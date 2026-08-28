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

# ── fixture helpers ────────────────────────────────────────────────────
make_pkg_dir() {
    # Creates a package dir with a package.json and (optionally) a node_modules.
    # Install-detection is content-based now (see _deps_fingerprint), so what
    # matters is whether node_modules exists, not relative mtimes: fresh=1 →
    # node_modules present (no install needed, self-heals a stamp), fresh=0 →
    # node_modules absent (install needed).
    local dir="$1"
    local fresh="${2:-1}"   # 1 = node_modules present
    mkdir -p "$dir"
    echo '{"name":"x"}' > "$dir/package.json"
    if [ "$fresh" = "1" ]; then
        mkdir -p "$dir/node_modules"
        echo '{}' > "$dir/node_modules/.marker"
    fi
}

write_pid_for_workspace() {
    local ws_name="$1"
    local pid="$2"
    local dir="$HOME/workspaces/$ws_name/.lucidos"
    mkdir -p "$dir"
    echo "$pid" > "$dir/frontend.pid"
}

# ── Test 1: refuse npm install when a frontend in THIS project is running ──
test_refuses_install_when_same_project_frontend_running() {
    echo "test: refuses install when a frontend in this project is running"

    # `local PROJECT_DIR` shadows the global that ensure_npm_deps reads —
    # bash dynamic scoping makes the local visible to called functions.
    local PROJECT_DIR="$SANDBOX/proj-conflict"
    local pkg="$PROJECT_DIR"
    make_pkg_dir "$pkg" 0       # no node_modules → install IS needed

    # Fake Vite running with cwd inside PROJECT_DIR, mirroring the real layout
    # ($PROJECT_DIR/crates/lucidos-app). Backgrounded inline (NOT inside a
    # helper called via $()) so the child stays alive until this test ends.
    mkdir -p "$PROJECT_DIR/crates/lucidos-app"
    ( cd "$PROJECT_DIR/crates/lucidos-app" && exec sleep 30 ) &
    local fake_pid=$!
    disown "$fake_pid" 2>/dev/null || true   # suppress "Terminated" noise on cleanup
    write_pid_for_workspace "other-ws" "$fake_pid"

    # Override `npm` so we can detect if install is attempted.
    npm() { echo "NPM_INSTALL_RAN" >&2; return 0; }
    export -f npm

    local out
    out="$(ensure_npm_deps "$pkg" "test deps" 2>&1)"
    local rc=$?

    kill "$fake_pid" 2>/dev/null || true
    unset -f npm

    if [ $rc -eq 0 ]; then
        fail "expected non-zero exit, got 0"
    else
        pass "exited non-zero ($rc)"
    fi
    if echo "$out" | grep -q "NPM_INSTALL_RAN"; then
        fail "npm install ran despite running frontend"
    else
        pass "npm install did NOT run"
    fi
    if echo "$out" | grep -q "other-ws"; then
        pass "error message names the running workspace"
    else
        fail "error message did not mention 'other-ws': $out"
    fi
}

# ── Test 1a: ENGINE_ONLY restart skips (does not abort) when frontend running ──
# The engine-only restart (CC Apply) deliberately keeps this checkout's frontend
# alive. When the applied change bumped a dependency, installing would corrupt
# that Vite — but aborting would leave the workspace with no engine at all
# (build_sdk runs before the ENGINE_ONLY early-exit). So ENGINE_ONLY must skip
# the install, warn, and return 0 so the engine still comes up.
test_engine_only_skips_install_when_frontend_running() {
    echo "test: ENGINE_ONLY restart skips install (no abort) when frontend running"

    local PROJECT_DIR="$SANDBOX/proj-engine-only"
    local pkg="$PROJECT_DIR"
    make_pkg_dir "$pkg" 0       # no node_modules → install IS needed
    # Dynamic scoping makes this visible to ensure_npm_deps, like PROJECT_DIR.
    local ENGINE_ONLY=1

    mkdir -p "$PROJECT_DIR/crates/lucidos-app"
    ( cd "$PROJECT_DIR/crates/lucidos-app" && exec sleep 30 ) &
    local fake_pid=$!
    disown "$fake_pid" 2>/dev/null || true
    write_pid_for_workspace "dev" "$fake_pid"

    npm() { echo "NPM_INSTALL_RAN" >&2; return 0; }
    export -f npm

    local out rc
    out="$(ensure_npm_deps "$pkg" "workspace dependencies" 2>&1)"
    rc=$?

    kill "$fake_pid" 2>/dev/null || true
    unset -f npm

    if [ $rc -eq 0 ]; then
        pass "exited 0 (engine restart not aborted)"
    else
        fail "expected exit 0, got $rc; output: $out"
    fi
    if echo "$out" | grep -q "NPM_INSTALL_RAN"; then
        fail "npm install ran despite running frontend"
    else
        pass "npm install did NOT run (frontend preserved)"
    fi
    if echo "$out" | grep -q "WARNING"; then
        pass "warned that deps changed"
    else
        fail "expected a WARNING about deferred deps; output: $out"
    fi
    # Exact bullet-line match — a loose "dev" would also hit "web-dev.sh" in the
    # warning text, so assert the listed-workspace line specifically.
    if echo "$out" | grep -qx "  - dev"; then
        pass "warning names the running workspace"
    else
        fail "warning did not list '  - dev': $out"
    fi
}

# ── Test 1b: allow install when running frontend is in a DIFFERENT checkout ──
# Mirrors the CC-worktree case: a Vite running from main repo doesn't
# share node_modules with a worktree, so installing in the worktree is safe.
test_allows_install_when_other_checkout_frontend_running() {
    echo "test: allows install when running frontend is in a different checkout"

    local PROJECT_DIR="$SANDBOX/proj-worktree"
    local pkg="$PROJECT_DIR"
    make_pkg_dir "$pkg" 0       # no node_modules → install IS needed

    # Fake Vite cwd in a DIFFERENT physical project (simulating main repo
    # while we install in a worktree).
    local other_project="$SANDBOX/proj-main"
    mkdir -p "$other_project/crates/lucidos-app"
    ( cd "$other_project/crates/lucidos-app" && exec sleep 30 ) &
    local fake_pid=$!
    disown "$fake_pid" 2>/dev/null || true
    write_pid_for_workspace "main-ws" "$fake_pid"

    npm() { echo "NPM_INSTALL_RAN" >&2; return 0; }
    export -f npm

    local out
    out="$(ensure_npm_deps "$pkg" "test deps" 2>&1)"
    local rc=$?

    kill "$fake_pid" 2>/dev/null || true
    unset -f npm

    if [ $rc -ne 0 ]; then
        fail "expected exit 0, got $rc; output: $out"
    else
        pass "exited 0"
    fi
    if echo "$out" | grep -q "NPM_INSTALL_RAN"; then
        pass "npm install ran"
    else
        fail "npm install did not run; output: $out"
    fi
}

# ── Test 2: install proceeds when no frontends are running ──
test_installs_when_no_frontend_running() {
    echo "test: installs when no frontend running"

    rm -rf "$HOME/workspaces"

    local pkg="$SANDBOX/pkg-needs-install-2"
    make_pkg_dir "$pkg" 0

    npm() { echo "NPM_INSTALL_RAN" >&2; return 0; }
    export -f npm

    local out
    out="$(ensure_npm_deps "$pkg" "test deps" 2>&1)"
    local rc=$?
    unset -f npm

    if [ $rc -ne 0 ]; then
        fail "expected exit 0, got $rc; output: $out"
    else
        pass "exited 0"
    fi
    if echo "$out" | grep -q "NPM_INSTALL_RAN"; then
        pass "npm install ran"
    else
        fail "npm install did not run; output: $out"
    fi
}

# ── Test 3: stale pidfile (process gone) does not block install ──
test_stale_pidfile_does_not_block() {
    echo "test: stale pidfile does not block install"

    rm -rf "$HOME/workspaces"

    local pkg="$SANDBOX/pkg-needs-install-3"
    make_pkg_dir "$pkg" 0

    # PID 999999 is almost certainly not alive.
    write_pid_for_workspace "ghost-ws" 999999

    npm() { echo "NPM_INSTALL_RAN" >&2; return 0; }
    export -f npm

    local out
    out="$(ensure_npm_deps "$pkg" "test deps" 2>&1)"
    local rc=$?
    unset -f npm

    if [ $rc -ne 0 ]; then
        fail "stale pidfile blocked install; output: $out"
    else
        pass "stale pidfile did not block install"
    fi
}

# ── Test 4: no install needed → no check, no failure ──
test_no_install_needed_skips_check() {
    echo "test: no install needed skips frontend check"

    rm -rf "$HOME/workspaces"

    local pkg="$SANDBOX/pkg-fresh"
    make_pkg_dir "$pkg" 1       # node_modules present → no install needed

    # Even with a running frontend, we should not fail because no install runs.
    sleep 30 &
    local fake_pid=$!
    disown "$fake_pid" 2>/dev/null || true   # suppress "Terminated" noise on cleanup
    write_pid_for_workspace "other-ws" "$fake_pid"

    npm() { echo "NPM_INSTALL_RAN" >&2; return 0; }
    export -f npm

    local out
    out="$(ensure_npm_deps "$pkg" "test deps" 2>&1)"
    local rc=$?

    kill "$fake_pid" 2>/dev/null || true
    unset -f npm

    if [ $rc -ne 0 ]; then
        fail "no-install case errored; output: $out"
    else
        pass "no-install case exited 0"
    fi
    if echo "$out" | grep -q "NPM_INSTALL_RAN"; then
        fail "npm install ran when not needed"
    else
        pass "npm install did not run"
    fi
}

# ── Test 5: resolve_workspace_path expands bare names like web-dev.sh ──
test_resolves_bare_name_to_home_workspaces() {
    echo "test: resolve_workspace_path expands bare name"

    rm -rf "$HOME/workspaces"
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

    rm -rf "$HOME/workspaces"
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

    rm -rf "$HOME/workspaces"

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

    rm -rf "$HOME/workspaces"

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

    rm -rf "$HOME/workspaces"

    # Build the layout that npm workspaces produces:
    #   $root/package.json   { "workspaces": ["pkg-member"] }
    #   $root/node_modules/  (populated by `npm install` at root)
    #   $root/pkg-member/package.json   ( workspace member )
    #   $root/pkg-member/   ( NO node_modules — this is the fresh-worktree case )
    local root="$SANDBOX/wsroot-$$"
    mkdir -p "$root/node_modules" "$root/pkg-member"
    cat > "$root/package.json" <<'EOF'
{"private":true,"workspaces":["pkg-member"]}
EOF
    echo '{"name":"pkg-member"}' > "$root/pkg-member/package.json"

    # Pin a running frontend for another workspace — if the function decides
    # install IS needed, this would make it exit 1.
    sleep 30 &
    local fake_pid=$!
    disown "$fake_pid" 2>/dev/null || true
    write_pid_for_workspace "other-ws" "$fake_pid"

    npm() { echo "NPM_INSTALL_RAN" >&2; return 0; }
    export -f npm

    local out rc
    out="$(ensure_npm_deps "$root/pkg-member" "frontend deps" 2>&1)"
    rc=$?

    kill "$fake_pid" 2>/dev/null || true
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

    rm -rf "$HOME/workspaces"

    local root="$SANDBOX/wsroot2-$$"
    mkdir -p "$root/node_modules" "$root/pkg-member"
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

    rm -rf "$HOME/workspaces"

    local PROJECT_DIR="$SANDBOX/proj-noop"
    local pkg="$PROJECT_DIR"
    mkdir -p "$pkg/node_modules"
    echo '{"name":"x"}' > "$pkg/package.json"

    npm() { echo "NPM_INSTALL_RAN" >&2; return 0; }
    export -f npm

    # Establish the stamp from the original content.
    ensure_npm_deps "$pkg" "test deps" >/dev/null 2>&1

    # Simulate a running frontend in THIS project — would force exit 1 if the
    # function wrongly decided an install was needed.
    mkdir -p "$PROJECT_DIR/crates/lucidos-app"
    ( cd "$PROJECT_DIR/crates/lucidos-app" && exec sleep 30 ) &
    local fake_pid=$!
    disown "$fake_pid" 2>/dev/null || true
    write_pid_for_workspace "live-ws" "$fake_pid"

    # No-op rewrite: identical bytes, fresh mtime (what `git checkout` does).
    echo '{"name":"x"}' > "$pkg/package.json"
    touch "$pkg/package.json"

    local out rc
    out="$(ensure_npm_deps "$pkg" "test deps" 2>&1)"
    rc=$?

    kill "$fake_pid" 2>/dev/null || true
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

    rm -rf "$HOME/workspaces"

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

# ── Shared build-watch teardown (checkout-level singleton, ref-counted) ──
# teardown_shared_build_watch_if_idle must keep the shared `vite build --watch`
# alive while ANY workspace of the checkout is still serving the frontend, and
# only kill it when the last one is gone. Mirrors how cleanup_processes / stop.sh
# tear it down. The ref-count reuses running_frontend_workspaces_in_project, so a
# fake Vite preview with cwd inside the project stands in for "still serving".
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

    # scripts/lib/e2e.sh calls start_engine directly and never starts a gateway,
    # so it only ever reaches this scope. Regressing it breaks e2e's frontend.
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

test_keeps_shared_build_watch_when_a_workspace_still_serves() {
    echo "test: shared build-watch survives while another workspace still serves"

    local PROJECT_DIR="$SANDBOX/proj-bw-keep"
    mkdir -p "$PROJECT_DIR/crates/lucidos-app/.build-watch"

    # Fake shared build-watch process + its checkout-level pidfile.
    ( exec sleep 30 ) & local bw_pid=$!
    disown "$bw_pid" 2>/dev/null || true
    echo "$bw_pid" > "$(build_watch_pidfile)"

    # A sibling workspace's Vite preview, cwd inside the project → "still serving".
    ( cd "$PROJECT_DIR/crates/lucidos-app" && exec sleep 30 ) & local fe_pid=$!
    disown "$fe_pid" 2>/dev/null || true
    write_pid_for_workspace "sibling-ws" "$fe_pid"

    teardown_shared_build_watch_if_idle >/dev/null 2>&1

    if kill -0 "$bw_pid" 2>/dev/null; then
        pass "build-watch left running while a workspace still serves"
    else
        fail "build-watch was killed despite a serving workspace"
    fi
    if [ -f "$(build_watch_pidfile)" ]; then
        pass "build-watch pidfile preserved"
    else
        fail "build-watch pidfile was removed"
    fi

    kill "$bw_pid" "$fe_pid" 2>/dev/null || true
}

test_kills_shared_build_watch_when_no_workspace_serves() {
    echo "test: shared build-watch torn down when the last workspace stops"

    local PROJECT_DIR="$SANDBOX/proj-bw-kill"
    mkdir -p "$PROJECT_DIR/crates/lucidos-app/.build-watch"
    rm -rf "$HOME/workspaces"   # no workspace serving this checkout

    ( exec sleep 30 ) & local bw_pid=$!
    disown "$bw_pid" 2>/dev/null || true
    echo "$bw_pid" > "$(build_watch_pidfile)"

    teardown_shared_build_watch_if_idle >/dev/null 2>&1

    # Give the kill a beat to land.
    local waited=0
    while kill -0 "$bw_pid" 2>/dev/null && [ "$waited" -lt 20 ]; do sleep 0.1; waited=$((waited+1)); done

    if kill -0 "$bw_pid" 2>/dev/null; then
        fail "build-watch still running with no serving workspace"
        kill "$bw_pid" 2>/dev/null || true
    else
        pass "build-watch killed when no workspace serves"
    fi
    if [ -f "$(build_watch_pidfile)" ]; then
        fail "build-watch pidfile not removed"
    else
        pass "build-watch pidfile removed"
    fi
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
    echo "test: swap_ports writes workspace-specific shared database URL"

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
    if grep -qx "DATABASE_URL=postgres://lucidos:lucidos@localhost:5544/lucidos_dev-project" "$ports_file"; then
        pass "ports file records shared DATABASE_URL"
    else
        fail "ports file missing shared DATABASE_URL: $(cat "$ports_file")"
    fi
    if [ "$DATABASE_URL" = "postgres://lucidos:lucidos@localhost:5544/lucidos_dev-project" ]; then
        pass "DATABASE_URL exported for direct engine mode"
    else
        fail "unexpected DATABASE_URL export: ${DATABASE_URL:-<unset>}"
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

test_seed_gateway_registry_removes_legacy_database_url() {
    echo "test: seed_gateway_registry removes legacy database_url"

    local PROJECT_DIR="$SANDBOX/proj-registry"
    mkdir -p "$PROJECT_DIR" "$HOME/workspaces/dev/.lucidos" "$(gateway_data_dir)/config"
    WORKSPACE="$HOME/workspaces/dev"
    ENGINE_PORT=5173
    GATEWAY_PORT=5251

    cat > "$(gateway_data_dir)/config/workspaces.json" <<'JSON'
{
  "workspaces": [
    {
      "id": "dev",
      "name": "Picker Name",
      "dir": "/old/dev",
      "port": 5000,
      "database_url": "postgres://lucidos:lucidos@localhost:5439/lucidos",
      "autostart": true
    }
  ]
}
JSON

    seed_gateway_registry >/dev/null

    local out
    out="$(python3 - "$(gateway_data_dir)/config/workspaces.json" <<'PY'
import json, sys
w = json.load(open(sys.argv[1]))["workspaces"][0]
print(w.get("name"))
print(w.get("dir"))
print(w.get("port"))
print(w.get("autostart"))
print("database_url" in w)
PY
)"
    if echo "$out" | grep -qx "Picker Name"; then
        pass "display name preserved"
    else
        fail "display name not preserved: $out"
    fi
    if echo "$out" | grep -qx "$WORKSPACE"; then
        pass "workspace dir refreshed"
    else
        fail "workspace dir not refreshed: $out"
    fi
    if echo "$out" | grep -qx "5173"; then
        pass "engine port refreshed"
    else
        fail "engine port not refreshed: $out"
    fi
    if echo "$out" | grep -qx "True"; then
        pass "autostart preserved"
    else
        fail "autostart not preserved: $out"
    fi
    if echo "$out" | grep -qx "False"; then
        pass "legacy database_url removed"
    else
        fail "database_url still present: $out"
    fi
}

test_refuses_install_when_same_project_frontend_running
test_engine_only_skips_install_when_frontend_running
test_allows_install_when_other_checkout_frontend_running
test_installs_when_no_frontend_running
test_stale_pidfile_does_not_block
test_no_install_needed_skips_check
test_workspace_member_with_root_deps_skips_install
test_workspace_member_install_when_package_json_bumped
test_noop_rewrite_does_not_trigger_install
test_workspace_member_install_when_root_node_modules_missing
test_resolves_bare_name_to_home_workspaces
test_resolves_absolute_path_unchanged
test_errors_on_missing_workspace
test_does_not_create_directories
test_shared_pg_sql_quoting
test_legacy_pg_volume_layout_detects_parent_pgdata
test_legacy_pg_volume_layout_detects_root_pgdata
test_swap_ports_writes_shared_database_url
test_swap_ports_keeps_the_proto_detect_tls_wrote
test_seed_gateway_registry_removes_legacy_database_url

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

    # A coalescing Apply SIGKILLs the whole build process group
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

    # A binary from a different commit — the case the retry exists for.
    make_build_id_stub "$stub" "0123456789abc"
    if [ "$(published_build_state "$stub")" = "stale" ]; then
        pass "a different commit reads as stale"
    else
        fail "a different commit was not detected as stale"
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

# ── pid_is_live ────────────────────────────────────────────────────────
# The zombie case is the whole point: `kill -0` succeeds for a defunct
# process, which is what let a dead engine report as running.
test_pid_is_live_rejects_a_zombie() {
    echo "pid_is_live"

    # A live process we own: `sleep` in the background, killed at the end.
    sleep 30 &
    local live_pid=$!
    if pid_is_live "$live_pid"; then
        pass "a running process is live"
    else
        fail "a running process was reported dead (pid $live_pid)"
    fi

    # A real zombie: fork a child that exits immediately from a parent that
    # never reaps it, and hold that parent open while we probe. python3 is
    # already required by this repo's scripts.
    local zombie_dir="$SANDBOX/zombie"
    mkdir -p "$zombie_dir"
    python3 -c "
import os, sys, time
pid = os.fork()
if pid == 0:
    os._exit(0)
sys.stdout.write(str(pid))
sys.stdout.flush()
time.sleep(10)
" > "$zombie_dir/pid" &
    local holder_pid=$!
    local zombie_pid="" waited=0
    while [ -z "$zombie_pid" ] && [ "$waited" -lt 50 ]; do
        sleep 0.1
        zombie_pid="$(cat "$zombie_dir/pid" 2>/dev/null || true)"
        waited=$((waited + 1))
    done

    if [ -z "$zombie_pid" ]; then
        fail "could not fork a zombie to test against"
    else
        # Guard the premise: this test is only meaningful while the pid really
        # is a zombie that `kill -0` still accepts.
        local state
        state="$(ps -o state= -p "$zombie_pid" 2>/dev/null | tr -d '[:space:]')"
        if [ "${state:0:1}" != "Z" ]; then
            fail "fixture pid $zombie_pid is not a zombie (state '$state')"
        elif ! kill -0 "$zombie_pid" 2>/dev/null; then
            fail "fixture zombie $zombie_pid is not kill -0 visible, premise gone"
        elif pid_is_live "$zombie_pid"; then
            fail "a zombie was reported live (pid $zombie_pid)"
        else
            pass "a zombie is not live (kill -0 says otherwise)"
        fi
    fi

    kill -KILL "$holder_pid" 2>/dev/null || true
    kill -KILL "$live_pid" 2>/dev/null || true
    wait "$holder_pid" 2>/dev/null || true
    wait "$live_pid" 2>/dev/null || true

    # A pid that has gone entirely (reaped, or never existed).
    if pid_is_live "$live_pid"; then
        fail "a reaped pid was reported live (pid $live_pid)"
    else
        pass "a gone pid is not live"
    fi

    # Garbage pidfile contents must not be live, and must not error.
    if pid_is_live ""; then
        fail "an empty pid was reported live"
    else
        pass "an empty pid is not live"
    fi
    if pid_is_live "not-a-pid"; then
        fail "a non-numeric pid was reported live"
    else
        pass "a non-numeric pid is not live"
    fi
}

test_pid_is_live_rejects_a_zombie
test_keeps_shared_build_watch_when_a_workspace_still_serves
test_kills_shared_build_watch_when_no_workspace_serves
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

echo ""
echo "Passed: $PASS  Failed: $FAIL"
[ $FAIL -eq 0 ]
