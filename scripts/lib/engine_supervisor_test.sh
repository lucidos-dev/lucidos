#!/bin/bash
# Tests for scripts/lib/engine_supervisor.sh — the watchdog loop that
# auto-restarts the engine on unexpected death (SIGKILL, OOM, panic) but
# stays out of the way of legitimate stops (clean exit, SIGUSR1 from
# kill_stale_processes / stop.sh / /api/v1/restart, SIGINT from Ctrl-C).
#
# Run: ./scripts/lib/engine_supervisor_test.sh
set -u

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"

SANDBOX="$(mktemp -d)"
CLEANUP_PIDS=()
cleanup() {
    local pid
    for pid in "${CLEANUP_PIDS[@]}"; do
        kill -KILL "$pid" 2>/dev/null || true
    done
    rm -rf "$SANDBOX"
}
trap cleanup EXIT

PASS=0
FAIL=0
fail() { echo "  FAIL: $*"; FAIL=$((FAIL+1)); }
pass() { echo "  ok:   $*"; PASS=$((PASS+1)); }

# shellcheck source=engine_supervisor.sh
source "$SCRIPT_DIR/engine_supervisor.sh"

# Block until $1 exists and is non-empty, or timeout after $2 seconds.
# Returns 0 on success, 1 on timeout.
wait_for_file() {
    local file="$1"
    local timeout_s="${2:-3}"
    local deadline=$(( $(date +%s) + timeout_s ))
    while [ "$(date +%s)" -lt "$deadline" ]; do
        [ -s "$file" ] && return 0
        sleep 0.1
    done
    return 1
}

# Block until $1 (a pid) has exited, or timeout after $2 seconds.
wait_for_exit() {
    local pid="$1"
    local timeout_s="${2:-5}"
    local deadline=$(( $(date +%s) + timeout_s ))
    while [ "$(date +%s)" -lt "$deadline" ]; do
        kill -0 "$pid" 2>/dev/null || return 0
        sleep 0.1
    done
    return 1
}

# Kill the pid recorded in $1, if any, and remove the file. Used in
# test teardown after `run_supervised` has spawned a long-running mock
# (`sleep 600`) — the supervisor is gone by then but the orphaned mock
# would otherwise leak.
kill_pidfile_pid() {
    local pidfile="$1"
    [ -s "$pidfile" ] || return 0
    kill -KILL "$(cat "$pidfile")" 2>/dev/null || true
}

# Block until $1 (a file with an integer) reaches at least $2, or timeout.
wait_for_counter() {
    local file="$1"
    local target="$2"
    local timeout_s="${3:-15}"
    local deadline=$(( $(date +%s) + timeout_s ))
    while [ "$(date +%s)" -lt "$deadline" ]; do
        local n
        n="$(cat "$file" 2>/dev/null || echo 0)"
        [ "$n" -ge "$target" ] 2>/dev/null && return 0
        sleep 0.1
    done
    return 1
}

# ── Test 1: pidfile is written after launch ─────────────────────────────
test_pidfile_written_on_launch() {
    echo "test: pidfile is written after launching the child"
    local tdir="$SANDBOX/t1"
    mkdir -p "$tdir"
    local pidfile="$tdir/pid"
    local logfile="$tdir/log"

    ( run_supervised "$pidfile" "$logfile" sleep 600 ) &
    local sup_pid=$!
    CLEANUP_PIDS+=("$sup_pid")

    if ! wait_for_file "$pidfile" 3; then
        fail "pidfile $pidfile not written within 3s"
        return
    fi
    local child_pid
    child_pid="$(cat "$pidfile")"
    if [ -z "$child_pid" ] || ! kill -0 "$child_pid" 2>/dev/null; then
        fail "pid in $pidfile ($child_pid) is not a live process"
        return
    fi
    pass "pidfile contains live child pid $child_pid"
    kill -KILL "$sup_pid" "$child_pid" 2>/dev/null || true
    wait_for_exit "$sup_pid" 3
}

