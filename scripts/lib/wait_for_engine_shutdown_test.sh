#!/bin/bash
# Tests for the engine-port shutdown wait in scripts/lib/workspace.sh
# (`wait_for_engine_shutdown` + its `_await_engine_port_released` core).
#
# Regression context: kill_stale_processes used to send the old engine
# SIGUSR1 and then `sleep 1` before building + launching the replacement.
# The engine's graceful-shutdown budget is 10s (main.rs), and draining an
# in-flight Claude Code session uses most of it — so the old engine still
# held the engine port long after the 1s wait. The freshly built
# replacement then died binding the port (`Error: AddrInUse`) and never
# recovered. The fix replaces the fixed sleep with a poll loop that blocks
# until the old engine is gone AND the port is free.
#
# `port_is_free` lives in ports.sh (sourced alongside workspace.sh in the
# real scripts); here we stub it so the wait logic can be exercised without
# real listening sockets.
#
# Run: ./scripts/lib/wait_for_engine_shutdown_test.sh
set -u

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"

PASS=0
FAIL=0
fail() { echo "  FAIL: $*"; FAIL=$((FAIL+1)); }
pass() { echo "  ok:   $*"; PASS=$((PASS+1)); }

# shellcheck source=workspace.sh
source "$SCRIPT_DIR/workspace.sh"

# ── Test 1: returns immediately when no pids and the port is already free ──
test_returns_when_port_free_and_no_pids() {
    echo "test: _await_engine_port_released returns 0 when port free + no pids"
    port_is_free() { return 0; }
    local start="$SECONDS"
    if _await_engine_port_released 65000 "" 5; then
        if [ $(( SECONDS - start )) -le 1 ]; then
            pass "returned 0 promptly"
        else
            fail "returned 0 but took $(( SECONDS - start ))s (should be immediate)"
        fi
    else
        fail "expected rc 0 when port free and no pids"
    fi
    unset -f port_is_free
}

# ── Test 2: times out (rc 1) while the port stays occupied ────────────────
# This is the core of the bug: the caller must NOT proceed while the port is
# still held. With port_is_free always false, the wait must run to its
# deadline and report failure rather than returning early.
test_times_out_while_port_held() {
    echo "test: _await_engine_port_released times out while port stays held"
    port_is_free() { return 1; }
    local start="$SECONDS"
    if _await_engine_port_released 65000 "" 1; then
        fail "returned 0 even though port never freed (the old sleep-1 bug)"
    else
        local elapsed=$(( SECONDS - start ))
        if [ "$elapsed" -ge 1 ]; then
            pass "timed out (rc 1) after ${elapsed}s of a held port"
        else
            fail "returned 1 too early (${elapsed}s — did it actually poll?)"
        fi
    fi
    unset -f port_is_free
}

# ── Test 3: does not return while a signaled pid is still alive ────────────
# Even with the port free, an old engine pid that hasn't exited yet means we
# must keep waiting — its socket can still be in TIME_WAIT / mid-close.
test_waits_for_pid_to_exit() {
    echo "test: _await_engine_port_released waits for the pid to exit"
    port_is_free() { return 0; }   # port looks free the whole time

    sleep 30 &
    local live_pid=$!
    disown "$live_pid" 2>/dev/null || true   # suppress job-control "Killed" noise

    if _await_engine_port_released 65000 "$live_pid" 1; then
        fail "returned 0 while pid $live_pid was still alive"
    else
        pass "did not return while pid alive (timed out as expected)"
    fi

    # Now kill it; the same call should succeed once the pid is gone.
    kill -KILL "$live_pid" 2>/dev/null || true
    wait "$live_pid" 2>/dev/null || true
    if _await_engine_port_released 65000 "$live_pid" 2; then
        pass "returned 0 after pid exited"
    else
        fail "still did not return after pid $live_pid exited"
    fi
    unset -f port_is_free
}

