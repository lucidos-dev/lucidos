#!/bin/bash
# Tests for scripts/lib/gateway_supervisor.sh — the watchdog loop that
# auto-restarts the machine-global workspace gateway on unexpected death
# (SIGKILL, OOM, panic) but, unlike the engine supervisor, IGNORES terminal /
# launcher signals (SIGHUP/SIGINT/SIGTERM) so it survives the launching shell and
# terminal. The only legitimate stops are the child exiting cleanly (0, the
# default exit of SIGINT or SIGUSR1, or already-running), which the `-b` stop
# path triggers via SIGUSR1 to the gateway child.
#
# Run: ./scripts/lib/gateway_supervisor_test.sh
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

# shellcheck source=gateway_supervisor.sh
source "$SCRIPT_DIR/gateway_supervisor.sh"

# Block until $1 exists and is non-empty, or timeout after $2 seconds.
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

# Kill the pid a mock recorded in $1, if any. Mocks record their own pid, as
# the real gateway publishes its own.
kill_child_pid() {
    local child_file="$1"
    [ -s "$child_file" ] || return 0
    kill -KILL "$(cat "$child_file")" 2>/dev/null || true
}

# Write a mock at $1 that records its pid in $2, then sleeps.
write_sleeping_mock() {
    local mock="$1" child_file="$2"
    cat > "$mock" <<EOF
#!/bin/bash
echo \$\$ > "$child_file"
exec sleep 600
EOF
    chmod +x "$mock"
}

# ── Test 1: the supervisor writes no pidfile ────────────────────────────
# The gateway publishes gateway.pid itself once it owns the data dir. A
# supervisor write would let a losing launch overwrite the winner's pid.
test_supervisor_writes_no_pidfile() {
    echo "test: the supervisor leaves its state dir alone on a launch"
    local tdir="$SANDBOX/t1"; mkdir -p "$tdir/state"
    local logfile="$tdir/log"
    write_sleeping_mock "$tdir/mock" "$tdir/child"

    ( run_gateway_supervised "$tdir/state" "$logfile" "$tdir/mock" ) &
    local sup_pid=$!
    CLEANUP_PIDS+=("$sup_pid")

    if ! wait_for_file "$tdir/child" 3; then
        fail "the child did not start within 3s"
        kill -KILL "$sup_pid" 2>/dev/null || true; return
    fi
    if [ -n "$(ls -A "$tdir/state")" ]; then
        fail "the supervisor wrote into its state dir: $(ls -A "$tdir/state")"
    else
        pass "no pidfile: only the gateway names itself"
    fi
    kill -KILL "$sup_pid" 2>/dev/null || true
    kill_child_pid "$tdir/child"
    wait_for_exit "$sup_pid" 3
}

# ── Test 2: each clean child exit stops the supervisor (stay dead) ───────
assert_clean_exit_stops_loop() {
    local exit_code="$1" tname="$2"
    echo "test: child exit $exit_code stops the supervisor"
    local tdir="$SANDBOX/$tname"; mkdir -p "$tdir"
    local logfile="$tdir/log" counter="$tdir/counter"
    echo 0 > "$counter"

    local mock="$tdir/mock"
    cat > "$mock" <<EOF
#!/bin/bash
n=\$(cat "$counter")
echo \$((n + 1)) > "$counter"
exit $exit_code
EOF
    chmod +x "$mock"

    ( run_gateway_supervised "$tdir" "$logfile" "$mock" ) &
    local sup_pid=$!
    CLEANUP_PIDS+=("$sup_pid")

    if ! wait_for_exit "$sup_pid" 5; then
        fail "supervisor did not exit within 5s after child exit $exit_code"
        kill -KILL "$sup_pid" 2>/dev/null || true; return
    fi
    local n; n="$(cat "$counter")"
    if [ "$n" -ne 1 ]; then
        fail "expected child to run once on exit $exit_code, ran $n times"; return
    fi
    pass "supervisor exited after a single exit $exit_code"
}

test_clean_exit_zero_stops_loop() { assert_clean_exit_stops_loop 0 t2; }
test_clean_exit_usr1_stops_loop() { assert_clean_exit_stops_loop $((128 + $(kill -l USR1))) t3; }
# A launch that lost the data dir to a running gateway (server.rs
# ALREADY_RUNNING_EXIT, pinned there). Respawning it would
# lose again every backoff, forever.
test_already_running_stops_loop() { assert_clean_exit_stops_loop 75 t3b; }

