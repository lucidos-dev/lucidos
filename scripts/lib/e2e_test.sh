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
    git -C "$canon" worktree add -q -b claude-code/e2e-fake \
        "$E2E_WORKSPACE/.lucidos/worktrees/e2e-cc" main >/dev/null 2>&1
    # Real CC session worktree: lives in a different workspace, on an
    # ancestor-of-main branch (just started, nothing committed yet).
    git -C "$canon" worktree add -q -b claude-code/real-live \
        "$dev/.lucidos/worktrees/real-cc" main >/dev/null 2>&1
    # Real CC session branch with NO worktree, also ancestor-of-main — exactly
    # what the old `for-each-ref … merge-base --is-ancestor … branch -D` deleted.
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
    if git -C "$canon" show-ref --verify --quiet refs/heads/claude-code/e2e-fake; then
        fail "e2e branch not deleted"
    else
        pass "e2e branch deleted"
    fi

    case "$wts" in
        *"$dev/.lucidos/worktrees/real-cc"*) pass "real session worktree preserved" ;;
        *) fail "real session worktree was removed" ;;
    esac
    if git -C "$canon" show-ref --verify --quiet refs/heads/claude-code/real-live; then
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

test_prune_removes_empty_dir
test_prune_removes_dir_with_dangling_gitdir
test_prune_keeps_live_worktree
test_prune_keeps_dir_without_git_pointer
test_prune_handles_missing_root
test_cleanup_spares_real_cc_sessions

echo ""
echo "Passed: $PASS  Failed: $FAIL"
[ $FAIL -eq 0 ]
