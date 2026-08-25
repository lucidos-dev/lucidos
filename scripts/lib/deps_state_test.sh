#!/bin/bash
# Tests for scripts/deps-state.sh, the answers the build-watch uses to decide
# whether to install and whether that is safe.
#
# Hermetic, and deliberately so. `running_frontend_workspaces_in_project` globs
# `$HOME/workspaces/*/.lucidos/frontend.pid`, so every case runs under a
# throwaway HOME with markers this file wrote. The only live pids are `sleep`
# children this file spawned and kills; nothing here signals a pid it did not
# create. That rule is not decoration: the port-allocator suite killed the
# user's engine twice by stubbing one probe and not another (ADR 0025).
#
# Covered: the fingerprint tracks a lock edit and matches the library's own
# answer; the stamp path sits inside the install root; and the dev-server probe
# tells the shared build-watch apart from a real Vite server, which is the
# distinction the whole self-heal rests on.
#
# Run: ./scripts/lib/deps_state_test.sh
set -u

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_ROOT="$(cd "$SCRIPT_DIR/../.." && pwd)"
CLI="$PROJECT_ROOT/scripts/deps-state.sh"

PASS=0
FAIL=0
pass() { echo "  ok:   $*"; PASS=$((PASS + 1)); }
fail() { echo "  FAIL: $*"; FAIL=$((FAIL + 1)); }

REAL_HOME="$HOME"
TMP_HOME=""
# Spawned pids go in a FILE, not a variable: `spawn_in_project` is called from a
# command substitution, so a variable it appended to would die with the
# subshell and cleanup would leak every `sleep` this file started.
SPAWNED_FILE=""

cleanup() {
    local pid
    if [ -n "$SPAWNED_FILE" ] && [ -f "$SPAWNED_FILE" ]; then
        while IFS= read -r pid; do
            # Only pids this file spawned, and only ones still alive.
            [ -n "$pid" ] && kill -0 "$pid" 2>/dev/null && kill "$pid" 2>/dev/null
        done < "$SPAWNED_FILE"
    fi
    [ -n "$TMP_HOME" ] && rm -rf "$TMP_HOME"
    [ -n "$SPAWNED_FILE" ] && rm -f "$SPAWNED_FILE"
    HOME="$REAL_HOME"
}
trap cleanup EXIT

TMP_HOME="$(mktemp -d)"
SPAWNED_FILE="$(mktemp)"

# ── the fingerprint ─────────────────────────────────────────────────────────

test_fingerprint_is_the_librarys_own_answer() {
    echo "test: the wrapper prints what _deps_fingerprint prints"
    # One definition, two callers. A second cksum written in JavaScript is the
    # thing this script exists to prevent, so the wrapper must not diverge
    # from the function even by a trailing byte.
    local direct wrapper
    direct="$(
        PROJECT_DIR="$PROJECT_ROOT" FRONTEND_DIR="$PROJECT_ROOT/crates/lucidos-app" \
        bash -c 'source "$PROJECT_DIR/scripts/lib/workspace.sh"; _deps_fingerprint "$(_resolve_npm_install_root "$FRONTEND_DIR")"'
    )"
    wrapper="$("$CLI" fingerprint)"
    if [ "$direct" = "$wrapper" ]; then
        pass "wrapper and library agree ($wrapper)"
    else
        fail "wrapper '$wrapper' != library '$direct'"
    fi
}

test_fingerprint_moves_when_the_lock_moves() {
    echo "test: editing the lockfile changes the fingerprint"
    # The whole premise of the stamp comparison. A fingerprint that ignored the
    # lock would let a new dependency through and wedge the build silently.
    local root before after
    root="$(mktemp -d)"
    printf '{"name":"t","workspaces":["a"]}' > "$root/package.json"
    printf '{"lockfileVersion":3}' > "$root/package-lock.json"
    before="$(
        PROJECT_DIR="$PROJECT_ROOT" bash -c \
        'source "$PROJECT_DIR/scripts/lib/workspace.sh"; _deps_fingerprint "$1"' _ "$root"
    )"
    printf '{"lockfileVersion":3,"packages":{"node_modules/x":{}}}' > "$root/package-lock.json"
    after="$(
        PROJECT_DIR="$PROJECT_ROOT" bash -c \
        'source "$PROJECT_DIR/scripts/lib/workspace.sh"; _deps_fingerprint "$1"' _ "$root"
    )"
    rm -rf "$root"
    if [ "$before" != "$after" ]; then
        pass "fingerprint moved"
    else
        fail "fingerprint unchanged after a lock edit: $before"
    fi
}

test_stamp_path_lives_in_the_install_root() {
    echo "test: the stamp sits under the install root's node_modules"
    local root stamp
    root="$("$CLI" install-root)"
    stamp="$("$CLI" stamp-path)"
    if [ "$stamp" = "$root/node_modules/.lucidos-deps-stamp" ]; then
        pass "$stamp"
    else
        fail "stamp '$stamp' is not under install root '$root'"
    fi
}

