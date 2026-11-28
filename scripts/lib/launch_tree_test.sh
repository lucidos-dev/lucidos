#!/bin/bash
# Tests for scripts/lib/launch_tree.sh (ADR 0668).
#
# Hermetic. Real git runs in a mktemp sandbox with its own HOME and no system
# config. The only processes signalled are ones this file spawned. Waits are
# FIFO rendezvous, never sleeps.
#
# Run: ./scripts/lib/launch_tree_test.sh
set -u

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
SANDBOX="$(cd "$(mktemp -d)" && pwd -P)"
trap 'rm -rf "$SANDBOX"' EXIT

export HOME="$SANDBOX/home"
export GIT_CONFIG_NOSYSTEM=1
export GIT_AUTHOR_NAME=Test GIT_AUTHOR_EMAIL=test@example.com
export GIT_COMMITTER_NAME=Test GIT_COMMITTER_EMAIL=test@example.com
mkdir -p "$HOME"

PASS=0
FAIL=0
pass() { echo "  ok:   $*"; PASS=$((PASS + 1)); }
fail() { echo "  FAIL: $*"; FAIL=$((FAIL + 1)); }
assert_eq() { # <expected> <actual> <label>
    if [ "$1" = "$2" ]; then pass "$3"; else fail "$3 (expected '$1', got '$2')"; fi
}

# shellcheck source=scripts/lib/launch_tree.sh
source "$SCRIPT_DIR/launch_tree.sh"
# shellcheck source=scripts/lib/flock_test_support.sh
source "$SCRIPT_DIR/flock_test_support.sh"

free_for_others() { # <lock>
    perl -MFcntl=:flock -e 'open(my $f, ">>", $ARGV[0]) or exit 2; flock($f, LOCK_EX | LOCK_NB) ? exit 0 : exit 1' "$1"
}

file_mtime() { # <path>
    stat -c %Y "$1" 2>/dev/null || stat -f %m "$1"
}

_REPO_SEQ=0
# A fresh primary checkout with one commit. Sets REPO and TREE.
new_repo() {
    _REPO_SEQ=$((_REPO_SEQ + 1))
    REPO="$SANDBOX/repo-$_REPO_SEQ"
    mkdir -p "$REPO/src"
    git -C "$REPO" init -q
    printf 'target/\nignored.txt\n' > "$REPO/.gitignore"
    echo one > "$REPO/src/a.txt"
    echo keep > "$REPO/src/keep.txt"
    echo gone > "$REPO/src/gone.txt"
    git -C "$REPO" add -A
    git -C "$REPO" commit -qm first
    TREE="$(launch_tree_dir "$REPO")"
}

test_wanted_only_in_a_primary_tree() {
    echo "test: launch_tree_wanted holds for a primary tree, never a linked worktree"
    new_repo
    if launch_tree_wanted "$REPO"; then pass "primary tree"; else fail "primary tree refused"; fi
    git -C "$REPO" worktree add -q --detach "$SANDBOX/linked-$_REPO_SEQ"
    if launch_tree_wanted "$SANDBOX/linked-$_REPO_SEQ"; then
        fail "linked worktree wanted a launch tree"
    else
        pass "linked worktree builds live"
    fi
    mkdir -p "$SANDBOX/plain"
    if launch_tree_wanted "$SANDBOX/plain"; then fail "non-git dir wanted one"; else pass "no git, no launch tree"; fi
}

test_switch_turns_the_launch_tree_off() {
    echo "test: LUCIDOS_LAUNCH_TREE=off builds a primary tree live, and no other value does"
    new_repo
    if LUCIDOS_LAUNCH_TREE=off launch_tree_wanted "$REPO"; then
        fail "off still wanted a launch tree"
    else
        pass "off builds live"
    fi
    if LUCIDOS_LAUNCH_TREE=no launch_tree_wanted "$REPO"; then
        pass "an unknown value keeps the launch tree"
    else
        fail "an unknown value turned the launch tree off"
    fi
}

test_snapshot_is_the_working_tree() {
    echo "test: the snapshot carries uncommitted and untracked files, not ignored ones"
    new_repo
    echo two > "$REPO/src/a.txt"
    echo new > "$REPO/src/untracked.txt"
    echo secret > "$REPO/ignored.txt"
    rm "$REPO/src/gone.txt"
    git -C "$REPO" add src/a.txt
    echo three > "$REPO/src/a.txt"
    local status_before head
    status_before="$(git -C "$REPO" status --porcelain)"
    launch_tree_ensure "$REPO" "$TREE" || fail "ensure failed"
    head="$(launch_tree_snapshot "$REPO" "$TREE")" || fail "snapshot failed"
    assert_eq "$(git -C "$REPO" rev-parse HEAD)" "$head" "prints the commit it came from"
    assert_eq "$head" "$(git -C "$TREE" rev-parse HEAD)" "tree HEAD is that commit"
    assert_eq three "$(cat "$TREE/src/a.txt")" "unstaged edit wins over the staged one"
    assert_eq new "$(cat "$TREE/src/untracked.txt" 2>/dev/null)" "untracked file carried"
    if [ -e "$TREE/ignored.txt" ]; then fail "ignored file carried"; else pass "ignored file left out"; fi
    if [ -e "$TREE/src/gone.txt" ]; then fail "deleted file still present"; else pass "deleted file absent"; fi
    assert_eq "$status_before" "$(git -C "$REPO" status --porcelain)" "the live index is untouched"
    if git -C "$TREE" symbolic-ref -q HEAD >/dev/null; then fail "tree HEAD is on a branch"; else pass "tree HEAD is detached"; fi
}

