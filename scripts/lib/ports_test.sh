#!/bin/bash
# Tests for scripts/lib/ports.sh.
# Run: ./scripts/lib/ports_test.sh
#
# Two independent test suites in one file:
#   - is_protected_host_pid: the guard that keeps CC subprocesses from killing
#     their own host engine when a test script invokes ports.sh.
#   - allocate_ports: policy tests — default allocation, stability, explicit
#     override (env + lucidos.toml), precedence, collision walk-forward,
#     validation, --engine-only short-circuit.
#
# The allocate_ports tests stub out `port_is_free` and `docker` so they can run
# anywhere without touching real ports. The is_protected_host_pid tests use a
# real long-running process so `kill -0 <pid>` succeeds.

set -u

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"

# Sandbox HOME so the global registry (~/.lucidos/port-registry) is isolated.
SANDBOX="$(mktemp -d)"
trap 'rm -rf "$SANDBOX"; kill $LIVE_PID 2>/dev/null || true' EXIT
export HOME="$SANDBOX"

# Reset any inherited overrides
unset LUCIDOS_VITE_PORT VITE_PORT_OVERRIDE 2>/dev/null || true
# allocate_ports reads ENGINE_ONLY via ${ENGINE_ONLY:-}; initialize to empty so
# the test runs under `set -u`.
export ENGINE_ONLY=""

# A real long-running process so `kill -0 <pid>` succeeds — needed for the
# pidfile-scan branch which requires the recorded PID to actually be alive.
sleep 600 &
LIVE_PID=$!
disown "$LIVE_PID" 2>/dev/null || true

# shellcheck source=ports.sh
source "$SCRIPT_DIR/ports.sh"

PASS=0
FAIL=0
fail() { echo "  FAIL: $*"; FAIL=$((FAIL+1)); }
pass() { echo "  ok:   $*"; PASS=$((PASS+1)); }

# ── stubs (for allocate_ports tests) ───────────────────────────────────
# By default no ports are occupied. Tests can override OCCUPIED_PORTS to
# simulate squatters.
OCCUPIED_PORTS=""
port_is_free() {
    local port="$1"
    case " $OCCUPIED_PORTS " in
        *" $port "*) return 1 ;;
        *) return 0 ;;
    esac
}

# Stub docker so the PG-container detection path doesn't shell out.
docker() { return 1; }

# ── helpers ────────────────────────────────────────────────────────────
reset_env() {
    unset LUCIDOS_HOST_PID LUCIDOS_FRONTEND_PID
    rm -rf "$HOME/workspaces"
}

write_pidfile() {
    local ws="$1"
    local kind="$2"   # engine or frontend
    local pid="$3"
    local dir="$HOME/workspaces/$ws/.lucidos"
    mkdir -p "$dir"
    echo "$pid" > "$dir/$kind.pid"
}

