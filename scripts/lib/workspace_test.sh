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

PASS=0
FAIL=0
fail() { echo "  FAIL: $*"; FAIL=$((FAIL+1)); }
pass() { echo "  ok:   $*"; PASS=$((PASS+1)); }

# Source the lib in a way that doesn't require global setup.
# shellcheck source=workspace.sh
source "$SCRIPT_DIR/workspace.sh"

# ── fixture helpers ────────────────────────────────────────────────────
make_pkg_dir() {
    # Creates a package dir with a package.json and (optionally) a node_modules
    # whose dir mtime is newer than package.json (so install is NOT needed).
    local dir="$1"
    local fresh="${2:-1}"   # 1 = node_modules newer than package.json
    mkdir -p "$dir"
    echo '{"name":"x"}' > "$dir/package.json"
    if [ "$fresh" = "1" ]; then
        mkdir -p "$dir/node_modules"
        # Adding a file bumps the directory's mtime — same effect npm install has.
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

# ── Test 1: refuse npm install when another workspace's frontend is running ──
test_refuses_install_when_other_frontend_running() {
    echo "test: refuses install when other frontend running"

    local pkg="$SANDBOX/pkg-needs-install"
    make_pkg_dir "$pkg" 0       # no node_modules → install IS needed

    sleep 30 &
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
    make_pkg_dir "$pkg" 1       # lock newer than package.json → no install needed

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

test_refuses_install_when_other_frontend_running
test_installs_when_no_frontend_running
test_stale_pidfile_does_not_block
test_no_install_needed_skips_check
test_resolves_bare_name_to_home_workspaces
test_resolves_absolute_path_unchanged
test_errors_on_missing_workspace
test_does_not_create_directories

echo ""
echo "Passed: $PASS  Failed: $FAIL"
[ $FAIL -eq 0 ]