# ── the dev-server probe ────────────────────────────────────────────────────

# Write a frontend marker for a fake workspace under the throwaway HOME.
seed_marker() {
    local ws="$1" pid="$2"
    mkdir -p "$TMP_HOME/workspaces/$ws/.lucidos"
    echo "$pid" > "$TMP_HOME/workspaces/$ws/.lucidos/frontend.pid"
}

# A live process whose cwd is inside the project, which is what the probe
# checks. Spawned by us, killed by us.
#
# It waits until `lsof` can actually report the cwd. The probe reads the cwd
# through lsof, and a freshly forked child is not yet visible to it, so
# returning early makes the conflict test flaky in the direction that passes for
# the wrong reason.
spawn_in_project() {
    # stdout and stderr detached from the caller. This runs inside a command
    # substitution, and a background child holding that pipe open would make
    # the substitution block until the `sleep` finished.
    ( cd "$PROJECT_ROOT" && exec sleep 60 ) >/dev/null 2>&1 &
    local pid=$!
    echo "$pid" >> "$SPAWNED_FILE"
    local waited=0
    while [ "$waited" -lt 50 ]; do
        if lsof -p "$pid" -a -d cwd -Fn 2>/dev/null | grep -q '^n'; then break; fi
        sleep 0.1
        waited=$((waited + 1))
    done
    echo "$pid"
}

probe() (
    HOME="$TMP_HOME"
    export HOME
    "$CLI" dev-server-running
)

test_no_markers_means_safe_to_install() {
    echo "test: nothing running means no conflict"
    rm -rf "${TMP_HOME:?}/workspaces"
    if probe; then
        fail "reported a dev server with no markers at all"
    else
        pass "no conflict"
    fi
}

test_the_build_watch_is_not_a_dev_server() {
    echo "test: a marker holding the build-watch pid is not a conflict"
    # THE case this script exists for. `start_frontend_built` records the shared
    # build-watch pid as every workspace's frontend.pid, so a probe that took
    # the marker at face value would refuse every install and the build would
    # never heal.
    rm -rf "${TMP_HOME:?}/workspaces"
    local pid bw_dir
    pid="$(spawn_in_project)"
    bw_dir="$PROJECT_ROOT/crates/lucidos-app/.build-watch"
    mkdir -p "$bw_dir"
    local saved=""
    [ -f "$bw_dir/pid" ] && saved="$(cat "$bw_dir/pid")"
    echo "$pid" > "$bw_dir/pid"
    seed_marker "alpha" "$pid"

    if probe; then
        fail "the shared build-watch was mistaken for a dev server"
    else
        pass "build-watch excluded"
    fi

    if [ -n "$saved" ]; then echo "$saved" > "$bw_dir/pid"; else rm -f "$bw_dir/pid"; fi
}

test_a_real_dev_server_is_a_conflict() {
    echo "test: a marker holding some OTHER live pid is a conflict"
    rm -rf "${TMP_HOME:?}/workspaces"
    local vite_pid bw_pid bw_dir
    vite_pid="$(spawn_in_project)"
    bw_pid="$(spawn_in_project)"
    bw_dir="$PROJECT_ROOT/crates/lucidos-app/.build-watch"
    mkdir -p "$bw_dir"
    local saved=""
    [ -f "$bw_dir/pid" ] && saved="$(cat "$bw_dir/pid")"
    echo "$bw_pid" > "$bw_dir/pid"
    seed_marker "beta" "$vite_pid"

    if probe; then
        pass "conflict reported"
    else
        fail "a live Vite server was not reported"
    fi

    if [ -n "$saved" ]; then echo "$saved" > "$bw_dir/pid"; else rm -f "$bw_dir/pid"; fi
}

test_a_dead_pid_is_not_a_conflict() {
    echo "test: a stale marker for a dead pid is not a conflict"
    rm -rf "${TMP_HOME:?}/workspaces"
    local pid
    pid="$(spawn_in_project)"
    kill "$pid" 2>/dev/null
    wait "$pid" 2>/dev/null
    seed_marker "gamma" "$pid"
    if probe; then
        fail "a dead pid was reported as a running dev server"
    else
        pass "stale marker ignored"
    fi
}

echo "deps-state.sh:"
test_fingerprint_is_the_librarys_own_answer
test_fingerprint_moves_when_the_lock_moves
test_stamp_path_lives_in_the_install_root
test_no_markers_means_safe_to_install
test_the_build_watch_is_not_a_dev_server
test_a_real_dev_server_is_a_conflict
test_a_dead_pid_is_not_a_conflict

echo ""
echo "passed $PASS, failed $FAIL"
[ "$FAIL" -eq 0 ]