test_live_edits_after_the_snapshot_stay_out() {
    echo "test: a live edit or commit after the snapshot never reaches the tree"
    new_repo
    launch_tree_ensure "$REPO" "$TREE" || fail "ensure failed"
    local head
    head="$(launch_tree_snapshot "$REPO" "$TREE")"
    echo moved > "$REPO/src/a.txt"
    git -C "$REPO" commit -qam second
    assert_eq one "$(cat "$TREE/src/a.txt")" "tree keeps the snapshot content"
    assert_eq "$head" "$(git -C "$TREE" rev-parse HEAD)" "tree HEAD stays put"
    head="$(launch_tree_snapshot "$REPO" "$TREE")"
    assert_eq "$(git -C "$REPO" rev-parse HEAD)" "$head" "the next snapshot follows HEAD"
    assert_eq moved "$(cat "$TREE/src/a.txt")" "and its content"
}

test_unchanged_files_keep_their_mtime() {
    echo "test: re-snapshotting rewrites only changed files"
    new_repo
    launch_tree_ensure "$REPO" "$TREE" || fail "ensure failed"
    launch_tree_snapshot "$REPO" "$TREE" >/dev/null
    touch -t 202001010000 "$TREE/src/a.txt" "$TREE/src/keep.txt"
    # Refresh the tree index so the old mtimes read as clean, as after a build.
    git -C "$TREE" update-index -q --refresh
    local keep_before a_before
    keep_before="$(file_mtime "$TREE/src/keep.txt")"
    a_before="$(file_mtime "$TREE/src/a.txt")"
    launch_tree_snapshot "$REPO" "$TREE" >/dev/null
    assert_eq "$keep_before" "$(file_mtime "$TREE/src/keep.txt")" "unchanged state touches nothing"
    echo edited > "$REPO/src/a.txt"
    launch_tree_snapshot "$REPO" "$TREE" >/dev/null
    assert_eq "$keep_before" "$(file_mtime "$TREE/src/keep.txt")" "an unrelated file keeps its mtime"
    if [ "$(file_mtime "$TREE/src/a.txt")" != "$a_before" ]; then pass "the changed file is rewritten"; else fail "changed file kept its mtime"; fi
    assert_eq edited "$(cat "$TREE/src/a.txt")" "with the new content"
}

test_ensure_recreates_a_wiped_tree() {
    echo "test: ensure re-adds the tree after target/ was wiped"
    new_repo
    launch_tree_ensure "$REPO" "$TREE" || fail "first ensure failed"
    launch_tree_snapshot "$REPO" "$TREE" >/dev/null
    rm -rf "$REPO/target"
    launch_tree_ensure "$REPO" "$TREE" || fail "ensure after wipe failed"
    launch_tree_snapshot "$REPO" "$TREE" >/dev/null || fail "snapshot after wipe failed"
    assert_eq one "$(cat "$TREE/src/a.txt" 2>/dev/null)" "content is back"
    local count
    count="$(git -C "$REPO" worktree list --porcelain | grep -c '^worktree ')"
    assert_eq 2 "$count" "one launch tree registered, not two"
    launch_tree_ensure "$REPO" "$TREE" || fail "ensure on a healthy tree failed"
    assert_eq one "$(cat "$TREE/src/a.txt" 2>/dev/null)" "a healthy tree is reused"
}

test_versions_are_seeded_once() {
    echo "test: gitignored VERSION files are seeded only when the tree has none"
    new_repo
    mkdir -p "$REPO/crates/engine"
    printf 'crates/*/VERSION\ncrates/*/.version_stamp\n' >> "$REPO/.gitignore"
    echo '[package]' > "$REPO/crates/engine/Cargo.toml"
    echo 2026.01.01.3 > "$REPO/crates/engine/VERSION"
    echo abc > "$REPO/crates/engine/.version_stamp"
    launch_tree_ensure "$REPO" "$TREE" || fail "ensure failed"
    launch_tree_snapshot "$REPO" "$TREE" >/dev/null
    launch_tree_seed_versions "$REPO" "$TREE"
    assert_eq 2026.01.01.3 "$(cat "$TREE/crates/engine/VERSION" 2>/dev/null)" "VERSION seeded"
    assert_eq abc "$(cat "$TREE/crates/engine/.version_stamp" 2>/dev/null)" "stamp seeded"
    echo 2026.02.02.1 > "$TREE/crates/engine/VERSION"
    launch_tree_seed_versions "$REPO" "$TREE"
    assert_eq 2026.02.02.1 "$(cat "$TREE/crates/engine/VERSION")" "the tree's own VERSION wins later"
}

