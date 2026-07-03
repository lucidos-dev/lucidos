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
test_seed_gateway_registry_removes_legacy_database_url
test_keeps_shared_build_watch_when_a_workspace_still_serves
test_kills_shared_build_watch_when_no_workspace_serves

echo ""
echo "Passed: $PASS  Failed: $FAIL"
[ $FAIL -eq 0 ]
