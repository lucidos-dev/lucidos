#!/bin/bash
# Tests that build_or_find_engine (scripts/lib/workspace.sh) compiles a primary
# checkout once, in its launch tree, however HEAD moves (ADR 0668).
#
# Hermetic. Real git runs in a mktemp sandbox with its own HOME. Cargo, the
# build slot and the preflight are stubs, so nothing compiles,
# signs or boots, and no process outside this file is touched.
#
# Run: ./scripts/lib/launch_tree_build_test.sh
set -u

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
SANDBOX="$(cd "$(mktemp -d)" && pwd -P)"
trap 'rm -rf "$SANDBOX"' EXIT

export HOME="$SANDBOX/home"
export GIT_CONFIG_NOSYSTEM=1
export GIT_AUTHOR_NAME=Test GIT_AUTHOR_EMAIL=test@example.com
export GIT_COMMITTER_NAME=Test GIT_COMMITTER_EMAIL=test@example.com
mkdir -p "$HOME" "$SANDBOX/bin"

PASS=0
FAIL=0
pass() { echo "  ok:   $*"; PASS=$((PASS + 1)); }
fail() { echo "  FAIL: $*"; FAIL=$((FAIL + 1)); }
assert_eq() { # <expected> <actual> <label>
    if [ "$1" = "$2" ]; then pass "$3"; else fail "$3 (expected '$1', got '$2')"; fi
}

# shellcheck source=scripts/lib/workspace.sh
source "$SCRIPT_DIR/workspace.sh"

# Logs `<cwd>|<CARGO_TARGET_DIR>|<args>` per call. A build writes stand-in
# binaries stamped with the commit of the tree it runs in. MOVE_HEAD names a
# checkout whose HEAD the compile moves, as an Apply would.
cat > "$SANDBOX/bin/cargo" <<'CARGO'
#!/bin/bash
echo "$PWD|${CARGO_TARGET_DIR:-}|$*" >> "$CARGO_LOG"
[ "$1" = build ] || exit 0
out="${CARGO_TARGET_DIR:-$PWD/target}/debug"
mkdir -p "$out"
head="$(git rev-parse --short HEAD)"
for bin in lucidos-engine lucidos-gateway lucidos lucidos-app lucidos-codegen; do
    printf '#!/bin/sh\ncase "$*" in *--source-state*) echo stale ;; *) echo %s ;; esac\n' \
        "$head" > "$out/$bin"
    chmod +x "$out/$bin"
done
if [ -n "${MOVE_HEAD:-}" ]; then
    echo "moved from $head" > "$MOVE_HEAD/src/a.txt"
    git -C "$MOVE_HEAD" commit -qam moved
fi
CARGO
chmod +x "$SANDBOX/bin/cargo"
export PATH="$SANDBOX/bin:$PATH"

_REPO_SEQ=0
# A primary checkout with a pass-through build slot. Sets REPO and TREE.
new_repo() {
    _REPO_SEQ=$((_REPO_SEQ + 1))
    REPO="$SANDBOX/repo-$_REPO_SEQ"
    mkdir -p "$REPO/src" "$REPO/scripts"
    git -C "$REPO" init -q
    printf 'target/\n.launch/\n' > "$REPO/.gitignore"
    echo one > "$REPO/src/a.txt"
    cat > "$REPO/scripts/with-build-slot.sh" <<'SLOT'
#!/bin/bash
while [ "$1" != -- ]; do shift; done
shift
exec "$@"
SLOT
    chmod +x "$REPO/scripts/with-build-slot.sh"
    cat > "$REPO/scripts/codegen.sh" <<'CODEGEN'
#!/bin/bash
echo "$(cd "$(dirname "$0")/.." && pwd -P)|${CARGO_TARGET_DIR:-}|$*" >> "$CODEGEN_LOG"
exit "${CODEGEN_RC:-0}"
CODEGEN
    chmod +x "$REPO/scripts/codegen.sh"
    git -C "$REPO" add -A
    git -C "$REPO" commit -qm first
    TREE="$(launch_tree_dir "$REPO")"
    export CARGO_LOG="$REPO.cargo.log" CODEGEN_LOG="$REPO.codegen.log"
    : > "$CARGO_LOG"
    : > "$CODEGEN_LOG"
}