# ── Test 4: returns as soon as the port flips free mid-wait ───────────────
test_returns_when_port_flips_free() {
    echo "test: _await_engine_port_released returns once the port flips free"
    # Free only after the 3rd poll.
    PORT_POLLS=0
    port_is_free() {
        PORT_POLLS=$(( PORT_POLLS + 1 ))
        [ "$PORT_POLLS" -ge 3 ]
    }
    if _await_engine_port_released 65000 "" 5; then
        if [ "$PORT_POLLS" -ge 3 ]; then
            pass "returned 0 after the port flipped free (polled $PORT_POLLS times)"
        else
            fail "returned 0 before the port was free (polled $PORT_POLLS times)"
        fi
    else
        fail "expected rc 0 once the port flipped free"
    fi
    unset -f port_is_free
    unset PORT_POLLS
}

# ── Test 5: wait_for_engine_shutdown succeeds without escalation ───────────
test_full_wait_succeeds_without_escalation() {
    echo "test: wait_for_engine_shutdown returns 0 when the port frees in time"
    port_is_free() { return 0; }
    local out rc
    out="$(wait_for_engine_shutdown 65000 "" 2 2>&1)"
    rc=$?
    if [ "$rc" -eq 0 ] && [ -z "$out" ]; then
        pass "returned 0 with no escalation output"
    else
        fail "expected rc 0 + no output, got rc=$rc out='$out'"
    fi
    unset -f port_is_free
}

# ── Test 6: wait_for_engine_shutdown escalates to SIGKILL on a wedged engine ─
# The whole point of the escalation: a graceful shutdown that overruns the
# budget must not block the rebuild forever. The port stays held until the
# wedged lucidos-engine occupant is SIGKILLed, after which the port frees.
test_full_wait_escalates_to_sigkill() {
    echo "test: wait_for_engine_shutdown SIGKILLs a wedged engine occupant"

    sleep 600 &
    OCC_PID=$!
    disown "$OCC_PID" 2>/dev/null || true   # suppress job-control "Killed" noise

    # Port is "occupied" exactly while the wedged occupant is alive.
    port_is_free() { ! kill -0 "$OCC_PID" 2>/dev/null; }
    # lsof reports the occupant; ps reports it as a lucidos-engine.
    lsof() { echo "$OCC_PID"; }
    ps() { echo "lucidos-engine"; }

    local out rc
    out="$(wait_for_engine_shutdown 65000 "" 1 2>&1)"
    rc=$?

    unset -f port_is_free lsof ps

    if kill -0 "$OCC_PID" 2>/dev/null; then
        fail "wedged occupant $OCC_PID was not killed"
        kill -KILL "$OCC_PID" 2>/dev/null || true
    elif [ "$rc" -ne 0 ]; then
        fail "expected rc 0 after force-kill freed the port, got rc=$rc out='$out'"
    elif ! echo "$out" | grep -q "force-killing"; then
        fail "expected a 'force-killing' notice on escalation, got: '$out'"
    else
        pass "force-killed the wedged engine and returned 0"
    fi
    wait "$OCC_PID" 2>/dev/null || true
    unset OCC_PID
}

# ── Test 7: warns (rc 1) when the port can't be freed even after escalation ─
test_full_wait_warns_when_unrecoverable() {
    echo "test: wait_for_engine_shutdown warns when the port stays held"
    port_is_free() { return 1; }   # never free
    lsof() { echo ""; }            # nothing to kill
    local out rc
    out="$(wait_for_engine_shutdown 65000 "" 1 2>&1)"
    rc=$?
    unset -f port_is_free lsof
    if [ "$rc" -ne 0 ] && echo "$out" | grep -qi "still occupied"; then
        pass "returned non-zero with a warning when unrecoverable"
    else
        fail "expected rc!=0 + warning, got rc=$rc out='$out'"
    fi
}