reset_state() {
    rm -rf "$SANDBOX"/* 2>/dev/null || true
    rm -rf "$SANDBOX"/.lucidos 2>/dev/null || true
    OCCUPIED_PORTS=""
    unset LUCIDOS_VITE_PORT VITE_PORT_OVERRIDE 2>/dev/null || true
    export ENGINE_ONLY=""
    unset API_PORT VITE_PORT PG_PORT 2>/dev/null || true
}

make_workspace() {
    local ws="$1"
    mkdir -p "$ws/.lucidos"
    echo "$ws"
}

# ═══════════════════════════════════════════════════════════════════════
# Suite 1: is_protected_host_pid
# ═══════════════════════════════════════════════════════════════════════

# ── Test 1: LUCIDOS_HOST_PID env var protects that PID ─────────────────
test_protects_host_pid_from_env_var() {
    echo "test: LUCIDOS_HOST_PID protects the named PID"
    reset_env
    export LUCIDOS_HOST_PID="$LIVE_PID"
    if is_protected_host_pid "$LIVE_PID"; then
        pass "is_protected_host_pid($LIVE_PID) returned 0"
    else
        fail "expected 0 for LUCIDOS_HOST_PID match, got $?"
    fi
}

# ── Test 2: LUCIDOS_FRONTEND_PID env var protects that PID ─────────────
test_protects_frontend_pid_from_env_var() {
    echo "test: LUCIDOS_FRONTEND_PID protects the named PID"
    reset_env
    export LUCIDOS_FRONTEND_PID="$LIVE_PID"
    if is_protected_host_pid "$LIVE_PID"; then
        pass "is_protected_host_pid($LIVE_PID) returned 0"
    else
        fail "expected 0 for LUCIDOS_FRONTEND_PID match, got $?"
    fi
}

# ── Test 3: PID from another workspace's engine.pid is protected ───────
# Mirrors the original incident: CC running in workspace "dev" invokes a
# test that runs ports.sh for workspace "e2e-test". The dev engine PID
# isn't in the e2e-test invocation's env, but it IS on disk under
# ~/workspaces/dev/.lucidos/engine.pid. We must protect it anyway.
test_protects_pid_from_other_workspace_pidfile() {
    echo "test: another workspace's engine.pid protects that PID"
    reset_env
    write_pidfile "dev" "engine" "$LIVE_PID"
    if is_protected_host_pid "$LIVE_PID"; then
        pass "scanned ~/workspaces/dev/.lucidos/engine.pid"
    else
        fail "expected 0 for pidfile-scan match, got $?"
    fi
}

# ── Test 4: frontend.pid on disk is also protected ─────────────────────
test_protects_pid_from_other_workspace_frontend_pidfile() {
    echo "test: another workspace's frontend.pid protects that PID"
    reset_env
    write_pidfile "personal" "frontend" "$LIVE_PID"
    if is_protected_host_pid "$LIVE_PID"; then
        pass "scanned ~/workspaces/personal/.lucidos/frontend.pid"
    else
        fail "expected 0 for frontend pidfile-scan match, got $?"
    fi
}

# ── Test 5: arbitrary PID is NOT protected ─────────────────────────────
# Picking a high random PID that's almost certainly not alive and not in
# any pidfile.
test_does_not_protect_arbitrary_pid() {
    echo "test: arbitrary PID is not protected"
    reset_env
    if is_protected_host_pid 999999; then
        fail "is_protected_host_pid(999999) unexpectedly returned 0"
    else
        pass "is_protected_host_pid(999999) returned non-zero"
    fi
}

# ── Test 6: stale pidfile (recorded PID dead) does NOT protect ─────────
# The pidfile-scan branch requires `kill -0 <pid>` to succeed. A stale
# pidfile pointing to a dead PID is meaningless — and we mustn't refuse
# to kill an unrelated process just because it happens to recycle that
# PID number now. Only LIVE protected processes count.
test_stale_pidfile_does_not_protect() {
    echo "test: stale pidfile does not protect a recycled PID"
    reset_env
    write_pidfile "ghost" "engine" 999999
    if is_protected_host_pid 999999; then
        fail "stale pidfile (999999) unexpectedly protected"
    else
        pass "stale pidfile did not protect"
    fi
}

# ── Test 7: empty PID is not protected (defensive) ─────────────────────
test_empty_pid_is_not_protected() {
    echo "test: empty PID is treated as not protected"
    reset_env
    if is_protected_host_pid ""; then
        fail "empty PID unexpectedly returned 0"
    else
        pass "empty PID returned non-zero"
    fi
}

# ── Test 8: pidfile match still requires liveness even when env unset ──
# Ensures the env-var branch and pidfile-scan branch are independent —
# the pidfile scan should NOT match a dead PID even if neither env var
# is set (covers a regression where the early-return shape forgot the
# kill -0 check on the pidfile path).
test_no_env_vars_no_match_for_dead_pidfile_pid() {
    echo "test: no env vars + dead pidfile PID → not protected"
    reset_env
    write_pidfile "ghost2" "engine" 999998
    if is_protected_host_pid 999998; then
        fail "dead pidfile (999998) protected despite no env vars"
    else
        pass "dead pidfile did not protect"
    fi
}

# ═══════════════════════════════════════════════════════════════════════
# Suite 2: allocate_ports
# ═══════════════════════════════════════════════════════════════════════

# ── Test 9: default allocation uses offset 0 ───────────────────────────
test_default_allocation_uses_offset_0() {
    reset_state
    echo "test: default allocation uses offset 0 (API=3000, Vite=5173, PG=5432)"
    local ws
    ws="$(make_workspace "$SANDBOX/ws-a")"

    if allocate_ports "$ws" > /dev/null 2>&1; then
        [ "$API_PORT" = "3000" ] && [ "$VITE_PORT" = "5173" ] && [ "$PG_PORT" = "5432" ] \
            && pass "default ports assigned" \
            || fail "expected 3000/5173/5432, got $API_PORT/$VITE_PORT/$PG_PORT"
    else
        fail "allocate_ports failed"
    fi
}

# ── Test 10: allocation is stable across calls ─────────────────────────
test_allocation_is_stable_across_calls() {
    reset_state
    echo "test: allocation is stable across repeated calls"
    local ws
    ws="$(make_workspace "$SANDBOX/ws-b")"

    allocate_ports "$ws" > /dev/null 2>&1
    local first_vite="$VITE_PORT"
    allocate_ports "$ws" > /dev/null 2>&1
    local second_vite="$VITE_PORT"

    [ "$first_vite" = "$second_vite" ] \
        && pass "ports stable across calls ($first_vite)" \
        || fail "ports changed: first=$first_vite, second=$second_vite"
}

# ── Test 11: two workspaces get different offsets ──────────────────────
test_two_workspaces_get_different_offsets() {
    reset_state
    echo "test: two workspaces get different offsets"
    local wsa wsb
    wsa="$(make_workspace "$SANDBOX/ws-c")"
    wsb="$(make_workspace "$SANDBOX/ws-d")"

    allocate_ports "$wsa" > /dev/null 2>&1
    local a_vite="$VITE_PORT"
    allocate_ports "$wsb" > /dev/null 2>&1
    local b_vite="$VITE_PORT"

    [ "$a_vite" != "$b_vite" ] \
        && pass "got distinct vite ports: a=$a_vite, b=$b_vite" \
        || fail "both workspaces got vite port $a_vite"
}

# ── Test 12: explicit port override via LUCIDOS_VITE_PORT env var ──────
test_explicit_port_override_via_env_var() {
    reset_state
    echo "test: LUCIDOS_VITE_PORT env var pins the vite port"
    local ws
    ws="$(make_workspace "$SANDBOX/ws-e")"

    export LUCIDOS_VITE_PORT=5273
    allocate_ports "$ws" > /dev/null 2>&1

    [ "$VITE_PORT" = "5273" ] \
        && pass "vite port = $VITE_PORT (override honored)" \
        || fail "expected VITE_PORT=5273, got $VITE_PORT"

    # API and PG should shift by the same offset (100): API=3100, PG=5532
    [ "$API_PORT" = "3100" ] \
        && pass "API port shifted to $API_PORT (offset 100)" \
        || fail "expected API_PORT=3100, got $API_PORT"

    [ "$PG_PORT" = "5532" ] \
        && pass "PG port shifted to $PG_PORT" \
        || fail "expected PG_PORT=5532, got $PG_PORT"
}

# ── Test 13: explicit port override via lucidos.toml ───────────────────
test_explicit_port_override_via_lucidos_toml() {
    reset_state
    echo "test: lucidos.toml [ports] vite = N pins the vite port"
    local ws
    ws="$(make_workspace "$SANDBOX/ws-f")"

    cat > "$ws/lucidos.toml" <<EOF
[ports]
vite = 5300
EOF

    allocate_ports "$ws" > /dev/null 2>&1

    [ "$VITE_PORT" = "5300" ] \
        && pass "lucidos.toml honored: VITE_PORT=$VITE_PORT" \
        || fail "expected VITE_PORT=5300, got $VITE_PORT"
}

# ── Test 14: env var beats lucidos.toml ────────────────────────────────
test_env_var_beats_lucidos_toml() {
    reset_state
    echo "test: LUCIDOS_VITE_PORT env var beats lucidos.toml"
    local ws
    ws="$(make_workspace "$SANDBOX/ws-g")"

    cat > "$ws/lucidos.toml" <<EOF
[ports]
vite = 5300
EOF
    export LUCIDOS_VITE_PORT=5400

    allocate_ports "$ws" > /dev/null 2>&1

    [ "$VITE_PORT" = "5400" ] \
        && pass "env var won: VITE_PORT=$VITE_PORT" \
        || fail "expected VITE_PORT=5400 (env), got $VITE_PORT"
}

# ── Test 15: collision walks forward to next free offset ───────────────
test_collision_walks_forward() {
    reset_state
    echo "test: collision on the default port walks forward to the next free offset"
    local ws
    ws="$(make_workspace "$SANDBOX/ws-h")"

    # Simulate a foreign process holding 5173 (Akram's ua-backoffice case)
    OCCUPIED_PORTS="5173"

    allocate_ports "$ws" > /dev/null 2>&1

    # Expect offset 1 (vite=5174). Cannot land on 5173.
    [ "$VITE_PORT" -ge "5174" ] \
        && pass "walked forward: VITE_PORT=$VITE_PORT (avoided 5173)" \
        || fail "expected VITE_PORT≥5174, got $VITE_PORT"

    [ "$VITE_PORT" != "5173" ] \
        && pass "did not pick collided port" \
        || fail "VITE_PORT landed on collided 5173"
}

# ── Test 16: collision walk persists in registry ───────────────────────
test_collision_walk_persists() {
    reset_state
    echo "test: collision-bumped offset persists across calls"
    local ws
    ws="$(make_workspace "$SANDBOX/ws-i")"

    # First call hits collision on 5173 → walks to 5174 (or higher)
    OCCUPIED_PORTS="5173"
    allocate_ports "$ws" > /dev/null 2>&1
    local first_vite="$VITE_PORT"

    # Second call: no collision (squatter gone), but registry should remember
    OCCUPIED_PORTS=""
    allocate_ports "$ws" > /dev/null 2>&1
    local second_vite="$VITE_PORT"

    [ "$first_vite" = "$second_vite" ] \
        && pass "persisted offset reused: $second_vite" \
        || fail "first=$first_vite, second=$second_vite — drift"
}

# ── Test 17: override collision walks forward and logs ─────────────────
test_override_collision_walks_forward() {
    reset_state
    echo "test: explicit override colliding with squatter walks forward"
    local ws
    ws="$(make_workspace "$SANDBOX/ws-j")"

    export LUCIDOS_VITE_PORT=5273
    OCCUPIED_PORTS="5273"

    # Capture stderr to a file so the function runs in the parent shell
    # (a $() subshell would lose the exported VITE_PORT).
    local errfile="$SANDBOX/test17.err"
    allocate_ports "$ws" 2> "$errfile" 1>/dev/null
    local rc=$?

    [ "$rc" = "0" ] \
        && pass "allocate_ports succeeded under override+collision" \
        || fail "allocate_ports returned $rc, stderr: $(cat "$errfile")"

    [ "${VITE_PORT:-0}" != "5273" ] && [ "${VITE_PORT:-0}" -ge "5274" ] \
        && pass "override+collision walked forward: VITE_PORT=$VITE_PORT" \
        || fail "expected VITE_PORT≥5274 (not 5273), got ${VITE_PORT:-unset}"
}

# ── Test 18: chosen ports are logged to stderr ─────────────────────────
test_chosen_ports_are_logged() {
    reset_state
    echo "test: chosen ports are loudly logged to stderr"
    local ws
    ws="$(make_workspace "$SANDBOX/ws-k")"

    local out
    out=$(allocate_ports "$ws" 2>&1 1>/dev/null)

    case "$out" in
        *"5173"*) pass "log mentions the chosen port" ;;
        *) fail "expected '5173' in stderr, got: $out" ;;
    esac
}

# ── Test 19: lucidos.toml with quoted vite value ───────────────────────
test_lucidos_toml_with_quoted_vite_value() {
    reset_state
    echo "test: lucidos.toml with vite = \"5320\" (quoted) is honored as int"
    local ws
    ws="$(make_workspace "$SANDBOX/ws-l")"

    cat > "$ws/lucidos.toml" <<EOF
[ports]
vite = "5320"
EOF

    allocate_ports "$ws" > /dev/null 2>&1

    [ "$VITE_PORT" = "5320" ] \
        && pass "quoted toml value parsed: VITE_PORT=$VITE_PORT" \
        || fail "expected VITE_PORT=5320, got $VITE_PORT"
}

# ── Test 20: non-numeric LUCIDOS_VITE_PORT is rejected ────────────────
test_non_numeric_env_var_is_rejected() {
    reset_state
    echo "test: LUCIDOS_VITE_PORT=abc is rejected with a clear error"
    local ws
    ws="$(make_workspace "$SANDBOX/ws-m")"

    export LUCIDOS_VITE_PORT=abc
    local errfile="$SANDBOX/test20.err"
    if allocate_ports "$ws" 2> "$errfile" 1>/dev/null; then
        fail "expected allocate_ports to fail on non-numeric override, got rc=0 VITE_PORT=${VITE_PORT:-unset}"
    else
        pass "non-numeric override rejected (rc != 0)"
    fi
    case "$(cat "$errfile")" in
        *LUCIDOS_VITE_PORT*) pass "error message names LUCIDOS_VITE_PORT" ;;
        *) fail "error should name LUCIDOS_VITE_PORT, got: $(cat "$errfile")" ;;
    esac
}

# ── Test 21: out-of-range LUCIDOS_VITE_PORT is rejected ───────────────
test_out_of_range_env_var_is_rejected() {
    reset_state
    echo "test: LUCIDOS_VITE_PORT=100 (< 5173) is rejected"
    local ws
    ws="$(make_workspace "$SANDBOX/ws-n")"

    export LUCIDOS_VITE_PORT=100
    local errfile="$SANDBOX/test21.err"
    if allocate_ports "$ws" 2> "$errfile" 1>/dev/null; then
        fail "expected allocate_ports to fail on port <5173, got rc=0 API=${API_PORT:-unset}"
    else
        pass "out-of-range override rejected (rc != 0)"
    fi
}

# ── Test 22: bogus toml vite value is rejected ────────────────────────
test_bogus_toml_value_is_rejected() {
    reset_state
    echo "test: lucidos.toml with vite = \"abc\" is rejected"
    local ws
    ws="$(make_workspace "$SANDBOX/ws-o")"

    cat > "$ws/lucidos.toml" <<EOF
[ports]
vite = "abc"
EOF
    local errfile="$SANDBOX/test22.err"
    if allocate_ports "$ws" 2> "$errfile" 1>/dev/null; then
        fail "expected allocate_ports to fail on non-numeric toml value"
    else
        pass "non-numeric toml value rejected"
    fi
}

# ── Test 23a: stale lucidos-engine on registered port is reclaimed ─────
# Regression: work workspace was at offset 5 in the registry, but each
# restart silently drifted because allocate_ports walked past any port
# that lsof reported occupied — including ports held by our own crashed/
# orphaned lucidos-engine that nothing else would ever clean up. Once
# walked, the new offset was persisted and the port had drifted forever.
# Fix: _port_is_ours_or_free first asks _try_reclaim_stale_lucidos_on_port
# to free the port; only genuine foreign occupiers cause a walk.
test_stale_lucidos_engine_reclaimed_no_drift() {
    reset_state
    echo "test: stale lucidos-engine on the registered port is reclaimed → no drift"
    local ws
    ws="$(make_workspace "$SANDBOX/ws-stale-self")"

    # First call: clean — gets offset 0 (vite=5173, api=3000).
    allocate_ports "$ws" > /dev/null 2>&1
    [ "$VITE_PORT" = "5173" ] || { fail "setup: expected VITE_PORT=5173, got $VITE_PORT"; return; }

    # Now both ports look occupied to port_is_free (squatter).
    # Stub lsof so OCCUPIER_PID is set deterministically regardless of
    # whatever the host has on these ports — otherwise the reclaim path
    # is only exercised on machines that happen to have a listener.
    # The reclaim helper, when called, simulates a successful kill by
    # clearing OCCUPIED_PORTS — i.e. proves allocate_ports *consults* the
    # reclaim path before walking, and trusts its success.
    OCCUPIED_PORTS="5173 3000"
    lsof() { echo "12345"; }
    _try_reclaim_stale_lucidos_on_port() {
        local port="$1"
        OCCUPIED_PORTS="$(echo " $OCCUPIED_PORTS " | sed "s/ $port / /g" | sed 's/^ //; s/ $//')"
        return 0
    }

    allocate_ports "$ws" > /dev/null 2>&1

    unset -f _try_reclaim_stale_lucidos_on_port lsof

    [ "$VITE_PORT" = "5173" ] \
        && pass "reclaim succeeded, stayed on offset 0 (VITE_PORT=$VITE_PORT)" \
        || fail "drifted to VITE_PORT=$VITE_PORT (expected 5173 after reclaim)"
}

# ── Test 23b: foreign occupier still walks (no regression) ─────────────
# When the reclaim helper refuses (genuinely foreign process), the walk-
# forward fallback must still kick in so the workspace eventually gets a
# port. Mirrors the policy in test_collision_walks_forward but explicit
# about the new reclaim integration.
test_foreign_occupier_still_walks_after_reclaim_refuses() {
    reset_state
    echo "test: foreign occupier + reclaim refuses → walks forward (no regression)"
    local ws
    ws="$(make_workspace "$SANDBOX/ws-foreign")"

    OCCUPIED_PORTS="5173"
    _try_reclaim_stale_lucidos_on_port() { return 1; }

    allocate_ports "$ws" > /dev/null 2>&1

    unset -f _try_reclaim_stale_lucidos_on_port

    [ "$VITE_PORT" -ge "5174" ] \
        && pass "walked past foreign occupier: VITE_PORT=$VITE_PORT" \
        || fail "expected VITE_PORT≥5174, got $VITE_PORT"
}

# ── Test 23: allocate_ports in --engine-only mode returns 0 without
# trying to free VITE_PORT. Pre-fix, the kill-guard refused to signal
# the workspace's own engine on VITE_PORT and allocate_ports returned 1
# with "Port still occupied", leaving the old engine in place and
# silently breaking the engine-only restart flow.
test_allocate_ports_engine_only_short_circuits() {
    reset_state
    echo "test: allocate_ports --engine-only short-circuits past cleanup"
    local ws="$HOME/workspaces/restart-target"
    mkdir -p "$ws/.lucidos"
    # The workspace's "engine" — a live sleep we expect to survive.
    sleep 600 &
    local fake_engine_pid=$!
    disown "$fake_engine_pid" 2>/dev/null || true
    echo "$fake_engine_pid" > "$ws/.lucidos/engine.pid"

    # Run in --engine-only mode. Should return 0 immediately and NOT
    # invoke any of the lsof/kill machinery.
    (
        export ENGINE_ONLY=1
        allocate_ports "$ws"
    )
    local rc=$?

    if [ $rc -ne 0 ]; then
        fail "allocate_ports returned $rc in --engine-only mode (expected 0)"
    elif ! kill -0 "$fake_engine_pid" 2>/dev/null; then
        fail "fake engine pid $fake_engine_pid was killed by allocate_ports"
    elif [ ! -f "$ws/.lucidos/ports" ]; then
        fail "allocate_ports did not write $ws/.lucidos/ports"
    else
        pass "short-circuited; fake engine survived; ports file written"
    fi

    kill "$fake_engine_pid" 2>/dev/null || true
}

# ── Test 24: .lucidos/ports records the user-facing port (post-swap),
# not the raw 3000-range API_PORT. Consumers (status.sh, frontend
# bookmarks) read this file as "the URL".
test_ports_file_records_user_facing_port() {
    reset_state
    echo "test: .lucidos/ports records the user-facing port (post-swap), not the raw API port"
    local ws
    ws="$(make_workspace "$SANDBOX/ws-userfacing")"

    allocate_ports "$ws" > /dev/null 2>&1

    local ports_file="$ws/.lucidos/ports"
    [ -f "$ports_file" ] || { fail "ports file not written"; return; }

    # offset 0 → vite=5173 (user-facing), api=3000 (would be the raw value
    # we DON'T want to see in the file).
    local recorded_api recorded_vite
    recorded_api=$(awk -F= '/^API_PORT=/  { print $2 }' "$ports_file")
    recorded_vite=$(awk -F= '/^VITE_PORT=/ { print $2 }' "$ports_file")

    [ "$recorded_api" = "5173" ] \
        && pass "API_PORT in file = $recorded_api (user-facing)" \
        || fail "expected API_PORT=5173 (user-facing), got $recorded_api (would leak raw 3000-range to consumers)"

    [ "$recorded_vite" = "5173" ] \
        && pass "VITE_PORT in file = $recorded_vite (user-facing)" \
        || fail "expected VITE_PORT=5173, got $recorded_vite"

    [ "$recorded_api" = "$recorded_vite" ] \
        && pass "both keys agree on the same user-facing port" \
        || fail "API_PORT ($recorded_api) and VITE_PORT ($recorded_vite) disagree — consumers will pick the wrong one"
}

# ── Test 25: --engine-only mode writes the user-facing port to
# .lucidos/ports without depending on swap_ports running afterwards.
test_ports_file_correct_in_engine_only_mode() {
    reset_state
    echo "test: --engine-only mode writes user-facing port to .lucidos/ports (no dependency on swap_ports)"
    local ws
    ws="$(make_workspace "$SANDBOX/ws-engine-only")"

    (
        export ENGINE_ONLY=1
        allocate_ports "$ws"
    ) > /dev/null 2>&1

    local recorded_api recorded_vite
    recorded_api=$(awk -F= '/^API_PORT=/  { print $2 }' "$ws/.lucidos/ports")
    recorded_vite=$(awk -F= '/^VITE_PORT=/ { print $2 }' "$ws/.lucidos/ports")

    [ "$recorded_api" = "5173" ] && [ "$recorded_vite" = "5173" ] \
        && pass "--engine-only ports file: API_PORT=VITE_PORT=$recorded_api" \
        || fail "expected both = 5173 (user-facing), got API=$recorded_api VITE=$recorded_vite"
}

# ── Test 26: registry_save under N concurrent writers preserves every
# entry exactly once. Each writer must serialize behind the registry
# lock so no read-modify-write window can lose a sibling's append.
test_registry_save_handles_concurrent_writers() {
    reset_state
    echo "test: registry_save handles concurrent writers without corruption"

    local N=20
    local i
    for i in $(seq 1 $N); do
        ( registry_save "/sandbox/ws-$i" "$i" ) &
    done
    wait

    local lines unique_workspaces
    lines=$(wc -l < "$LUCIDOS_PORT_REGISTRY" | tr -d ' ')
    unique_workspaces=$(awk -F'\t' '{print $1}' "$LUCIDOS_PORT_REGISTRY" | sort -u | wc -l | tr -d ' ')

    [ "$lines" = "$N" ] \
        && pass "registry has exactly $N entries after $N concurrent writers" \
        || fail "expected $N entries, got $lines (corruption or lost writes)"

    [ "$unique_workspaces" = "$N" ] \
        && pass "all $N workspaces are present (no clobber)" \
        || fail "expected $N unique workspaces, got $unique_workspaces"
}

# ── Test 27: registry_save failure propagates through allocate_ports.
# A lock-acquire timeout or mktemp failure must bubble up — silently
# swallowing it leaves the file written but the offset unpersisted, so
# the next restart can't find the entry and picks a fresh one (drift).
test_registry_save_failure_propagates() {
    reset_state
    echo "test: registry_save failure propagates as allocate_ports non-zero"
    local ws
    ws="$(make_workspace "$SANDBOX/ws-fail-prop")"

    # Force registry_save to fail. The override stays in effect only for
    # this test (reset_state restores the function table on next run via
    # re-source) — but reset_state doesn't re-source, so unset explicitly.
    registry_save() { return 1; }

    if allocate_ports "$ws" > /dev/null 2>&1; then
        fail "expected allocate_ports to return non-zero when registry_save fails"
    else
        pass "allocate_ports propagated registry_save failure"
    fi

    unset -f registry_save
}

# ── run all ────────────────────────────────────────────────────────────
test_protects_host_pid_from_env_var
test_protects_frontend_pid_from_env_var
test_protects_pid_from_other_workspace_pidfile
test_protects_pid_from_other_workspace_frontend_pidfile
test_does_not_protect_arbitrary_pid
test_stale_pidfile_does_not_protect
test_empty_pid_is_not_protected
test_no_env_vars_no_match_for_dead_pidfile_pid
test_default_allocation_uses_offset_0
test_allocation_is_stable_across_calls
test_two_workspaces_get_different_offsets
test_explicit_port_override_via_env_var
test_explicit_port_override_via_lucidos_toml
test_env_var_beats_lucidos_toml
test_collision_walks_forward
test_collision_walk_persists
test_override_collision_walks_forward
test_chosen_ports_are_logged
test_lucidos_toml_with_quoted_vite_value
test_non_numeric_env_var_is_rejected
test_out_of_range_env_var_is_rejected
test_bogus_toml_value_is_rejected
test_stale_lucidos_engine_reclaimed_no_drift
test_foreign_occupier_still_walks_after_reclaim_refuses
test_allocate_ports_engine_only_short_circuits
test_ports_file_records_user_facing_port
test_ports_file_correct_in_engine_only_mode
test_registry_save_handles_concurrent_writers
test_registry_save_failure_propagates

echo ""
echo "Results: $PASS passed, $FAIL failed"
exit "$FAIL"