# Runs the real build_or_find_engine in <checkout>, as web-dev.sh runs it.
# Output goes to $OUT, the published uplift dir to $PUBLISHED.
# shellcheck disable=SC2329 # the stubs are called by build_or_find_engine
run_build() { # <checkout>
    local checkout="$1"
    shift
    OUT="$checkout.out"
    PUBLISHED="$checkout.published"
    (
        set -e
        cd "$checkout"
        prune_stale_debug_objects() { :; }
        sign_engine_binary() { :; }
        publish_after_preflight() {
            echo "$1 ${3:-}" >> "$PUBLISHED"
            publish_all_launch_binaries "$@"
        }
        BUILD=1
        PROJECT_DIR="$checkout"
        build_or_find_engine
        echo "source=$LAUNCH_SOURCE_DIR uplift=$LAUNCH_UPLIFT_DIR"
    ) > "$OUT" 2>&1
}

builds_in() { # <log> <subcommand>
    grep -c "|$2 " "$1" || true
}

test_head_move_costs_no_second_compile() {
    echo "test: HEAD moving mid-compile costs no second compile and no exit 75"
    new_repo
    local start rc=0
    start="$(git -C "$REPO" rev-parse --short HEAD)"
    MOVE_HEAD="$REPO" run_build "$REPO" || rc=$?
    assert_eq 0 "$rc" "the build exits 0"
    assert_eq 1 "$(builds_in "$CARGO_LOG" build)" "cargo compiled once"
    assert_eq "$TREE|$TREE/target" "$(cut -d'|' -f1-2 "$CARGO_LOG")" "cargo ran in the launch tree, on its target"
    assert_eq "$TREE/target/debug " "$(cat "$PUBLISHED")" "the launch tree's binaries published"
    assert_eq one "$(cat "$TREE/src/a.txt")" "the launch tree kept the snapshot"
    local delivered="$REPO/.launch/debug/plain/lucidos-engine" current
    current="$(git -C "$REPO" rev-parse --short HEAD)"
    if [ "$current" != "$start" ]; then pass "live HEAD moved"; else fail "live HEAD did not move"; fi
    assert_eq "$start" "$("$delivered" --build-id)" "the delivered executable identifies the pinned commit"
    assert_eq "$start" "$("$REPO/.launch/debug/plain/lucidos-gateway" --build-id)" "the gateway identifies the same snapshot"
    case "$(cat "$OUT")" in
        *"Building $start"*"from the launch tree"*) pass "the log names the snapshot's commit" ;;
        *) fail "no snapshot line: $(cat "$OUT")" ;;
    esac
    case "$(cat "$OUT")" in
        *WARNING*"$(git -C "$REPO" rev-parse --short HEAD)"*"$start"*) pass "a note names both commits" ;;
        *) fail "no head-moved note: $(cat "$OUT")" ;;
    esac
    case "$(cat "$OUT")" in
        *"source=$TREE uplift=$TREE/target/debug"*) pass "source and uplift dirs point into the tree" ;;
        *) fail "wrong exported dirs: $(cat "$OUT")" ;;
    esac
}

test_fresh_clean_takes_the_snapshot() {
    echo "test: a fresh build cleans in the launch tree, and the clean takes the snapshot"
    new_repo
    local rc=0
    ENGINE_BUILD_FRESH=1 run_build "$REPO" || rc=$?
    assert_eq 0 "$rc" "the build exits 0"
    assert_eq "$TREE|$TREE/target|clean --profile dev -p lucidos-engine -p lucidos-gateway -p lucidos-cli" \
        "$(head -n 1 "$CARGO_LOG")" "the clean runs first, in the tree"
    assert_eq 1 "$(builds_in "$CARGO_LOG" build)" "cargo compiled once"
    assert_eq 1 "$(grep -c "from the launch tree" "$OUT")" "one snapshot per build"
}

test_dev_window_builds_in_the_same_snapshot() {
    echo "test: the dev window compiles in the same snapshot as the engine"
    new_repo
    local window rc=0
    window="$(dev_window_bin "$REPO/.launch/debug/plain")"
    mkdir -p "$(dirname "$window")"
    touch "$window"
    run_build "$REPO" || rc=$?
    assert_eq 0 "$rc" "the build exits 0"
    assert_eq 2 "$(builds_in "$CARGO_LOG" build)" "the engine set and the window compiled"
    assert_eq 0 "$(grep -vc "^$TREE|$TREE/target|" "$CARGO_LOG")" "both ran in the tree"
    assert_eq 1 "$(grep -c "from the launch tree" "$OUT")" "one snapshot per build"
    assert_eq "$TREE/target/debug 1" "$(cat "$PUBLISHED")" "the window publishes with the engine"
}

