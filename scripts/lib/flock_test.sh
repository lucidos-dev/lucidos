#!/bin/bash
# Tests for scripts/lib/flock.sh (ADR 0438).
#
# Hermetic. Every lock lives under a mktemp sandbox, and the only processes
# signalled are holders this file spawned. Waits are FIFO rendezvous, never
# sleeps.
#
# Run: ./scripts/lib/flock_test.sh
set -u

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
SANDBOX="$(mktemp -d)"
trap 'rm -rf "$SANDBOX"' EXIT

PASS=0
FAIL=0
pass() { echo "  ok:   $*"; PASS=$((PASS + 1)); }
fail() { echo "  FAIL: $*"; FAIL=$((FAIL + 1)); }
assert_eq() { # <expected> <actual> <label>
    if [ "$1" = "$2" ]; then pass "$3"; else fail "$3 (expected '$1', got '$2')"; fi
}

# shellcheck source=scripts/lib/flock.sh
source "$SCRIPT_DIR/flock.sh"
# shellcheck source=scripts/lib/flock_test_support.sh
source "$SCRIPT_DIR/flock_test_support.sh"

# Whether a separate process could take <lock> exclusively right now.
free_for_others() { # <lock>
    perl -MFcntl=:flock -e 'open(my $f, ">>", $ARGV[0]) or exit 2; flock($f, LOCK_EX | LOCK_NB) ? exit 0 : exit 1' "$1"
}

test_open_holds_until_close() {
    echo "test: flock_open holds the lock after perl exits, until flock_close"
    local lock="$SANDBOX/open.lock"
    flock_open 7 "$lock" ex || fail "flock_open failed"
    if free_for_others "$lock"; then fail "another process took a held lock"; else pass "held across shell code"; fi
    flock_close 7
    if free_for_others "$lock"; then pass "released by flock_close"; else fail "still held after close"; fi
}

test_try_refuses_a_held_lock() {
    echo "test: flock_try refuses a held lock and leaves its descriptor closed"
    local lock="$SANDBOX/try.lock" rc=0
    start_lock_holder "$SANDBOX" "$lock" ex
    flock_try 7 "$lock" ex || rc=$?
    assert_eq 1 "$rc" "contended"
    if { true >&7; } 2>/dev/null; then fail "descriptor 7 left open"; else pass "descriptor closed"; fi
    release_lock_holder "$HOLDER_PID" "$HOLDER_RELEASE"
    rc=0
    flock_try 7 "$lock" ex || rc=$?
    assert_eq 0 "$rc" "free once the holder is gone"
    flock_close 7
}

test_shared_locks_coexist() {
    echo "test: two shared holders coexist, and an exclusive try is refused"
    local lock="$SANDBOX/shared.lock" rc=0
    start_lock_holder "$SANDBOX" "$lock" sh
    local first="$HOLDER_PID" first_release="$HOLDER_RELEASE"
    flock_try 7 "$lock" sh || rc=$?
    assert_eq 0 "$rc" "a second shared lock is granted"
    flock_close 7
    rc=0
    flock_try 7 "$lock" ex || rc=$?
    assert_eq 1 "$rc" "exclusive refused under a shared holder"
    release_lock_holder "$first" "$first_release"
}

test_open_waits_once_then_takes_it() {
    echo "test: a contended flock_open reports waiting once, then waits for the holder"
    local lock="$SANDBOX/wait.lock" events="$SANDBOX/wait.events" line
    channel_open 6 "$events"
    start_lock_holder "$SANDBOX" "$lock" ex
    (
        _flock_waiting() { echo waiting > "$events"; }
        flock_open 7 "$lock" ex && echo locked > "$events"
    ) &
    local waiter=$!
    read -r line <&6
    assert_eq waiting "$line" "the waiter reported waiting before it held"
    release_lock_holder "$HOLDER_PID" "$HOLDER_RELEASE"
    read -r line <&6
    assert_eq locked "$line" "it took the lock once the holder released"
    wait "$waiter"
    channel_close 6
}

test_lock_passes_to_a_child() {
    echo "test: a child that inherits the descriptor keeps the lock after the parent closes"
    local lock="$SANDBOX/handover.lock" go="$SANDBOX/handover.go"
    channel_open 6 "$go"
    flock_open 7 "$lock" ex || fail "flock_open failed"
    ( read -r _ <&6 ) &
    local child=$!
    flock_close 7
    if lock_is_held "$lock"; then pass "held by the child"; else fail "released with the parent's copy"; fi
    echo go >&6
    wait "$child"
    channel_close 6
    if lock_is_held "$lock"; then fail "held after the child exited"; else pass "released with the child"; fi
}

test_death_releases_the_lock() {
    echo "test: a SIGKILLed holder leaves nothing to reclaim"
    local lock="$SANDBOX/death.lock" rc=0
    start_lock_holder "$SANDBOX" "$lock" ex
    kill -KILL "$HOLDER_PID"
    wait "$HOLDER_PID" 2>/dev/null || true
    flock_try 7 "$lock" ex || rc=$?
    assert_eq 0 "$rc" "free at once"
    flock_close 7
}

test_lock_is_held_answers() {
    echo "test: lock_is_held is 1 for a missing or free lock and 0 for a held one"
    local lock="$SANDBOX/probe.lock" rc=0
    lock_is_held "$lock" || rc=$?
    assert_eq 1 "$rc" "missing file"
    if [ ! -e "$lock" ]; then pass "the probe created nothing"; else fail "the probe created the file"; fi
    : >"$lock"
    rc=0
    lock_is_held "$lock" || rc=$?
    assert_eq 1 "$rc" "free"
    start_lock_holder "$SANDBOX" "$lock" ex
    rc=0
    lock_is_held "$lock" || rc=$?
    assert_eq 0 "$rc" "held"
    release_lock_holder "$HOLDER_PID" "$HOLDER_RELEASE"
}