# ── Tests 2–4: each clean-exit code (0, 130, 138) stops the supervisor ──
# 0 = graceful_shutdown completed. 130 = SIGINT default action (Ctrl-C
# before handler installed). 138 = SIGUSR1 default action (signal arrived
# before handler installed; /api/v1/restart relies on this path when the
# engine catches SIGUSR1 itself, exit 138 covers the cold-boot race).
assert_clean_exit_stops_loop() {
    local exit_code="$1"
    local tname="$2"
    echo "test: child exit $exit_code stops the supervisor"
    local tdir="$SANDBOX/$tname"
    mkdir -p "$tdir"
    local pidfile="$tdir/pid"
    local logfile="$tdir/log"
    local counter="$tdir/counter"
    echo 0 > "$counter"

    local mock="$tdir/mock"
    cat > "$mock" <<EOF
#!/bin/bash
n=\$(cat "$counter")
echo \$((n + 1)) > "$counter"
exit $exit_code
EOF
    chmod +x "$mock"

    ( run_supervised "$pidfile" "$logfile" "$mock" ) &
    local sup_pid=$!
    CLEANUP_PIDS+=("$sup_pid")

    if ! wait_for_exit "$sup_pid" 5; then
        fail "supervisor did not exit within 5s after child exit $exit_code"
        kill -KILL "$sup_pid" 2>/dev/null || true
        return
    fi
    local n
    n="$(cat "$counter")"
    if [ "$n" -ne 1 ]; then
        fail "expected child to run once on exit $exit_code, ran $n times"
        return
    fi
    pass "supervisor exited after a single exit $exit_code"
}

test_clean_exit_zero_stops_loop() { assert_clean_exit_stops_loop 0 t2; }
test_clean_exit_130_stops_loop()  { assert_clean_exit_stops_loop 130 t3; }
test_clean_exit_138_stops_loop()  { assert_clean_exit_stops_loop 138 t4; }

# ── Test 5: non-zero exit triggers restart ──────────────────────────────
# Mocks the engine dying from SIGKILL (exit 137) — verify the supervisor
# spawns it again. This is THE behavior change the watchdog adds.
test_unexpected_exit_137_restarts() {
    echo "test: child exit 137 (SIGKILL) triggers restart"
    local tdir="$SANDBOX/t5"
    mkdir -p "$tdir"
    local pidfile="$tdir/pid"
    local logfile="$tdir/log"
    local counter="$tdir/counter"
    echo 0 > "$counter"

    # Crash twice (exit 137), then sleep on the third invocation so the
    # test can observe restart-loop progress and clean up.
    local mock="$tdir/mock"
    cat > "$mock" <<EOF
#!/bin/bash
n=\$(cat "$counter")
echo \$((n + 1)) > "$counter"
if [ "\$n" -ge 2 ]; then
    sleep 600
else
    exit 137
fi
EOF
    chmod +x "$mock"

    ( run_supervised "$pidfile" "$logfile" "$mock" ) &
    local sup_pid=$!
    CLEANUP_PIDS+=("$sup_pid")

    # The third invocation runs `sleep 600`; that means at least one
    # restart happened past the initial death. Allow generous time for
    # backoff (1s then 2s between rapid restarts).
    if ! wait_for_counter "$counter" 3 15; then
        fail "child only ran $(cat "$counter") times — expected ≥ 3 (initial + 2 restarts)"
        kill -KILL "$sup_pid" 2>/dev/null || true
        kill_pidfile_pid "$pidfile"
        return
    fi
    pass "supervisor restarted child after exit 137 (ran $(cat "$counter") times)"
    kill -KILL "$sup_pid" 2>/dev/null || true
    kill_pidfile_pid "$pidfile"
    wait_for_exit "$sup_pid" 3
}