# ── Test 3: unexpected exit (137 = SIGKILL) triggers restart ─────────────
test_unexpected_exit_137_restarts() {
    echo "test: child exit 137 (SIGKILL) triggers restart"
    local tdir="$SANDBOX/t4"; mkdir -p "$tdir"
    local logfile="$tdir/log" counter="$tdir/counter"
    echo 0 > "$counter"

    local mock="$tdir/mock"
    cat > "$mock" <<EOF
#!/bin/bash
n=\$(cat "$counter")
echo \$((n + 1)) > "$counter"
if [ "\$n" -ge 2 ]; then
    echo \$\$ > "$tdir/child"
    exec sleep 600
else
    exit 137
fi
EOF
    chmod +x "$mock"

    ( run_gateway_supervised "$tdir" "$logfile" "$mock" ) &
    local sup_pid=$!
    CLEANUP_PIDS+=("$sup_pid")

    if ! wait_for_counter "$counter" 3 15; then
        fail "child only ran $(cat "$counter") times — expected ≥ 3 (initial + 2 restarts)"
        kill -KILL "$sup_pid" 2>/dev/null || true; kill_child_pid "$tdir/child"; return
    fi
    pass "supervisor restarted child after exit 137 (ran $(cat "$counter") times)"
    kill -KILL "$sup_pid" 2>/dev/null || true
    kill_child_pid "$tdir/child"
    wait_for_exit "$sup_pid" 3
}

# ── Test 4: THE detach property — HUP/INT/TERM neither stop the supervisor
#    nor reach the child. This is the whole point of decoupling the gateway
#    supervisor from the engine's: a closing terminal (SIGHUP), a Ctrl-C on the
#    launcher (SIGINT), or a stray SIGTERM must leave the machine-global gateway
#    running.
test_terminal_signals_are_ignored() {
    echo "test: SIGHUP/SIGINT/SIGTERM to supervisor are ignored (gateway survives)"
    local tdir="$SANDBOX/t6"; mkdir -p "$tdir"
    local logfile="$tdir/log"
    write_sleeping_mock "$tdir/mock" "$tdir/child"

    ( run_gateway_supervised "$tdir" "$logfile" "$tdir/mock" ) &
    local sup_pid=$!
    CLEANUP_PIDS+=("$sup_pid")

    if ! wait_for_file "$tdir/child" 3; then
        fail "the child did not start"; kill -KILL "$sup_pid" 2>/dev/null || true; return
    fi
    local child_pid; child_pid="$(cat "$tdir/child")"
    CLEANUP_PIDS+=("$child_pid")

    kill -HUP "$sup_pid" 2>/dev/null || true
    kill -INT "$sup_pid" 2>/dev/null || true
    kill -TERM "$sup_pid" 2>/dev/null || true

    # Give the signals a moment to (not) take effect.
    sleep 1

    if ! kill -0 "$sup_pid" 2>/dev/null; then
        fail "supervisor died on a terminal signal (HUP/INT/TERM) — not detached"
        return
    fi
    if ! kill -0 "$child_pid" 2>/dev/null; then
        fail "gateway child died on a terminal signal — not detached"
        kill -KILL "$sup_pid" 2>/dev/null || true; return
    fi
    pass "supervisor + gateway child both survived HUP/INT/TERM"
    kill -KILL "$sup_pid" "$child_pid" 2>/dev/null || true
    wait_for_exit "$sup_pid" 3
}

# ── Test 5: respawn sidecar written on unexpected death ──────────────────
test_respawn_sidecar_written_on_unexpected_death() {
    echo "test: respawn sidecar carries old_pid + exit_code on unexpected death"
    local tdir="$SANDBOX/t7"; mkdir -p "$tdir"
    local logfile="$tdir/log"
    local sidecar="$tdir/gateway.last-death.json" counter="$tdir/counter"
    echo 0 > "$counter"

    local mock="$tdir/mock"
    cat > "$mock" <<EOF
#!/bin/bash
n=\$(cat "$counter")
echo \$((n + 1)) > "$counter"
if [ "\$n" -eq 0 ]; then
    exit 137
else
    echo \$\$ > "$tdir/child"
    exec sleep 600
fi
EOF
    chmod +x "$mock"

    ( run_gateway_supervised "$tdir" "$logfile" "$mock" ) &
    local sup_pid=$!
    CLEANUP_PIDS+=("$sup_pid")

    if ! wait_for_counter "$counter" 2 10; then
        fail "supervisor did not respawn within 10s"
        kill -KILL "$sup_pid" 2>/dev/null || true; kill_child_pid "$tdir/child"; return
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
    kill_child_pid "$tdir/child"
    wait_for_exit "$sup_pid" 3
}

test_supervisor_writes_no_pidfile
test_clean_exit_zero_stops_loop
test_clean_exit_usr1_stops_loop
test_already_running_stops_loop
test_unexpected_exit_137_restarts
test_terminal_signals_are_ignored
test_respawn_sidecar_written_on_unexpected_death

echo ""
echo "Passed: $PASS  Failed: $FAIL"
[ $FAIL -eq 0 ]