test_exec_in_snapshots_then_runs_inside() {
    echo "test: launch_tree_exec_in snapshots when asked, then runs in the tree"
    new_repo
    mkdir -p "$REPO/crates/engine"
    echo 'crates/*/VERSION' >> "$REPO/.gitignore"
    echo '[package]' > "$REPO/crates/engine/Cargo.toml"
    echo 2026.01.01.3 > "$REPO/crates/engine/VERSION"
    launch_tree_ensure "$REPO" "$TREE" || fail "ensure failed"
    echo fresh > "$REPO/src/a.txt"
    local out
    out="$(launch_tree_exec_in "$REPO" "$TREE" 1 sh -c 'pwd -P; cat src/a.txt' 2>/dev/null)"
    assert_eq "$TREE"$'\n'fresh "$out" "runs in the snapshot"
    assert_eq 2026.01.01.3 "$(cat "$TREE/crates/engine/VERSION" 2>/dev/null)" "the snapshot seeds VERSION"
    echo later > "$REPO/src/a.txt"
    out="$(launch_tree_exec_in "$REPO" "$TREE" "" cat src/a.txt 2>/dev/null)"
    assert_eq fresh "$out" "no snapshot when not asked"
}

test_lock_waits_and_never_leaks_to_children() {
    echo "test: launch_tree_locked waits for a holder and no child keeps the lock"
    new_repo
    local lock child_pid
    lock="$(launch_tree_lock_path "$REPO")"
    start_lock_holder "$SANDBOX" "$lock" ex
    channel_open 3 "$SANDBOX/waiting-$_REPO_SEQ"
    (
        _flock_waiting() { echo waiting >&3; }
        launch_tree_locked "$REPO" touch "$SANDBOX/ran-$_REPO_SEQ"
    ) &
    local waiter=$! _
    read -r _ <&3
    if [ -e "$SANDBOX/ran-$_REPO_SEQ" ]; then fail "body ran under a held lock"; else pass "second build waits"; fi
    release_lock_holder "$HOLDER_PID" "$HOLDER_RELEASE"
    wait "$waiter"
    channel_close 3
    if [ -e "$SANDBOX/ran-$_REPO_SEQ" ]; then pass "body ran once the lock was free"; else fail "body never ran"; fi

    spawn_daemon() {
        if free_for_others "$lock"; then fail "lock free inside the body"; else pass "held inside the body"; fi
        sleep 300 &
        child_pid=$!
    }
    launch_tree_locked "$REPO" spawn_daemon
    if free_for_others "$lock"; then pass "a surviving child does not hold the lock"; else fail "child inherited the lock"; fi
    kill "$child_pid" 2>/dev/null
    wait "$child_pid" 2>/dev/null
}

test_killed_build_frees_the_lock() {
    echo "test: a build shell killed mid-build leaves the lock free"
    new_repo
    local lock shell_pid _
    lock="$(launch_tree_lock_path "$REPO")"
    channel_open 3 "$SANDBOX/inside-$_REPO_SEQ"
    mkfifo "$SANDBOX/never-$_REPO_SEQ"
    bash -c 'source "$1/launch_tree.sh"; never="$3"
        body() { echo inside >&3; exec 4<>"$never"; read -r _ <&4; }
        launch_tree_locked "$2" body' _ "$SCRIPT_DIR" "$REPO" "$SANDBOX/never-$_REPO_SEQ" &
    shell_pid=$!
    read -r _ <&3
    channel_close 3
    kill -9 "$shell_pid"
    wait "$shell_pid" 2>/dev/null
    # The holder polls its parent once a second; give it a bounded wait.
    if perl -MFcntl=:flock -e 'alarm 10; open(my $f, ">>", $ARGV[0]) or exit 2; flock($f, LOCK_EX) ? exit 0 : exit 1' "$lock"; then
        pass "lock freed after the shell died"
    else
        fail "lock still held after the shell died"
    fi
}

test_wanted_only_in_a_primary_tree
test_switch_turns_the_launch_tree_off
test_killed_build_frees_the_lock
test_snapshot_is_the_working_tree
test_live_edits_after_the_snapshot_stay_out
test_unchanged_files_keep_their_mtime
test_ensure_recreates_a_wiped_tree
test_versions_are_seeded_once
test_exec_in_snapshots_then_runs_inside
test_lock_waits_and_never_leaks_to_children

echo ""
echo "launch_tree_test: $PASS passed, $FAIL failed"
[ "$FAIL" -eq 0 ]