# ── Test 6: pidfile is updated on each restart ──────────────────────────
# Without this, kill_stale_processes (which reads the pidfile to send
# SIGUSR1) would target a dead pid after the supervisor respawns the
# engine — and the new engine would survive the legit shutdown.
test_pidfile_updates_on_restart() {
    echo "test: pidfile is rewritten to the new pid after a restart"
    local tdir="$SANDBOX/t6"
    mkdir -p "$tdir"
    local pidfile="$tdir/pid"
    local logfile="$tdir/log"
    local counter="$tdir/counter"
    echo 0 > "$counter"

    # First invocation crashes (exit 137), second sleeps so we can read
    # its pid from the file.
    local mock="$tdir/mock"
    cat > "$mock" <<EOF
#!/bin/bash
n=\$(cat "$counter")
echo \$((n + 1)) > "$counter"
if [ "\$n" -ge 1 ]; then
    sleep 600
else
    exit 137
fi
EOF
    chmod +x "$mock"

    ( run_supervised "$pidfile" "$logfile" "$mock" ) &
    local sup_pid=$!
    CLEANUP_PIDS+=("$sup_pid")

    if ! wait_for_file "$pidfile" 3; then
        fail "pidfile not written for first invocation"
        kill -KILL "$sup_pid" 2>/dev/null || true
        return
    fi
    local first_pid
    first_pid="$(cat "$pidfile")"

    if ! wait_for_counter "$counter" 2 10; then
        fail "supervisor did not restart child within 10s"
        kill -KILL "$sup_pid" 2>/dev/null || true
        kill_pidfile_pid "$pidfile"
        return
    fi

    # After restart, the pidfile should hold a new pid (different from
    # the first, and alive).
    local deadline=$(( $(date +%s) + 5 ))
    local second_pid=""
    while [ "$(date +%s)" -lt "$deadline" ]; do
        second_pid="$(cat "$pidfile" 2>/dev/null || true)"
        if [ -n "$second_pid" ] && [ "$second_pid" != "$first_pid" ] && kill -0 "$second_pid" 2>/dev/null; then
            break
        fi
        sleep 0.1
    done

    if [ -z "$second_pid" ] || [ "$second_pid" = "$first_pid" ]; then
        fail "pidfile still holds first pid $first_pid after restart"
    elif ! kill -0 "$second_pid" 2>/dev/null; then
        fail "pidfile holds dead pid $second_pid after restart"
    else
        pass "pidfile updated from $first_pid → $second_pid on restart"
    fi
    kill -KILL "$sup_pid" 2>/dev/null || true
    kill_pidfile_pid "$pidfile"
    wait_for_exit "$sup_pid" 3
}

# ── Test 7: SIGTERM to supervisor signals SIGUSR1 to engine, then exits ─
# This is the key safety property: when web-dev.sh kill_stale_processes
# does `pkill -P <old_web_dev>` (SIGTERM to direct children = supervisor),
# the supervisor must NOT restart the engine that's about to be SIGUSR1'd
# by the same kill_stale_processes call. Instead it forwards the shutdown
# intent to the engine as SIGUSR1 and exits.
test_sigterm_to_supervisor_signals_engine_then_exits() {
    echo "test: SIGTERM to supervisor → SIGUSR1 to child → supervisor exits"
    local tdir="$SANDBOX/t7"
    mkdir -p "$tdir"
    local pidfile="$tdir/pid"
    local logfile="$tdir/log"
    local marker="$tdir/got_usr1"
    local ready="$tdir/ready"

    # The mock writes `ready` only after its SIGUSR1 trap is installed.
    # The test waits for `ready` before sending SIGTERM to the supervisor,
    # so SIGUSR1 from the supervisor lands on an armed handler (a real
    # engine is past handler-install by the time anyone targets it; this
    # mirrors that invariant in the test fixture).
    local mock="$tdir/mock"
    cat > "$mock" <<EOF
#!/bin/bash
trap 'echo got > "$marker"; exit 0' SIGUSR1
echo ready > "$ready"
sleep 600 &
wait \$!
EOF
    chmod +x "$mock"

    ( run_supervised "$pidfile" "$logfile" "$mock" ) &
    local sup_pid=$!
    CLEANUP_PIDS+=("$sup_pid")

    if ! wait_for_file "$pidfile" 3; then
        fail "pidfile not written"
        kill -KILL "$sup_pid" 2>/dev/null || true
        return
    fi
    local child_pid
    child_pid="$(cat "$pidfile")"

    if ! wait_for_file "$ready" 3; then
        fail "child did not become ready (trap not installed) within 3s"
        kill -KILL "$sup_pid" "$child_pid" 2>/dev/null || true
        return
    fi

    kill -TERM "$sup_pid"

    if ! wait_for_exit "$sup_pid" 5; then
        fail "supervisor did not exit within 5s after SIGTERM"
        kill -KILL "$sup_pid" 2>/dev/null || true
        kill -KILL "$child_pid" 2>/dev/null || true
        return
    fi

    if [ ! -f "$marker" ]; then
        fail "child did not receive SIGUSR1 (no $marker)"
        return
    fi

    # Child should also be gone (it exited 0 in its USR1 trap).
    if kill -0 "$child_pid" 2>/dev/null; then
        fail "child pid $child_pid still alive after supervisor exit"
        kill -KILL "$child_pid" 2>/dev/null || true
        return
    fi

    pass "SIGTERM cleanly stopped supervisor + child via SIGUSR1"
}

