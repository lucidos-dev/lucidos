#!/bin/bash
# Tests for scripts/deps-state.sh, the answers the build-watch uses to decide
# whether to install.
#
# Hermetic: every case reads a throwaway tree or the checkout's own manifests,
# and nothing here starts or signals a process.
#
# Covered: the fingerprint tracks a lock edit and matches the library's own
# answer, and the stamp path sits inside the install root.
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

test_fingerprint_ignores_a_nested_checkout() {
    echo "test: a nested git checkout's package.json does not move the fingerprint"
    # A coding agent's worktree under .claude/worktrees/ is another tree.
    local root before after
    root="$(mktemp -d)"
    git -C "$root" init -q
    printf '{"name":"t","workspaces":["a"]}' > "$root/package.json"
    mkdir -p "$root/a"
    printf '{"name":"a"}' > "$root/a/package.json"
    printf '{"lockfileVersion":3}' > "$root/package-lock.json"
    before="$(
        PROJECT_DIR="$PROJECT_ROOT" bash -c \
        'source "$PROJECT_DIR/scripts/lib/workspace.sh"; _deps_fingerprint "$1"' _ "$root"
    )"
    mkdir -p "$root/nested"
    git -C "$root/nested" init -q
    printf '{"name":"nested","dependencies":{"x":"1"}}' > "$root/nested/package.json"
    after="$(
        PROJECT_DIR="$PROJECT_ROOT" bash -c \
        'source "$PROJECT_DIR/scripts/lib/workspace.sh"; _deps_fingerprint "$1"' _ "$root"
    )"
    printf '{"name":"a","dependencies":{"y":"1"}}' > "$root/a/package.json"
    local member_edit
    member_edit="$(
        PROJECT_DIR="$PROJECT_ROOT" bash -c \
        'source "$PROJECT_DIR/scripts/lib/workspace.sh"; _deps_fingerprint "$1"' _ "$root"
    )"
    rm -rf "$root"
    if [ "$before" = "$after" ]; then
        pass "nested checkout ignored"
    else
        fail "a nested checkout moved the fingerprint: $before -> $after"
    fi
    if [ "$after" != "$member_edit" ]; then
        pass "an untracked member edit still moves it"
    else
        fail "a member package.json edit did not move the fingerprint"
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

test_fingerprint_is_the_librarys_own_answer
test_fingerprint_moves_when_the_lock_moves
test_fingerprint_ignores_a_nested_checkout
test_stamp_path_lives_in_the_install_root

echo ""
echo "passed $PASS, failed $FAIL"
[ "$FAIL" -eq 0 ]