test_live_pid_needs_the_lock() {
    echo "test: live_pid prints a pid only while its lock is held"
    local ws="$SANDBOX/ws" got
    mkdir -p "$ws/.lucidos"
    : >"$(engine_lock_path "$ws")"
    channel_open 6 "$SANDBOX/bystander.go"
    # A live process named in the pidfile, with nobody holding the lock: the
    # shape of a pid reused after its engine died.
    ( read -r _ <&6 ) &
    local bystander=$!
    echo "$bystander" > "$(engine_pid_path "$ws")"
    got="$(engine_live_pid "$ws")"
    assert_eq "" "$got" "a live pid with no lock held is not trusted"
    echo go >&6
    wait "$bystander"
    channel_close 6

    start_lock_holder "$SANDBOX" "$(engine_lock_path "$ws")" ex "$(engine_pid_path "$ws")"
    got="$(engine_live_pid "$ws")"
    assert_eq "$HOLDER_PID" "$got" "the holder's published pid"
    echo "not-a-pid" > "$(engine_pid_path "$ws")"
    got="$(engine_live_pid "$ws")"
    assert_eq "" "$got" "garbage is not a pid"
    echo 1 > "$(engine_pid_path "$ws")"
    got="$(engine_live_pid "$ws")"
    assert_eq "" "$got" "pid 1 is never a pid to signal"
    release_lock_holder "$HOLDER_PID" "$HOLDER_RELEASE"
}

test_a_pre_lock_workspace_keeps_its_pidfile() {
    echo "test: with no lock file, the pidfile counts while it names a live lucidos-engine"
    local ws="$SANDBOX/ws-pre-lock" got
    mkdir -p "$ws/.lucidos"
    echo "$$" >"$(engine_pid_path "$ws")"
    # `ps` is stubbed in each subshell, so argv[0] is fixed and no exec is raced.
    got="$( ps() { echo "/opt/lucidos/lucidos-engine --serve"; }; engine_live_pid "$ws" )"
    assert_eq "$$" "$got" "an engine from before the lock is still found"
    got="$( ps() { echo "/bin/bash ./some-script.sh"; }; engine_live_pid "$ws" )"
    assert_eq "" "$got" "a reused pid running another program is not"
    got="$( ps() { return 1; }; engine_live_pid "$ws" )"
    assert_eq "" "$got" "a dead pid is not"
    : >"$(engine_lock_path "$ws")"
    got="$( ps() { echo "/opt/lucidos/lucidos-engine --serve"; }; engine_live_pid "$ws" )"
    assert_eq "" "$got" "once a lock file exists, only a held lock vouches"
}

test_a_waiting_successor_is_found_only_while_it_waits() {
    echo "test: engine_waiting_pid names a successor only while it holds the successor lock"
    local ws="$SANDBOX/ws-successor" got
    mkdir -p "$ws/.lucidos"
    start_lock_holder "$SANDBOX" "$(engine_successor_lock_path "$ws")" ex "$(engine_successor_pid_path "$ws")"
    got="$(engine_waiting_pid "$ws")"
    assert_eq "$HOLDER_PID" "$got" "the waiting successor"
    release_lock_holder "$HOLDER_PID" "$HOLDER_RELEASE"
    got="$(engine_waiting_pid "$ws")"
    assert_eq "" "$got" "nobody once it stopped waiting"
}

test_only_a_running_engine_is_signalled() {
    echo "test: engine_pids_to_signal names the owner and a waiting successor that run lucidos-engine"
    local ws="$SANDBOX/ws-signal" got owner waiter
    mkdir -p "$ws/.lucidos"
    start_lock_holder "$SANDBOX" "$(engine_lock_path "$ws")" ex "$(engine_pid_path "$ws")"
    owner="$HOLDER_PID"; local owner_release="$HOLDER_RELEASE"
    start_lock_holder "$SANDBOX" "$(engine_successor_lock_path "$ws")" ex "$(engine_successor_pid_path "$ws")"
    waiter="$HOLDER_PID"
    got="$( ps() { echo "/opt/lucidos/lucidos-engine"; }; engine_pids_to_signal "$ws" | tr '\n' ' ' )"
    assert_eq "$owner $waiter " "$got" "both engines, owner first"
    # The lock check and the signal are two steps; a reused pid runs another program.
    got="$( ps() { echo "/usr/bin/vim notes.txt"; }; engine_pids_to_signal "$ws" )"
    assert_eq "" "$got" "a pid now running another program is not signalled"
    release_lock_holder "$waiter" "$HOLDER_RELEASE"
    release_lock_holder "$owner" "$owner_release"
}

test_live_pid_survives_set_e() {
    echo "test: live_pid exits 0 under set -e when it prints nothing"
    local rc=0
    ( set -e; live_pid "$SANDBOX/none.lock" "$SANDBOX/none.pid" >/dev/null; echo reached ) >/dev/null || rc=$?
    assert_eq 0 "$rc" "no abort"
}

test_open_holds_until_close
test_try_refuses_a_held_lock
test_shared_locks_coexist
test_open_waits_once_then_takes_it
test_lock_passes_to_a_child
test_death_releases_the_lock
test_lock_is_held_answers
test_live_pid_needs_the_lock
test_a_pre_lock_workspace_keeps_its_pidfile
test_a_waiting_successor_is_found_only_while_it_waits
test_only_a_running_engine_is_signalled
test_live_pid_survives_set_e

echo ""
echo "Passed: $PASS  Failed: $FAIL"
[ "$FAIL" -eq 0 ]