# ── Test 8: respawn sidecar written on unexpected death ──────────────────
# The bash supervisor writes <pidfile_dir>/engine.last-death.json before
# respawning, carrying {old_pid, exit_code, died_at, supervisor_pid}. The
# next engine reads + emits + deletes it (see
# engine::supervisor_respawn_sidecar). Without the sidecar, a silent
# supervisor respawn would leave no audit-timeline trace.
test_respawn_sidecar_written_on_unexpected_death() {
    echo "test: respawn sidecar carries old_pid + exit_code on unexpected death"
    local tdir="$SANDBOX/t8"
    mkdir -p "$tdir"
    local pidfile="$tdir/pid"
    local logfile="$tdir/log"
    local sidecar="$tdir/engine.last-death.json"
    local counter="$tdir/counter"
    echo 0 > "$counter"

    # Crash with exit 137 on first invocation, sleep on subsequent so the
    # sidecar persists long enough to inspect.
    local mock="$tdir/mock"
    cat > "$mock" <<EOF
#!/bin/bash
n=\$(cat "$counter")
echo \$((n + 1)) > "$counter"
if [ "\$n" -eq 0 ]; then
    exit 137
else
    sleep 600
fi
EOF
    chmod +x "$mock"

    ( run_supervised "$pidfile" "$logfile" "$mock" ) &
    local sup_pid=$!
    CLEANUP_PIDS+=("$sup_pid")

    # Wait for the supervisor to have respawned past the first death.
    if ! wait_for_counter "$counter" 2 10; then
        fail "supervisor did not respawn within 10s"
        kill -KILL "$sup_pid" 2>/dev/null || true
        kill_pidfile_pid "$pidfile"
        return
    fi

    if [ ! -f "$sidecar" ]; then
        fail "sidecar $sidecar was not written"
    elif ! grep -q '"exit_code":137' "$sidecar"; then
        fail "sidecar missing exit_code=137 — content: $(cat "$sidecar")"
    elif ! grep -q '"old_pid":' "$sidecar"; then
        fail "sidecar missing old_pid — content: $(cat "$sidecar")"
    elif ! grep -q "\"supervisor_pid\":$sup_pid" "$sidecar"; then
        fail "sidecar supervisor_pid != $sup_pid — content: $(cat "$sidecar")"
    else
        pass "sidecar contains exit_code=137 + old_pid + supervisor_pid=$sup_pid"
    fi

    kill -KILL "$sup_pid" 2>/dev/null || true
    kill_pidfile_pid "$pidfile"
    wait_for_exit "$sup_pid" 3
}

test_pidfile_written_on_launch
test_clean_exit_zero_stops_loop
test_clean_exit_130_stops_loop
test_clean_exit_138_stops_loop
test_unexpected_exit_137_restarts
test_pidfile_updates_on_restart
test_sigterm_to_supervisor_signals_engine_then_exits
test_respawn_sidecar_written_on_unexpected_death

echo ""
echo "Passed: $PASS  Failed: $FAIL"
[ $FAIL -eq 0 ]