# ── Test 8: kill_stale_processes must not abort a `set -e` caller when the ──
# engine port can't be freed. wait_for_engine_shutdown returns non-zero on
# the unrecoverable path; as the tail command of kill_stale_processes (called
# bare under `set -e` in web-dev.sh / tauri-dev.sh) an unguarded non-zero
# return would abort the script before the rebuild + relaunch. The call site
# guards it with `|| true`. Runs the real kill_stale_processes in a `set -e`
# bash subprocess (this test file's own functions run under set -u, not -e).
test_kill_stale_does_not_abort_caller_under_set_e() {
    echo "test: kill_stale_processes proceeds (set -e) when the port can't be freed"
    local tmp; tmp="$(mktemp -d)"
    local script="$tmp/scenario.sh"
    cat > "$script" <<EOF
#!/bin/bash
set -e
source "$SCRIPT_DIR/workspace.sh"
BUILD=1
ENGINE_ONLY=1
VITE_PORT=65000
ENGINE_PIDFILE="$tmp/engine.pid"
sleep 30 & epid=\$!
echo "\$epid" > "\$ENGINE_PIDFILE"
# Unrecoverable: port never frees and there's nothing to SIGKILL. Stub the
# wait to fail fast instead of running the real ~18s graceful+grace window.
port_is_free() { return 1; }
lsof() { echo ""; }
wait_for_engine_shutdown() { return 1; }
kill_stale_processes
echo "REACHED_AFTER_KILL_STALE"
kill -KILL "\$epid" 2>/dev/null || true
EOF
    local out rc
    out="$(bash "$script" 2>&1)"
    rc=$?
    rm -rf "$tmp"
    if echo "$out" | grep -q "REACHED_AFTER_KILL_STALE"; then
        pass "caller continued past kill_stale_processes despite unrecoverable port"
    else
        fail "set -e aborted the caller (missing '|| true' guard); rc=$rc out: $out"
    fi
}

# ── Test 9: escalation stops the engine's supervisor before SIGKILL ───────
# A SIGKILL'd engine exits 137, which engine_supervisor.sh treats as an
# unexpected death → respawn onto the same port, fighting the rebuild. The
# escalation must first SIGTERM the engine's supervisor (its parent) so it
# exits without respawning. Here a fake supervisor records the SIGTERM.
test_escalation_stops_supervisor_before_sigkill() {
    echo "test: escalation SIGTERMs the engine's supervisor before SIGKILL"
    local tmp; tmp="$(mktemp -d)"
    local sup_marker="$tmp/sup_got_term"

    # Fake supervisor: records SIGTERM then exits (mirrors run_supervised's
    # SIGTERM handler, which stops the loop instead of respawning). Uses
    # `sleep & wait` like run_supervised so the trap fires on signal — a
    # foreground `sleep` would defer the trap until it ends (engine_supervisor_
    # test.sh test 7 uses the same pattern for the same reason).
    bash -c 'trap "echo got > \"$1\"; exit 0" TERM; sleep 30 & wait $!' _ "$sup_marker" &
    SUP_PID=$!
    disown "$SUP_PID" 2>/dev/null || true

    # Fake wedged engine occupant.
    sleep 600 &
    OCC_PID=$!
    disown "$OCC_PID" 2>/dev/null || true

    # Port is held until the occupant dies; lsof reports it; ps reports it as a
    # lucidos-engine whose parent (ppid) is our fake supervisor.
    port_is_free() { ! kill -0 "$OCC_PID" 2>/dev/null; }
    lsof() { echo "$OCC_PID"; }
    ps() {
        case "$*" in
            *comm=*) echo "lucidos-engine" ;;
            *ppid=*) echo "$SUP_PID" ;;
        esac
    }

    wait_for_engine_shutdown 65000 "" 1 >/dev/null 2>&1

    unset -f port_is_free lsof ps

    # Allow the fake supervisor a moment to handle SIGTERM.
    local deadline=$(( SECONDS + 3 ))
    while (( SECONDS < deadline )); do [ -f "$sup_marker" ] && break; sleep 0.1; done

    if [ -f "$sup_marker" ]; then
        pass "supervisor received SIGTERM (will not respawn the SIGKILL'd engine)"
    else
        fail "supervisor was NOT SIGTERM'd — a respawn could fight the rebuild"
    fi

    kill -KILL "$OCC_PID" "$SUP_PID" 2>/dev/null || true
    wait "$OCC_PID" 2>/dev/null || true
    wait "$SUP_PID" 2>/dev/null || true
    rm -rf "$tmp"
    unset OCC_PID SUP_PID
}

test_returns_when_port_free_and_no_pids
test_times_out_while_port_held
test_waits_for_pid_to_exit
test_returns_when_port_flips_free
test_full_wait_succeeds_without_escalation
test_full_wait_escalates_to_sigkill
test_full_wait_warns_when_unrecoverable
test_escalation_stops_supervisor_before_sigkill
test_kill_stale_does_not_abort_caller_under_set_e

echo ""
echo "Passed: $PASS  Failed: $FAIL"
[ $FAIL -eq 0 ]
