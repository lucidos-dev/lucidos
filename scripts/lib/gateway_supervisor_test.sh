#!/bin/bash
# Tests for scripts/lib/gateway_supervisor.sh — the watchdog loop that
# auto-restarts the machine-global workspace gateway on unexpected death
# (SIGKILL, OOM, panic) but, unlike the engine supervisor, IGNORES terminal /
# launcher signals (SIGHUP/SIGINT/SIGTERM) so it survives the launching shell and
# terminal. The only legitimate stop is the child exiting cleanly (0/130/138),
# which the `-b` stop path triggers via SIGUSR1 to the gateway child.
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

kill_pidfile_pid() {
    local pidfile="$1"
    [ -s "$pidfile" ] || return 0
    kill -KILL "$(cat "$pidfile")" 2>/dev/null || true
}

# ── Test 1: pidfile is written after launch ─────────────────────────────
test_pidfile_written_on_launch() {
    echo "test: pidfile is written after launching the child"
    local tdir="$SANDBOX/t1"; mkdir -p "$tdir"
    local pidfile="$tdir/pid" logfile="$tdir/log"

    ( run_gateway_supervised "$pidfile" "$logfile" sleep 600 ) &
    local sup_pid=$!
    CLEANUP_PIDS+=("$sup_pid")

    if ! wait_for_file "$pidfile" 3; then
        fail "pidfile $pidfile not written within 3s"; return
    fi
    local child_pid; child_pid="$(cat "$pidfile")"
    if [ -z "$child_pid" ] || ! kill -0 "$child_pid" 2>/dev/null; then
        fail "pid in $pidfile ($child_pid) is not a live process"; return
    fi
    pass "pidfile contains live child pid $child_pid"
    kill -KILL "$sup_pid" "$child_pid" 2>/dev/null || true
    wait_for_exit "$sup_pid" 3
}

# ── Test 2: clean child exit (0/130/138) stops the supervisor (stay dead) ─
assert_clean_exit_stops_loop() {
    local exit_code="$1" tname="$2"
    echo "test: child exit $exit_code stops the supervisor"
    local tdir="$SANDBOX/$tname"; mkdir -p "$tdir"
    local pidfile="$tdir/pid" logfile="$tdir/log" counter="$tdir/counter"
    echo 0 > "$counter"

    local mock="$tdir/mock"
    cat > "$mock" <<EOF
#!/bin/bash
n=\$(cat "$counter")
echo \$((n + 1)) > "$counter"
exit $exit_code
EOF
    chmod +x "$mock"

    ( run_gateway_supervised "$pidfile" "$logfile" "$mock" ) &
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
test_clean_exit_138_stops_loop()  { assert_clean_exit_stops_loop 138 t3; }

# ── Test 3: unexpected exit (137 = SIGKILL) triggers restart ─────────────
test_unexpected_exit_137_restarts() {
    echo "test: child exit 137 (SIGKILL) triggers restart"
    local tdir="$SANDBOX/t4"; mkdir -p "$tdir"
    local pidfile="$tdir/pid" logfile="$tdir/log" counter="$tdir/counter"
    echo 0 > "$counter"

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

    ( run_gateway_supervised "$pidfile" "$logfile" "$mock" ) &
    local sup_pid=$!
    CLEANUP_PIDS+=("$sup_pid")

    if ! wait_for_counter "$counter" 3 15; then
        fail "child only ran $(cat "$counter") times — expected ≥ 3 (initial + 2 restarts)"
        kill -KILL "$sup_pid" 2>/dev/null || true; kill_pidfile_pid "$pidfile"; return
    fi
    pass "supervisor restarted child after exit 137 (ran $(cat "$counter") times)"
    kill -KILL "$sup_pid" 2>/dev/null || true
    kill_pidfile_pid "$pidfile"
    wait_for_exit "$sup_pid" 3
}

# ── Test 4: pidfile is rewritten to the new pid after a restart ──────────
test_pidfile_updates_on_restart() {
    echo "test: pidfile is rewritten to the new pid after a restart"
    local tdir="$SANDBOX/t5"; mkdir -p "$tdir"
    local pidfile="$tdir/pid" logfile="$tdir/log" counter="$tdir/counter"
    echo 0 > "$counter"

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

    ( run_gateway_supervised "$pidfile" "$logfile" "$mock" ) &
    local sup_pid=$!
    CLEANUP_PIDS+=("$sup_pid")

    if ! wait_for_file "$pidfile" 3; then
        fail "pidfile not written for first invocation"
        kill -KILL "$sup_pid" 2>/dev/null || true; return
    fi
    local first_pid; first_pid="$(cat "$pidfile")"

    if ! wait_for_counter "$counter" 2 10; then
        fail "supervisor did not restart child within 10s"
        kill -KILL "$sup_pid" 2>/dev/null || true; kill_pidfile_pid "$pidfile"; return
    fi

    local deadline=$(( $(date +%s) + 5 )) second_pid=""
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

# ── Test 5: THE detach property — HUP/INT/TERM neither stop the supervisor
#    nor reach the child. This is the whole point of decoupling the gateway
#    supervisor from the engine's: a closing terminal (SIGHUP), a Ctrl-C on the
#    launcher (SIGINT), or a stray SIGTERM must leave the machine-global gateway
#    running.
test_terminal_signals_are_ignored() {
    echo "test: SIGHUP/SIGINT/SIGTERM to supervisor are ignored (gateway survives)"
    local tdir="$SANDBOX/t6"; mkdir -p "$tdir"
    local pidfile="$tdir/pid" logfile="$tdir/log"

    ( run_gateway_supervised "$pidfile" "$logfile" sleep 600 ) &
    local sup_pid=$!
    CLEANUP_PIDS+=("$sup_pid")

    if ! wait_for_file "$pidfile" 3; then
        fail "pidfile not written"; kill -KILL "$sup_pid" 2>/dev/null || true; return
    fi
    local child_pid; child_pid="$(cat "$pidfile")"
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

# ── Test 6: respawn sidecar written on unexpected death ──────────────────
test_respawn_sidecar_written_on_unexpected_death() {
    echo "test: respawn sidecar carries old_pid + exit_code on unexpected death"
    local tdir="$SANDBOX/t7"; mkdir -p "$tdir"
    local pidfile="$tdir/pid" logfile="$tdir/log"
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
    sleep 600
fi
EOF
    chmod +x "$mock"

    ( run_gateway_supervised "$pidfile" "$logfile" "$mock" ) &
    local sup_pid=$!
    CLEANUP_PIDS+=("$sup_pid")

    if ! wait_for_counter "$counter" 2 10; then
        fail "supervisor did not respawn within 10s"
        kill -KILL "$sup_pid" 2>/dev/null || true; kill_pidfile_pid "$pidfile"; return
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
test_clean_exit_138_stops_loop
test_unexpected_exit_137_restarts
test_pidfile_updates_on_restart
test_terminal_signals_are_ignored
test_respawn_sidecar_written_on_unexpected_death

echo ""
echo "Passed: $PASS  Failed: $FAIL"
[ $FAIL -eq 0 ]