test_first_dev_window_builds_in_the_tree() {
    echo "test: a first dev window build compiles in the launch tree too"
    new_repo
    local rc=0 recorded="$REPO.window"
    (
        cd "$REPO"
        # shellcheck disable=SC2329 # called by build_dev_window
        publish_dev_window_binary() { echo "$1" > "$recorded"; }
        PROJECT_DIR="$REPO"
        build_dev_window "$REPO/.launch/debug/plain"
    ) > "$REPO.out" 2>&1 || rc=$?
    assert_eq 0 "$rc" "the build exits 0"
    assert_eq "$TREE|$TREE/target|build --locked -p lucidos-app" "$(cat "$CARGO_LOG")" "the window compiled in the tree"
    assert_eq "$TREE/target/debug/lucidos-app" "$(cat "$recorded" 2>/dev/null)" "the tree's window published"
}

lock_is_held() { # <lock>
    ! perl -MFcntl=:flock -e 'open(my $f, ">>", $ARGV[0]) or exit 2; flock($f, LOCK_EX | LOCK_NB) ? exit 0 : exit 1' "$1"
}

# The tree shares its checkout's git common dir, so it names the same lock.
record_after() {
    if lock_is_held "$(launch_tree_lock_path "$LAUNCH_SOURCE_DIR")"; then
        echo "held $LAUNCH_SOURCE_DIR" > "$CARGO_LOG.after"
    else
        echo "free $LAUNCH_SOURCE_DIR" > "$CARGO_LOG.after"
    fi
}

test_after_runs_under_the_lock() {
    echo "test: a build's <after> step runs on the tree, before the lock is released"
    new_repo
    local rc=0
    ENGINE_BUILD_AFTER=record_after run_build "$REPO" || rc=$?
    assert_eq 0 "$rc" "the build exits 0"
    assert_eq "held $TREE" "$(cat "$CARGO_LOG.after" 2>/dev/null)" "after ran on the tree under the lock"
    if lock_is_held "$(launch_tree_lock_path "$REPO")"; then
        fail "the lock outlived the build"
    else
        pass "the lock is free after the build"
    fi
}

test_codegen_fills_the_cache_from_the_tree() {
    echo "test: the tree's codegen fills the shared cache from the snapshot it built"
    new_repo
    local rc=0
    run_build "$REPO" || rc=$?
    assert_eq 0 "$rc" "the build exits 0"
    assert_eq "$TREE|$TREE/target|--fill-cache --bin $TREE/target/debug/lucidos-codegen" \
        "$(cat "$CODEGEN_LOG")" "the tree's codegen rendered the tree, once"
}

test_a_failed_cache_fill_keeps_the_build() {
    echo "test: a failed cache fill warns and keeps the published build"
    new_repo
    local rc=0
    CODEGEN_RC=1 run_build "$REPO" || rc=$?
    assert_eq 0 "$rc" "the build exits 0"
    assert_eq "$TREE/target/debug " "$(cat "$PUBLISHED")" "the build published"
    case "$(cat "$OUT")" in
        *WARNING*codegen*) pass "a warning names codegen" ;;
        *) fail "no codegen warning: $(cat "$OUT")" ;;
    esac
}

test_linked_worktree_builds_live() {
    echo "test: a linked worktree builds live, with no launch tree"
    new_repo
    local linked="$SANDBOX/linked-$_REPO_SEQ" rc=0
    git -C "$REPO" worktree add -q --detach "$linked" HEAD
    run_build "$linked" || rc=$?
    assert_eq 0 "$rc" "the build exits 0"
    assert_eq "$linked|" "$(cut -d'|' -f1-2 "$CARGO_LOG")" "cargo ran in the checkout, on its own target"
    if [ -e "$(launch_tree_dir "$linked")" ] || [ -e "$TREE" ]; then
        fail "a launch tree was created"
    else
        pass "no launch tree"
    fi
    assert_eq "$linked/target/debug " "$(cat "$linked.published")" "the checkout's binaries published"
    assert_eq "" "$(cat "$CODEGEN_LOG")" "no cache fill: generate_build_outputs renders live"
}

test_head_move_costs_no_second_compile
test_fresh_clean_takes_the_snapshot
test_dev_window_builds_in_the_same_snapshot
test_first_dev_window_builds_in_the_tree
test_after_runs_under_the_lock
test_codegen_fills_the_cache_from_the_tree
test_a_failed_cache_fill_keeps_the_build
test_linked_worktree_builds_live

echo ""
echo "launch_tree_build_test: $PASS passed, $FAIL failed"
[ "$FAIL" -eq 0 ]
