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
    rm -rf "$wt_root"/*
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
    rm -rf "$wt_root"/*
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
    rm -rf "$wt_root"/* "$SANDBOX/repo"
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
    rm -rf "$wt_root"/*
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

test_prune_removes_empty_dir
test_prune_removes_dir_with_dangling_gitdir
test_prune_keeps_live_worktree
test_prune_keeps_dir_without_git_pointer
test_prune_handles_missing_root

echo ""
echo "Passed: $PASS  Failed: $FAIL"
[ $FAIL -eq 0 ]
